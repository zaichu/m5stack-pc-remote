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

const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
const BODY: &str = r#"{"confirm":true}"#;

/// bridgeの `GET /status` 確認の受信タイムアウト(Issue #183)。
///
/// `net::STATUS_PROBE_TIMEOUT` と同じ800ms。`/status` 全体の最悪は
/// PC probe 800ms + bridge 800ms = 1.6秒。参照系なので電源操作の3秒より短くする。
pub const BRIDGE_STATUS_TIMEOUT: Duration = Duration::from_millis(800);

/// bridgeの `GET /status` 応答の受け入れ上限。固定値の小さなJSON(100B未満)の
/// ため十分に余裕がある。ESP32のヒープ保護のため、超過分は読まずに失敗扱いに
/// する(`ota.rs` の `MANIFEST_MAX_BYTES` と同じ考え方)。
pub const BRIDGE_STATUS_MAX_BYTES: usize = 1024;

/// 電源操作の識別子はwire protocolの一部なので `pc-remote-signing` が正本。
/// firmware側は再エクスポートして使う。
pub use pc_remote_signing::PowerAction;

/// 日本語の表示文言。wire protocolではないのでshared crateへは置かない。
/// `PowerAction` は他crateの型で inherent method を足せないため拡張traitにする。
/// 用語は `docs/glossary.md` が正本。再起動・シャットダウンは対象(PC)を必ず明記する。
pub trait PowerActionLabel {
    /// ボタンや「PCを{}しますか？」の確認文に入る短い動作名。
    /// 確認文自体に「PCを」が付くため、ここでは対象を付けない。
    fn label_ja(self) -> &'static str;
    /// 結果文に入る対象付きの操作名。「PCの再起動」「PCのシャットダウン」。
    fn subject_label_ja(self) -> &'static str;
}

impl PowerActionLabel for PowerAction {
    fn label_ja(self) -> &'static str {
        match self {
            PowerAction::Reboot => "再起動",
            PowerAction::Shutdown => "シャットダウン",
        }
    }

    fn subject_label_ja(self) -> &'static str {
        match self {
            PowerAction::Reboot => "PCの再起動",
            PowerAction::Shutdown => "PCのシャットダウン",
        }
    }
}

/// 電源操作の結果文。Telegram応答と本体パネル操作の通知で同じ文言を使うため、
/// ここ1箇所で組み立てる(両箇所にコピーしない)。
pub fn accepted_text(action: PowerAction) -> String {
    format!("{}を受け付けました。", action.subject_label_ja())
}

/// 電源操作の結果文(bridgeが非2xxで拒否)。`accepted_text` と同じく共通化する。
pub fn rejected_text(action: PowerAction, code: u16) -> String {
    format!("{}が拒否されました。({code})", action.subject_label_ja())
}

/// 電源操作の結果文(送信失敗)。`accepted_text` と同じく共通化する。
pub fn failed_text(action: PowerAction) -> String {
    format!("{}に失敗しました。", action.subject_label_ja())
}

/// Unix時刻(秒)。NTP未同期ならエラーにする。OTAの署名付きGETでも使う。
pub(crate) fn unix_now() -> Result<u64, Box<dyn Error>> {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    // NTP同期前の時計で署名しても、bridge側のtimestamp検証で弾かれる。
    // 送信前に止めた方が原因が分かりやすい。
    if !crate::net::is_ntp_synced(secs as i64) {
        return Err("system clock is not NTP-synced yet".into());
    }
    Ok(secs)
}

/// リプレイ防止用nonce。ハードウェア乱数と単調増加時刻を組み合わせる。
/// OTAの署名付きGETでも使う。
pub(crate) fn nonce() -> String {
    let random = unsafe { esp_idf_sys::esp_random() };
    let uptime = unsafe { esp_idf_sys::esp_timer_get_time() };
    format!("{random:x}-{uptime:x}")
}

/// 署名済みの電源操作を送り、HTTPステータスコードを返す。2xxだけを受理扱いにする。
///
/// `pc_ip_address` はTelegram経由で実行時に変更できるため、`AppConfig` ではなく
/// 呼び出し側(`settings::RuntimeSettings`)から都度渡してもらう。
pub fn send_command(
    action: PowerAction,
    config: &AppConfig,
    pc_ip_address: &str,
) -> Result<u16, Box<dyn Error>> {
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

    let url = format!("http://{pc_ip_address}:{}{path}", config.bridge_port);

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

/// bridgeの `GET /status` に接続し、操作サービスが応答しているかを返す(Issue #183)。
///
/// 接続失敗・タイムアウト・2xx以外・上限超過・解釈不能のいずれでも `false`
/// (応答あり扱いにしない)。**PCがオンのときだけ呼ぶこと**(オフなら必ず失敗し待つだけ)。
pub fn check_bridge_online(config: &AppConfig, pc_ip_address: &str) -> bool {
    let url = format!(
        "http://{pc_ip_address}:{}{}",
        config.bridge_port,
        pc_remote_signing::BRIDGE_STATUS_PATH
    );

    let result: Result<bool, Box<dyn Error>> = (|| {
        let mut client = HttpClient::wrap(EspHttpConnection::new(&HttpConfiguration {
            timeout: Some(BRIDGE_STATUS_TIMEOUT),
            ..Default::default()
        })?);
        let request = client.request(Method::Get, &url, &[])?;
        let mut response = request.submit()?;
        if !is_accepted(response.status()) {
            return Ok(false);
        }

        // 上限を超える本文は読まずに失敗扱いにする(ESP32のヒープ保護。
        // `ota.rs` の `MANIFEST_MAX_BYTES` と同じ考え方)。
        let mut body = Vec::new();
        let mut chunk = [0u8; 256];
        loop {
            let read = response.read(&mut chunk)?;
            if read == 0 {
                break;
            }
            if body.len() + read > BRIDGE_STATUS_MAX_BYTES {
                return Ok(false);
            }
            body.extend_from_slice(&chunk[..read]);
        }
        Ok(pc_remote_signing::bridge_status_online(&body))
    })();

    match result {
        Ok(online) => online,
        Err(e) => {
            // URL(IP・ポート)は出さない。失敗の事実だけ残す。
            println!("bridge status check failed: {e}");
            false
        }
    }
}
