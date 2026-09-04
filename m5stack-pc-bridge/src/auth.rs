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
    const MAX_NONCE_LEN: usize = 64;
    const MAX_ENTRIES: usize = 10_000;

    pub fn insert_once(
        &self,
        nonce: &str,
        timestamp: i64,
        now: OffsetDateTime,
        ttl_seconds: i64,
    ) -> bool {
        // 毎リクエストの全走査 O(N) を避けるため、100件ごとに間引き
        if self.seen.len().is_multiple_of(100) {
            self.evict_expired(now.unix_timestamp(), ttl_seconds);
        }
        if nonce.len() > Self::MAX_NONCE_LEN
            || !nonce
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
            || self.seen.len() >= Self::MAX_ENTRIES
        {
            return false;
        }
        self.seen.insert(nonce.to_owned(), timestamp).is_none()
    }

    fn evict_expired(&self, now: i64, ttl_seconds: i64) {
        // i64::abs_diff はu64を返しオーバーフローし得ないため使う。
        // 以前は `saturating_sub(...).abs()` だったが、timestampに極端な値
        // (例: i64::MAX)が入ると差がi64::MINへ飽和し、release buildでは
        // `.abs()` がオーバーフローチェック無効でi64::MIN(負値)のまま返り、
        // 「期限切れ」判定が意図せず反転する余地があった。
        let ttl_seconds = ttl_seconds.max(0) as u64;
        self.seen
            .retain(|_, timestamp| now.abs_diff(*timestamp) <= ttl_seconds);
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
    // i64::abs_diff はu64を返しオーバーフローし得ない。以前の
    // `saturating_sub(...).abs()` は、攻撃者が送るX-Timestampへ極端な値
    // (例: i64::MAX)を入れると差がi64::MINへ飽和し、release buildでは
    // `.abs()` がオーバーフローチェック無効でi64::MIN(負値)のまま返るため、
    // `skew > allowed_skew_seconds` が常に偽になりチェックを迂回できた。
    let skew = now.unix_timestamp().abs_diff(timestamp);
    if skew > config.allowed_skew_seconds.max(0) as u64 {
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
