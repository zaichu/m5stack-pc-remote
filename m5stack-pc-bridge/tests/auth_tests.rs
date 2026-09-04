use m5stack_pc_bridge::auth::{verify_request, AuthConfig, AuthError, NonceStore};
use pc_remote_signing::sign_request;
use serde_json::json;
use time::OffsetDateTime;

fn config() -> AuthConfig {
    AuthConfig {
        secret: b"0123456789abcdef0123456789abcdef".to_vec(),
        allowed_skew_seconds: 60,
    }
}

fn config_with_skew(allowed_skew_seconds: i64) -> AuthConfig {
    AuthConfig {
        secret: b"0123456789abcdef0123456789abcdef".to_vec(),
        allowed_skew_seconds,
    }
}

#[test]
fn accepts_valid_signature_once() {
    let cfg = config();
    let store = NonceStore::default();
    let now = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
    let body = json!({"action":"reboot"}).to_string();
    let signature = sign_request(
        &cfg.secret,
        "POST",
        "/reboot",
        now.unix_timestamp(),
        "nonce-1",
        body.as_bytes(),
    );

    verify_request(
        &cfg,
        &store,
        "POST",
        "/reboot",
        now.unix_timestamp(),
        "nonce-1",
        body.as_bytes(),
        &signature,
        now,
    )
    .unwrap();
}

#[test]
fn rejects_replayed_nonce() {
    let cfg = config();
    let store = NonceStore::default();
    let now = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
    let body = b"{}";
    let signature = sign_request(
        &cfg.secret,
        "POST",
        "/shutdown",
        now.unix_timestamp(),
        "same-nonce",
        body,
    );

    verify_request(
        &cfg,
        &store,
        "POST",
        "/shutdown",
        now.unix_timestamp(),
        "same-nonce",
        body,
        &signature,
        now,
    )
    .unwrap();

    let err = verify_request(
        &cfg,
        &store,
        "POST",
        "/shutdown",
        now.unix_timestamp(),
        "same-nonce",
        body,
        &signature,
        now,
    )
    .unwrap_err();

    assert_eq!(err, AuthError::Replay);
}

#[test]
fn rejects_replayed_nonce_for_entire_allowed_skew_window() {
    let cfg = config_with_skew(900);
    let store = NonceStore::default();
    let first_seen = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
    let replay_seen = first_seen + time::Duration::seconds(700);
    let body = br#"{"confirm":true}"#;
    let signature = sign_request(
        &cfg.secret,
        "POST",
        "/shutdown",
        first_seen.unix_timestamp(),
        "long-skew-nonce",
        body,
    );

    verify_request(
        &cfg,
        &store,
        "POST",
        "/shutdown",
        first_seen.unix_timestamp(),
        "long-skew-nonce",
        body,
        &signature,
        first_seen,
    )
    .unwrap();

    let err = verify_request(
        &cfg,
        &store,
        "POST",
        "/shutdown",
        first_seen.unix_timestamp(),
        "long-skew-nonce",
        body,
        &signature,
        replay_seen,
    )
    .unwrap_err();

    assert_eq!(err, AuthError::Replay);
}

#[test]
fn rejects_expired_timestamp() {
    let cfg = config();
    let store = NonceStore::default();
    let now = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
    let timestamp = now.unix_timestamp() - 120;
    let body = b"{}";
    let signature = sign_request(&cfg.secret, "POST", "/reboot", timestamp, "nonce-2", body);

    let err = verify_request(
        &cfg, &store, "POST", "/reboot", timestamp, "nonce-2", body, &signature, now,
    )
    .unwrap_err();

    assert_eq!(err, AuthError::TimestampSkew);
}

#[test]
fn rejects_tampered_body() {
    let cfg = config();
    let store = NonceStore::default();
    let now = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
    let signature = sign_request(
        &cfg.secret,
        "POST",
        "/shutdown",
        now.unix_timestamp(),
        "nonce-3",
        br#"{"confirm":true}"#,
    );

    let err = verify_request(
        &cfg,
        &store,
        "POST",
        "/shutdown",
        now.unix_timestamp(),
        "nonce-3",
        br#"{"confirm":false}"#,
        &signature,
        now,
    )
    .unwrap_err();

    assert_eq!(err, AuthError::BadSignature);
}

#[test]
fn rejects_signature_made_for_another_path() {
    // `/reboot` 用の署名を `/shutdown` の検証へ渡すと BadSignature になる。
    let cfg = config();
    let store = NonceStore::default();
    let now = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
    let body = br#"{"confirm":true}"#;
    let signature = sign_request(
        &cfg.secret,
        "POST",
        "/reboot",
        now.unix_timestamp(),
        "nonce-path-reuse",
        body,
    );

    let err = verify_request(
        &cfg,
        &store,
        "POST",
        "/shutdown",
        now.unix_timestamp(),
        "nonce-path-reuse",
        body,
        &signature,
        now,
    )
    .unwrap_err();

    assert_eq!(err, AuthError::BadSignature);
}

#[test]
fn rejects_signature_made_for_another_method() {
    // POST 用の署名を `GET /firmware` の検証へ渡すと BadSignature になる。
    let cfg = config();
    let store = NonceStore::default();
    let now = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
    let body = b"";
    let signature = sign_request(
        &cfg.secret,
        "POST",
        "/firmware",
        now.unix_timestamp(),
        "nonce-method-reuse",
        body,
    );

    let err = verify_request(
        &cfg,
        &store,
        "GET",
        "/firmware",
        now.unix_timestamp(),
        "nonce-method-reuse",
        body,
        &signature,
        now,
    )
    .unwrap_err();

    assert_eq!(err, AuthError::BadSignature);
}
