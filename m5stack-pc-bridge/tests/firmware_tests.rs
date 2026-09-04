//! Phase 2のfirmware配信(`GET /firmware/manifest`, `GET /firmware`)のテスト。
//!
//! 実ネットワーク・実機は使わない。`oneshot`(ループバックなし)でrouterを
//! 直接叩き、配信ファイルは一時ディレクトリに置く。秘密情報はダミー値のみ。

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use m5stack_pc_bridge::{
    app_config::AgentConfig,
    firmware::FirmwarePaths,
    server::{router, router_with_firmware_paths},
};
use pc_remote_signing::{body_sha256_hex, sign_manifest, sign_request, verify_manifest_signature};
use serde_json::Value;
use time::OffsetDateTime;
use tower::ServiceExt;

const SECRET: &[u8] = b"local-development-secret";
const FIRMWARE_BYTES: &[u8] = b"fake-firmware-image-for-ota-phase2";
const FIRMWARE_VERSION: &str = "0.2.0";

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

fn setup_files(version: Option<&str>) -> (tempfile::TempDir, FirmwarePaths) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("firmware.bin"), FIRMWARE_BYTES).unwrap();
    if let Some(version) = version {
        std::fs::write(dir.path().join("firmware.version"), version).unwrap();
    }
    let paths = FirmwarePaths {
        bin: dir.path().join("firmware.bin"),
        version: dir.path().join("firmware.version"),
    };
    (dir, paths)
}

fn signed_get(path: &str, nonce: &str) -> Request<Body> {
    let timestamp = OffsetDateTime::now_utc().unix_timestamp();
    let signature = sign_request(SECRET, "GET", path, timestamp, nonce, b"");

    Request::builder()
        .method("GET")
        .uri(path)
        .header("x-timestamp", timestamp.to_string())
        .header("x-nonce", nonce)
        .header("x-signature", signature)
        .body(Body::empty())
        .unwrap()
}

fn unsigned_get(path: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(path)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn manifest_returns_metadata_with_valid_signature() {
    let (_dir, paths) = setup_files(Some(FIRMWARE_VERSION));
    let app = router_with_firmware_paths(config(), paths);

    let response = app
        .oneshot(signed_get("/firmware/manifest", "fw-nonce-1"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap();

    // JSON形状: 要求された5フィールドが揃っていること。
    let version = json["version"].as_str().unwrap();
    let size = json["size"].as_u64().unwrap();
    let sha256 = json["sha256"].as_str().unwrap();
    let created_at = json["created_at"].as_str().unwrap();
    let signature = json["signature"].as_str().unwrap();

    assert_eq!(version, FIRMWARE_VERSION);
    assert_eq!(size, FIRMWARE_BYTES.len() as u64);
    assert_eq!(sha256, body_sha256_hex(FIRMWARE_BYTES));
    assert!(created_at.contains('T'), "created_at must be RFC3339");

    // 署名が期待値と一致すること(共有crateの正本で再計算して比較)。
    let expected = sign_manifest(SECRET, version, size, sha256, created_at);
    assert_eq!(signature, expected);
    assert!(verify_manifest_signature(
        SECRET, version, size, sha256, created_at, signature
    ));
}

#[tokio::test]
async fn manifest_falls_back_to_unknown_version() {
    let (_dir, paths) = setup_files(None);
    let app = router_with_firmware_paths(config(), paths);

    let response = app
        .oneshot(signed_get("/firmware/manifest", "fw-nonce-1b"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["version"], "unknown");
}

#[tokio::test]
async fn firmware_returns_binary_octet_stream() {
    let (_dir, paths) = setup_files(Some(FIRMWARE_VERSION));
    let app = router_with_firmware_paths(config(), paths);

    let response = app
        .oneshot(signed_get("/firmware", "fw-nonce-2"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "application/octet-stream"
    );

    let bytes = to_bytes(response.into_body(), 16 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(&bytes[..], FIRMWARE_BYTES);
}

#[tokio::test]
async fn missing_firmware_returns_404_not_500() {
    let dir = tempfile::tempdir().unwrap();
    let paths = FirmwarePaths {
        bin: dir.path().join("firmware.bin"),
        version: dir.path().join("firmware.version"),
    };

    for (path, nonce) in [
        ("/firmware/manifest", "fw-nonce-3a"),
        ("/firmware", "fw-nonce-3b"),
    ] {
        let app = router_with_firmware_paths(config(), paths.clone());
        let response = app.oneshot(signed_get(path, nonce)).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        let bytes = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(&bytes[..], b"firmware not found", "{path}");
    }
}

#[tokio::test]
async fn unsigned_requests_are_rejected() {
    let (_dir, paths) = setup_files(Some(FIRMWARE_VERSION));

    for path in ["/firmware/manifest", "/firmware"] {
        let app = router_with_firmware_paths(config(), paths.clone());
        let response = app.oneshot(unsigned_get(path)).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
        let bytes = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(&bytes[..], b"unauthorized", "{path}");
    }
}

#[tokio::test]
async fn tampered_signature_is_rejected() {
    let (_dir, paths) = setup_files(Some(FIRMWARE_VERSION));
    let app = router_with_firmware_paths(config(), paths);

    let timestamp = OffsetDateTime::now_utc().unix_timestamp();
    let mut signature = sign_request(
        SECRET,
        "GET",
        "/firmware/manifest",
        timestamp,
        "fw-nonce-4",
        b"",
    );
    // 末尾を書き換えて別人の署名にする。
    //
    // 必ず「元と違う文字」へ置き換える。`push('0')` 固定にすると、署名末尾が
    // たまたま '0' のときに署名が変化せず、正当なリクエストのまま200が返って
    // このテストが1/16で落ちる。署名はtimestamp依存で毎回変わるためflakeになる。
    let last = signature.pop().expect("signature is not empty");
    signature.push(if last == '0' { '1' } else { '0' });

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/firmware/manifest")
                .header("x-timestamp", timestamp.to_string())
                .header("x-nonce", "fw-nonce-4")
                .header("x-signature", signature)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn existing_status_route_is_unchanged() {
    // 既存の `/status` が新エンドポイント追加の影響を受けないこと。
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
}
