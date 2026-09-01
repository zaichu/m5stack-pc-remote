// HMAC-signed commands to the Windows Agent (REBOOT / SHUTDOWN).
//
// Port of firmware/src/power_controller.cpp's postAgentCommand(). The wire
// format must stay byte-identical to the C++ implementation and to
// windows-agent's verifier:
//
//   canonical = "POST\n" + path + "\n" + timestamp + "\n" + nonce + "\n"
//               + sha256_hex(body)
//   X-Signature = hmac_sha256_hex(AGENT_SHARED_SECRET, canonical)
//
// The body is always `{"confirm":true}`: the agent rejects requests without
// it, so a stray signed request cannot trigger a power action on its own.

use std::error::Error;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use embedded_svc::http::client::Client as HttpClient;
use embedded_svc::http::Method;
use esp_idf_svc::http::client::{Configuration as HttpConfiguration, EspHttpConnection};
use esp_idf_svc::io::Write;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::config::{AGENT_PORT, AGENT_SHARED_SECRET, PC_IP_ADDRESS};

/// Rejects timestamps from before 2023-11-14, i.e. a clock that never got an
/// NTP sync. The agent verifies the timestamp against its own clock, so a
/// bogus one would be rejected anyway; failing early gives a clearer error.
const MIN_VALID_UNIX_TIME: u64 = 1_700_000_000;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
const BODY: &str = r#"{"confirm":true}"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerAction {
    Reboot,
    Shutdown,
}

impl PowerAction {
    pub fn path(self) -> &'static str {
        match self {
            PowerAction::Reboot => "/reboot",
            PowerAction::Shutdown => "/shutdown",
        }
    }

    pub fn label_ja(self) -> &'static str {
        match self {
            PowerAction::Reboot => "再起動",
            PowerAction::Shutdown => "シャットダウン",
        }
    }

    /// Slug used inside Telegram callback_data. Must stay a plain lowercase
    /// word: the data is parsed by splitting on ':'.
    pub fn slug(self) -> &'static str {
        match self {
            PowerAction::Reboot => "reboot",
            PowerAction::Shutdown => "shutdown",
        }
    }

    pub fn from_slug(slug: &str) -> Option<Self> {
        match slug {
            "reboot" => Some(PowerAction::Reboot),
            "shutdown" => Some(PowerAction::Shutdown),
            _ => None,
        }
    }
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

fn hmac_sha256_hex(secret: &[u8], message: &str) -> Result<String, Box<dyn Error>> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).map_err(|e| e.to_string())?;
    mac.update(message.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

fn unix_now() -> Result<u64, Box<dyn Error>> {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    if secs < MIN_VALID_UNIX_TIME {
        return Err("system clock is not NTP-synced yet".into());
    }
    Ok(secs)
}

/// Nonce for replay protection. Uses the hardware RNG plus the monotonic clock,
/// mirroring the C++ implementation's esp_random() + millis() pair.
fn nonce() -> String {
    let random = unsafe { esp_idf_sys::esp_random() };
    let uptime = unsafe { esp_idf_sys::esp_timer_get_time() };
    format!("{random:x}-{uptime:x}")
}

/// Sends a signed, confirmed power command and returns the HTTP status code.
/// Only 2xx counts as accepted.
pub fn send_command(action: PowerAction) -> Result<u16, Box<dyn Error>> {
    let path = action.path();
    let timestamp = unix_now()?;
    let request_nonce = nonce();

    let canonical = format!(
        "POST\n{path}\n{timestamp}\n{request_nonce}\n{}",
        sha256_hex(BODY.as_bytes())
    );
    let signature = hmac_sha256_hex(AGENT_SHARED_SECRET.as_bytes(), &canonical)?;

    let url = format!("http://{PC_IP_ADDRESS}:{AGENT_PORT}{path}");

    // Keep this short: the caller holds the power lock, which also blocks the
    // touch UI confirm flow and the Telegram task while a request is in flight,
    // so a slow or unreachable agent must fail fast.
    let mut client = HttpClient::wrap(EspHttpConnection::new(&HttpConfiguration {
        timeout: Some(REQUEST_TIMEOUT),
        ..Default::default()
    })?);

    let timestamp_text = timestamp.to_string();
    let content_length = BODY.len().to_string();
    let headers = [
        ("Content-Type", "application/json"),
        ("Content-Length", content_length.as_str()),
        ("X-Timestamp", timestamp_text.as_str()),
        ("X-Nonce", request_nonce.as_str()),
        ("X-Signature", signature.as_str()),
    ];

    let mut request = client.request(Method::Post, &url, &headers)?;
    request.write_all(BODY.as_bytes())?;
    request.flush()?;
    let response = request.submit()?;
    let status = response.status();

    println!("agent {path} -> {status}");
    Ok(status)
}

/// True when the agent accepted the command.
pub fn is_accepted(status: u16) -> bool {
    (200..300).contains(&status)
}
