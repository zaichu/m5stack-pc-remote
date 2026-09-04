use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use m5stack_pc_bridge::{app_config::AgentConfig, audit_log, server::router};
use pc_remote_signing::sign_request;
use serde_json::Value;
use time::OffsetDateTime;
use tower::ServiceExt;

/// audit.log の実体を触るテスト同士を直列化する錠。
/// fail-closed テストでは audit.log の場所へ一時的にディレクトリを置くため、
/// 同じファイルへ追記する他のテストと並行すると互いを壊す。
/// リクエスト跨ぎで保持するため、stdではなくtokioのMutexを使う。
static AUDIT_PATH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn config() -> AgentConfig {
    AgentConfig {
        bind: "127.0.0.1:0".to_string(),
        shared_secret: "local-development-secret".to_string(),
        allowed_skew_seconds: 60,
        dry_run: true,
        telegram_bot_token: None,
        telegram_chat_id: None,
    }
}

fn signed_post(path: &str, nonce: &str, body: &str) -> Request<Body> {
    let timestamp = OffsetDateTime::now_utc().unix_timestamp();
    let secret = b"local-development-secret";
    let signature = sign_request(secret, "POST", path, timestamp, nonce, body.as_bytes());

    Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .header("x-timestamp", timestamp.to_string())
        .header("x-nonce", nonce)
        .header("x-signature", signature)
        .body(Body::from(body.to_owned()))
        .unwrap()
}

#[tokio::test]
async fn status_returns_online_agent_health() {
    let app = router(config());
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 1024).await.unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["agent_online"], true);
    assert_eq!(json["agent"], "m5stack-pc-bridge");
    assert_eq!(json["status"], "ok");
}

