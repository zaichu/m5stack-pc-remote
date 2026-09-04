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

// ===== Issue #136: 検証順序の固定 =====
//
// `verify_request` は timestamp → 署名 → nonce の順で検証する。
// この順序を入れ替える変異 (例: nonce → 署名 → timestamp) を入れても
// 既存テストは全て緑のままだったため、複合的に不正な入力で
// 「どのエラーが返るか」を固定する。

/// 期限切れtimestamp + 不正署名 → TimestampSkew が返ること。
/// 署名検証を先に持ってくる変異を入れると BadSignature になって落ちる。
#[test]
fn expired_timestamp_with_bad_signature_reports_timestamp_skew_first() {
    let cfg = config();
    let store = NonceStore::default();
    let now = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
    let timestamp = now.unix_timestamp() - 120;
    let body = b"{}";
    // 正しい鍵ではない鍵で署名し、署名自体も不正にする。
    let bad_signature = sign_request(
        b"ffffffffffffffffffffffffffffffff",
        "POST",
        "/reboot",
        timestamp,
        "order-nonce-1",
        body,
    );

    let err = verify_request(
        &cfg,
        &store,
        "POST",
        "/reboot",
        timestamp,
        "order-nonce-1",
        body,
        &bad_signature,
        now,
    )
    .unwrap_err();

    assert_eq!(err, AuthError::TimestampSkew);
}

/// 期限切れtimestamp + 正しい署名 + 使用済みnonce → TimestampSkew が返ること。
/// nonce検査を先に持ってくる変異を入れると Replay になって落ちる。
#[test]
fn expired_timestamp_with_reused_nonce_reports_timestamp_skew_first() {
    let cfg = config();
    let store = NonceStore::default();
    let first = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
    let body = b"{}";
    let signature = sign_request(
        &cfg.secret,
        "POST",
        "/reboot",
        first.unix_timestamp(),
        "order-nonce-2",
        body,
    );

    verify_request(
        &cfg,
        &store,
        "POST",
        "/reboot",
        first.unix_timestamp(),
        "order-nonce-2",
        body,
        &signature,
        first,
    )
    .unwrap();

    // skew の外へ時計を進めて同じリクエストを再送する。
    // 署名は正しく nonce は使用済みだが、期限切れが優先されること。
    let later = first + time::Duration::seconds(120);
    let err = verify_request(
        &cfg,
        &store,
        "POST",
        "/reboot",
        first.unix_timestamp(),
        "order-nonce-2",
        body,
        &signature,
        later,
    )
    .unwrap_err();

    assert_eq!(err, AuthError::TimestampSkew);
}

/// 不正署名のリクエストは nonce を消費しないこと。
/// 署名検証の前に nonce を登録する変異を入れると、
/// 2回目の正しい再送が Replay になって落ちる。
#[test]
fn failed_signature_does_not_consume_nonce() {
    let cfg = config();
    let store = NonceStore::default();
    let now = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
    let body = b"{}";
    let good_signature = sign_request(
        &cfg.secret,
        "POST",
        "/reboot",
        now.unix_timestamp(),
        "order-nonce-3",
        body,
    );
    let bad_signature = sign_request(
        b"ffffffffffffffffffffffffffffffff",
        "POST",
        "/reboot",
        now.unix_timestamp(),
        "order-nonce-3",
        body,
    );

    let err = verify_request(
        &cfg,
        &store,
        "POST",
        "/reboot",
        now.unix_timestamp(),
        "order-nonce-3",
        body,
        &bad_signature,
        now,
    )
    .unwrap_err();
    assert_eq!(err, AuthError::BadSignature);

    // 同じ nonce での正しい再送は受け付けられること。
    verify_request(
        &cfg,
        &store,
        "POST",
        "/reboot",
        now.unix_timestamp(),
        "order-nonce-3",
        body,
        &good_signature,
        now,
    )
    .unwrap();
}

// ===== Issue #136: timestamp 境界の固定 =====
//
// allowed_skew_seconds = 60 に対し、skew ちょうどは受け入れ、
// skew+1 は過去・未来の両方向で拒否することを固定する。
// `>` を `>=` に変える変異はちょうどの受け入れで落ち、
// `abs_diff` を `saturating_sub` (過去のみ検査) に変える変異は
// 未来方向の拒否で落ちる。

/// skew ちょうど (過去 now-60 / 未来 now+60) は受け入れる。
#[test]
fn accepts_timestamp_exactly_at_skew_boundary() {
    let cfg = config();
    let store = NonceStore::default();
    let now = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
    let body = b"{}";

    for (timestamp, nonce) in [
        (now.unix_timestamp() - 60, "skew-past-exact"),
        (now.unix_timestamp() + 60, "skew-future-exact"),
    ] {
        let signature = sign_request(&cfg.secret, "POST", "/reboot", timestamp, nonce, body);
        verify_request(
            &cfg, &store, "POST", "/reboot", timestamp, nonce, body, &signature, now,
        )
        .unwrap();
    }
}

/// skew+1 (過去 now-61 / 未来 now+61) は拒否する。
#[test]
fn rejects_timestamp_one_second_beyond_skew_in_both_directions() {
    let cfg = config();
    let store = NonceStore::default();
    let now = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
    let body = b"{}";

    for (timestamp, nonce) in [
        (now.unix_timestamp() - 61, "skew-past-over"),
        (now.unix_timestamp() + 61, "skew-future-over"),
    ] {
        let signature = sign_request(&cfg.secret, "POST", "/reboot", timestamp, nonce, body);
        let err = verify_request(
            &cfg, &store, "POST", "/reboot", timestamp, nonce, body, &signature, now,
        )
        .unwrap_err();
        assert_eq!(err, AuthError::TimestampSkew, "timestamp={timestamp}");
    }
}

// ===== Issue #136: nonce 入力検証の verify_request 側の写像 =====
//
// `insert_once` の長さ・文字種チェックを外す変異は
// `nonce_eviction_tests.rs` 側の直接テストで落とす。
// ここでは verify_request 経由で不正 nonce が Replay として
// 拒否されること (署名は正しい状態で) を固定する。

/// 長すぎる nonce は verify_request で拒否されること。
#[test]
fn rejects_overlong_nonce_at_verify_request() {
    let cfg = config();
    let store = NonceStore::default();
    let now = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
    let body = b"{}";
    // MAX_NONCE_LEN(=64) を1文字超える。
    let nonce = "n".repeat(65);
    let signature = sign_request(
        &cfg.secret,
        "POST",
        "/reboot",
        now.unix_timestamp(),
        &nonce,
        body,
    );

    let err = verify_request(
        &cfg,
        &store,
        "POST",
        "/reboot",
        now.unix_timestamp(),
        &nonce,
        body,
        &signature,
        now,
    )
    .unwrap_err();

    assert_eq!(err, AuthError::Replay);
}
