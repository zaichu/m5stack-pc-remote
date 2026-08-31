use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

pub fn body_sha256_hex(body: &[u8]) -> String {
    hex::encode(Sha256::digest(body))
}

pub fn canonical_string(
    method: &str,
    path: &str,
    timestamp: i64,
    nonce: &str,
    body: &[u8],
) -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}",
        method.to_ascii_uppercase(),
        path,
        timestamp,
        nonce,
        body_sha256_hex(body)
    )
}

pub fn sign_request(
    secret: &[u8],
    method: &str,
    path: &str,
    timestamp: i64,
    nonce: &str,
    body: &[u8],
) -> String {
    let canonical = canonical_string(method, path, timestamp, nonce, body);
    let mut mac =
        HmacSha256::new_from_slice(secret).expect("HMAC accepts keys of any size for SHA-256");
    mac.update(canonical.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

pub fn verify_signature(
    secret: &[u8],
    method: &str,
    path: &str,
    timestamp: i64,
    nonce: &str,
    body: &[u8],
    signature_hex: &str,
) -> bool {
    let Ok(expected) = hex::decode(signature_hex) else {
        return false;
    };
    let canonical = canonical_string(method, path, timestamp, nonce, body);
    let Ok(mut mac) = HmacSha256::new_from_slice(secret) else {
        return false;
    };
    mac.update(canonical.as_bytes());
    mac.verify_slice(&expected).is_ok()
}
