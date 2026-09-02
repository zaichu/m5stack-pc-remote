// m5stack-pc-bridgeへ送るHMAC署名付き電源操作(REBOOT / SHUTDOWN)。
//
// 署名のcanonical文字列とHMAC計算は `pc-remote-signing` (shared/) に実装があり、
// m5stack-pc-bridge側の検証処理と同じ実装を使う。本文は常に `{"confirm":true}`。
// m5stack-pc-bridgeもこれを必須にしているため、署名だけでは電源操作を実行できない。

use std::error::Error;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use embedded_svc::http::client::Client as HttpClient;
use embedded_svc::http::Method;
use esp_idf_svc::http::client::{Configuration as HttpConfiguration, EspHttpConnection};
use esp_idf_svc::io::Write;

use crate::app_config::AppConfig;

/// 2023-11-14より古い時刻は、NTP同期前の時計として扱って拒否する。
/// m5stack-pc-bridgeでもtimestampを検証するが、送信前に止めた方が原因が分かりやすい。
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

    /// Telegram callback_data内で使う識別子。`:` 区切りで解析するため小文字の単語にする。
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

fn unix_now() -> Result<u64, Box<dyn Error>> {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    if secs < MIN_VALID_UNIX_TIME {
        return Err("system clock is not NTP-synced yet".into());
    }
    Ok(secs)
}

/// リプレイ防止用nonce。ハードウェア乱数と単調増加時刻を組み合わせる。
fn nonce() -> String {
    let random = unsafe { esp_idf_sys::esp_random() };
    let uptime = unsafe { esp_idf_sys::esp_timer_get_time() };
    format!("{random:x}-{uptime:x}")
}

/// 署名済みの電源操作を送り、HTTPステータスコードを返す。2xxだけを受理扱いにする。
pub fn send_command(action: PowerAction, config: &AppConfig) -> Result<u16, Box<dyn Error>> {
    let path = action.path();
    let timestamp = unix_now()?;
    let request_nonce = nonce();

    let signature = pc_remote_signing::sign_request(
        config.bridge_shared_secret.as_bytes(),
        "POST",
        path,
        timestamp as i64,
        &request_nonce,
        BODY.as_bytes(),
    );

    let url = format!(
        "http://{}:{}{path}",
        config.pc_ip_address, config.bridge_port
    );

    // 呼び出し元は電源操作ロックを保持している。m5stack-pc-bridge応答待ちでUIやTelegram処理を
    // 長く止めないよう、短いtimeoutで失敗させる。
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

    println!("bridge {path} -> {status}");
    Ok(status)
}

/// m5stack-pc-bridgeがコマンドを受理した場合にtrue。
pub fn is_accepted(status: u16) -> bool {
    (200..300).contains(&status)
}
