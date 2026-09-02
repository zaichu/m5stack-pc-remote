use dashmap::DashMap;
use thiserror::Error;
use time::OffsetDateTime;

// `secret` を含むため `Debug` は手書きし、値を出力しない。
// ログや panic メッセージへ `{:?}` で secret が漏れる事故を防ぐ。
#[derive(Clone)]
pub struct AuthConfig {
    pub secret: Vec<u8>,
    pub allowed_skew_seconds: i64,
}

impl std::fmt::Debug for AuthConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthConfig")
            .field("secret", &"[REDACTED]")
            .field("allowed_skew_seconds", &self.allowed_skew_seconds)
            .finish()
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AuthError {
    #[error("timestamp is outside the allowed clock skew")]
    TimestampSkew,
    #[error("nonce has already been used")]
    Replay,
    #[error("bad request signature")]
    BadSignature,
    #[error("missing authentication header")]
    MissingHeader,
}

#[derive(Debug, Default)]
pub struct NonceStore {
    seen: DashMap<String, i64>,
}

impl NonceStore {
    pub fn insert_once(
        &self,
        nonce: &str,
        timestamp: i64,
        now: OffsetDateTime,
        ttl_seconds: i64,
    ) -> bool {
        self.evict_expired(now.unix_timestamp(), ttl_seconds);
        self.seen.insert(nonce.to_owned(), timestamp).is_none()
    }

    fn evict_expired(&self, now: i64, ttl_seconds: i64) {
        self.seen
            .retain(|_, timestamp| now.saturating_sub(*timestamp).abs() <= ttl_seconds);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn verify_request(
    config: &AuthConfig,
    nonces: &NonceStore,
    method: &str,
    path: &str,
    timestamp: i64,
    nonce: &str,
    body: &[u8],
    signature_hex: &str,
    now: OffsetDateTime,
) -> Result<(), AuthError> {
    let skew = now.unix_timestamp().saturating_sub(timestamp).abs();
    if skew > config.allowed_skew_seconds {
        return Err(AuthError::TimestampSkew);
    }

    if !pc_remote_signing::verify_signature(
        &config.secret,
        method,
        path,
        timestamp,
        nonce,
        body,
        signature_hex,
    ) {
        return Err(AuthError::BadSignature);
    }

    if !nonces.insert_once(nonce, timestamp, now, config.allowed_skew_seconds) {
        return Err(AuthError::Replay);
    }

    Ok(())
}