#[tokio::test]
async fn reboot_accepts_signed_request_and_stays_dry_run() {
    let _audit_guard = AUDIT_PATH_LOCK.lock().await;
    let app = router(config());
    let response = app
        .oneshot(signed_post(
            "/reboot",
            "server-nonce-1",
            r#"{"confirm":true}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["action"], "reboot");
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["command"][0], "shutdown.exe");
}

#[tokio::test]
async fn reboot_rejects_signed_request_without_confirm_true() {
    let app = router(config());
    let response = app
        .oneshot(signed_post(
            "/reboot",
            "server-nonce-1b",
            r#"{"confirm":false}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = to_bytes(response.into_body(), 1024).await.unwrap();
    assert_eq!(&bytes[..], b"confirm must be true");
}

#[tokio::test]
async fn shutdown_rejects_missing_auth_headers() {
    let app = router(config());
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/shutdown")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn shutdown_rejects_replayed_nonce() {
    let _audit_guard = AUDIT_PATH_LOCK.lock().await;
    let app = router(config());
    let first = app
        .clone()
        .oneshot(signed_post(
            "/shutdown",
            "server-nonce-2",
            r#"{"confirm":true}"#,
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    let second = app
        .oneshot(signed_post(
            "/shutdown",
            "server-nonce-2",
            r#"{"confirm":true}"#,
        ))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::UNAUTHORIZED);
}

/// `/reboot` 用の署名を `/shutdown` へ転用すると401になること。
/// canonical string に PATH が含まれることをエンドポイント越しに守る。
#[tokio::test]
async fn shutdown_rejects_signature_made_for_reboot() {
    let app = router(config());
    let body = r#"{"confirm":true}"#;
    let timestamp = OffsetDateTime::now_utc().unix_timestamp();
    let nonce = "server-nonce-reuse-path";
    let secret = b"local-development-secret";
    let signature = sign_request(secret, "POST", "/reboot", timestamp, nonce, body.as_bytes());

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/shutdown")
                .header("content-type", "application/json")
                .header("x-timestamp", timestamp.to_string())
                .header("x-nonce", nonce)
                .header("x-signature", signature)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// POST 用の署名を `GET /firmware` へ転用すると401になること。
/// 同一パス(`/firmware`)でメソッドだけを変えるため、canonical string に
/// METHOD が含まれることを分離して守る。
#[tokio::test]
async fn firmware_rejects_post_signature_for_get() {
    let app = router(config());
    let timestamp = OffsetDateTime::now_utc().unix_timestamp();
    let nonce = "server-nonce-reuse-method";
    let secret = b"local-development-secret";
    let signature = sign_request(secret, "POST", "/firmware", timestamp, nonce, b"");

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/firmware")
                .header("x-timestamp", timestamp.to_string())
                .header("x-nonce", nonce)
                .header("x-signature", signature)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// `CommandRequest` の `#[serde(deny_unknown_fields)]` を固定する。
/// 未知フィールド付きbodyは400になる。denyを外すとdry_run成功(200)に
/// なってしまい、このテストが落ちる。
#[tokio::test]
async fn reboot_rejects_unknown_json_fields() {
    let app = router(config());
    let response = app
        .oneshot(signed_post(
            "/reboot",
            "server-nonce-unknown-field",
            r#"{"confirm":true,"unexpected":1}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// `DefaultBodyLimit::max(128)` を固定する。128Bを大きく超えるbodyは
/// 413になる。上限を外すとdry_run成功(200)になってしまい落ちる。
#[tokio::test]
async fn reboot_rejects_oversized_body() {
    // JSONとして有効なまま膨らませる(末尾の空白はserde_jsonが無視する)。
    // 未知フィールドを足すとdeny_unknown_fieldsの400と混ざるため使わない。
    let mut body = r#"{"confirm":true}"#.to_string();
    while body.len() <= 256 {
        body.push(' ');
    }
    assert!(body.len() > 128);

    let app = router(config());
    let response = app
        .oneshot(signed_post("/reboot", "server-nonce-oversize", &body))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

/// 電源操作の成功時に監査ログへ追記されることを固定する。
/// サーバ側の事前 `audit_log::append` を削除すると行が増えなくなり落ちる。
#[tokio::test]
async fn reboot_appends_audit_log_on_success() {
    let _audit_guard = AUDIT_PATH_LOCK.lock().await;
    let audit_path = audit_log::path();
    let before = std::fs::read_to_string(&audit_path).unwrap_or_default();

    let app = router(config());
    let response = app
        .oneshot(signed_post(
            "/reboot",
            "server-nonce-audit-append",
            r#"{"confirm":true}"#,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let after = std::fs::read_to_string(&audit_path).unwrap();
    let appended = after.get(before.len()..).unwrap_or(&after);
    assert!(
        appended.contains("action=reboot")
            && appended.contains("dry_run=true")
            && appended.contains("result="),
        "audit log gained no reboot line: {appended:?}"
    );
}

/// fail-closedを固定する: 監査ログを残せないとき電源操作は実行せず500になる。
/// 事前appendの削除やエラーの握り潰しがあると200になってしまい落ちる。
/// 署名は正しいものを使うため、認証を通過したうえでの500であることも守る。
#[tokio::test]
async fn reboot_returns_500_when_audit_log_is_unwritable() {
    let _audit_guard = AUDIT_PATH_LOCK.lock().await;
    let _blocker = AuditPathBlocker::install();

    let app = router(config());
    let response = app
        .oneshot(signed_post(
            "/reboot",
            "server-nonce-audit-fail-closed",
            r#"{"confirm":true}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let bytes = to_bytes(response.into_body(), 1024).await.unwrap();
    assert_eq!(&bytes[..], b"failed to write audit log");
}

/// audit.logの場所をディレクトリで塞ぎ、`append` を必ず失敗させる仕掛け。
/// Dropで元に戻すため、assert失敗時も後続テストを壊さない。
struct AuditPathBlocker {
    path: std::path::PathBuf,
    backup: std::path::PathBuf,
    had_file: bool,
}

impl AuditPathBlocker {
    fn install() -> Self {
        let path = audit_log::path();
        let backup = path.with_extension("log.test-backup");
        // 過去の異常終了で塞ぎっぱなしの場合は掃除する。
        if path.is_dir() {
            std::fs::remove_dir(&path).unwrap();
        }
        // 異常終了で復元されなかった退避があれば先に戻す。
        if !path.exists() && backup.is_file() {
            std::fs::rename(&backup, &path).unwrap();
        }
        let had_file = path.is_file();
        if had_file {
            std::fs::rename(&path, &backup).unwrap();
        }
        std::fs::create_dir(&path).unwrap();
        Self {
            path,
            backup,
            had_file,
        }
    }
}

impl Drop for AuditPathBlocker {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.path);
        if self.had_file {
            let _ = std::fs::rename(&self.backup, &self.path);
        }
    }
}
