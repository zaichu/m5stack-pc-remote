//! M5Stack firmware(署名側)とm5stack-pc-bridge(検証側)が共有するHMAC署名wire protocol。
//!
//! canonical文字列は次の形式で固定する。片方だけを変更すると署名が一致しなくなるため、
//! 実装を1箇所にまとめてある。
//!
//! ```text
//! METHOD + "\n" + PATH + "\n" + TIMESTAMP + "\n" + NONCE + "\n" + SHA256(BODY)
//! X-Signature = hmac_sha256_hex(shared_secret, canonical)
//! ```

use hmac::{Hmac, KeyInit, Mac};
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

/// M5Stack firmware側(署名する側)が使う。
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

/// m5stack-pc-bridge側(検証する側)が使う。
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

/// firmware(署名側)とm5stack-pc-bridge(検証側)で共有する電源操作の識別子。
///
/// HTTPパスとslugの対応をここ1箇所で決める。canonical文字列にはPATHが入るため、
/// 片方だけを変えると署名が一致しなくなる。表示文言はwire protocolの一部では
/// ないので、ここには置かない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerAction {
    Reboot,
    Shutdown,
}

impl PowerAction {
    pub const ALL: [PowerAction; 2] = [PowerAction::Reboot, PowerAction::Shutdown];

    /// Telegram callback_dataや監査ログで使う識別子。`:` 区切りで解析するため
    /// 小文字の単語にする。
    pub fn slug(self) -> &'static str {
        match self {
            PowerAction::Reboot => "reboot",
            PowerAction::Shutdown => "shutdown",
        }
    }

    /// 署名対象になるHTTPパス。slugへ `/` を付けたものと必ず一致する。
    pub fn path(self) -> &'static str {
        match self {
            PowerAction::Reboot => "/reboot",
            PowerAction::Shutdown => "/shutdown",
        }
    }

    pub fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.slug() == slug)
    }

    /// HTTPパスからの逆引き。bridge側のrouting確認に使える。
    pub fn from_path(path: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.path() == path)
    }
}

#[cfg(test)]
mod power_action_tests {
    use super::PowerAction;

    #[test]
    fn path_is_slug_prefixed_with_slash() {
        for action in PowerAction::ALL {
            assert_eq!(action.path(), format!("/{}", action.slug()));
        }
    }

    #[test]
    fn slug_and_path_round_trip() {
        for action in PowerAction::ALL {
            assert_eq!(PowerAction::from_slug(action.slug()), Some(action));
            assert_eq!(PowerAction::from_path(action.path()), Some(action));
        }
        assert_eq!(PowerAction::from_slug("hibernate"), None);
        assert_eq!(PowerAction::from_path("/reboot/"), None);
    }
}
