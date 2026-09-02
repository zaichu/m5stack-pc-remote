use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use m5stack_pc_bridge::{app_config::AgentConfig, server::router};
use pc_remote_signing::sign_request;
use serde_json::Value;
use time::OffsetDateTime;
use tower::ServiceExt;

fn config() -> AgentConfig {
    AgentConfig {
        bind: "127.0.0.1:0".to_string(),
        shared_secret: "local-development-secret".to_string(),
        allowed_skew_seconds: 60,
        dry_run: true,
    }
}

fn signed_post(path: &str, nonce: &str, body: &'static str) -> Request<Body> {
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
        .body(Body::from(body))
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
