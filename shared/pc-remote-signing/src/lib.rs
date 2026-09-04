//! M5Stack firmwareとm5stack-pc-bridgeが共有する実装。
//!
//! 中心はHMAC署名のwire protocolだが、「両側で同一でなければ壊れる」ものは
//! 電源操作の識別子(`PowerAction`)やアラート抑制ポリシー(`AlertThrottle`)も
//! ここへ置く。
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

/// m5stack-pc-bridgeが配信するfirmware manifestの署名。
///
/// bridgeは「配布場所」であって「信頼の根」ではないため、manifestへ
/// HMAC-SHA256署名を付ける。M5Stack側(Phase 3のOTAクライアント)は
/// 公開値(version/size/sha256)だけを信じず、この署名を検証してから
/// ダウンロードへ進む。
///
/// canonical文字列は次の形式で固定する。リクエスト署名(`canonical_string`)
/// の流用ではなくmanifest専用の文字列にする。理由はドメイン分離のため:
/// 先頭の `FIRMWARE-MANIFEST-v1` が無いと、manifest署名が何らかのリクエスト
/// 署名と一致し得て、署名の使い回し(クロスプロトコル confusion)の余地が
/// 残る。鍵とアルゴリズム(HMAC-SHA256 + shared_secret + hex)は
/// リクエスト署名と同じで、新しい署名方式は発明しない。
///
/// ```text
/// "FIRMWARE-MANIFEST-v1" + "\n" + VERSION + "\n" + SIZE + "\n" + SHA256_HEX + "\n" + CREATED_AT
/// Manifest-Signature = hmac_sha256_hex(shared_secret, canonical)
/// ```
///
/// `SIZE` は10進のバイト数、`SHA256_HEX` は小文字hex、`CREATED_AT` はRFC3339。
/// フィールド順序はこの関数が一意に決める。呼び出し側で順序を組み立てないこと。
pub fn manifest_canonical_string(
    version: &str,
    size: u64,
    sha256_hex: &str,
    created_at: &str,
) -> String {
    format!("FIRMWARE-MANIFEST-v1\n{version}\n{size}\n{sha256_hex}\n{created_at}")
}

/// m5stack-pc-bridge側(署名する側)が使う。
pub fn sign_manifest(
    secret: &[u8],
    version: &str,
    size: u64,
    sha256_hex: &str,
    created_at: &str,
) -> String {
    let canonical = manifest_canonical_string(version, size, sha256_hex, created_at);
    let mut mac =
        HmacSha256::new_from_slice(secret).expect("HMAC accepts keys of any size for SHA-256");
    mac.update(canonical.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// M5Stack firmware側(検証する側)が使う。Phase 3のOTAクライアントが呼ぶ。
pub fn verify_manifest_signature(
    secret: &[u8],
    version: &str,
    size: u64,
    sha256_hex: &str,
    created_at: &str,
    signature_hex: &str,
) -> bool {
    let Ok(expected) = hex::decode(signature_hex) else {
        return false;
    };
    let canonical = manifest_canonical_string(version, size, sha256_hex, created_at);
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

/// 認証・認可の失敗が続いたときに通知を出すかどうかを決める抑制ロジック。
///
/// firmware(Telegramの未許可ユーザー)とbridge(HTTP認証失敗)で同じポリシーを使う。
/// 「閾値回たまったら発火し、発火後は一定時間鳴らさない」。1回目から鳴らすと、
/// 無関係なbot巡回や時計ずれによる単発の失敗でも通知が飛んでしまう。
///
/// 現在時刻を引数で受け取るため、待たずにテストできる。
pub struct AlertThrottle {
    threshold: u32,
    interval: std::time::Duration,
    failures: u32,
    last_fired: Option<std::time::Instant>,
}

impl AlertThrottle {
    /// 何回たまったら発火するか。
    pub const DEFAULT_THRESHOLD: u32 = 3;
    /// 発火後、次に鳴らせるようになるまでの時間。スキャンや連投で通知が
    /// 埋まらないようにする。
    pub const DEFAULT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3600);

    pub fn new(threshold: u32, interval: std::time::Duration) -> Self {
        Self {
            threshold,
            interval,
            failures: 0,
            last_fired: None,
        }
    }

    /// 失敗を1件記録する。通知すべきならたまっていた件数を返し、カウンタを戻す。
    pub fn record(&mut self, now: std::time::Instant) -> Option<u32> {
        self.failures += 1;
        if self.failures < self.threshold {
            return None;
        }
        if let Some(fired) = self.last_fired {
            if now.duration_since(fired) < self.interval {
                return None;
            }
        }

        let count = self.failures;
        self.failures = 0;
        self.last_fired = Some(now);
        Some(count)
    }
}

impl Default for AlertThrottle {
    fn default() -> Self {
        Self::new(Self::DEFAULT_THRESHOLD, Self::DEFAULT_INTERVAL)
    }
}

#[cfg(test)]
mod manifest_tests {
    use super::{manifest_canonical_string, sign_manifest, verify_manifest_signature};

    const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";

    #[test]
    fn canonical_string_pins_field_order() {
        assert_eq!(
            manifest_canonical_string("0.2.0", 1234, "abcd", "2026-09-04T00:00:00Z",),
            "FIRMWARE-MANIFEST-v1\n0.2.0\n1234\nabcd\n2026-09-04T00:00:00Z",
        );
    }

    #[test]
    fn sign_and_verify_round_trip() {
        let signature = sign_manifest(SECRET, "0.2.0", 42, "abcd", "2026-09-04T00:00:00Z");
        assert!(verify_manifest_signature(
            SECRET,
            "0.2.0",
            42,
            "abcd",
            "2026-09-04T00:00:00Z",
            &signature,
        ));
    }

    #[test]
    fn rejects_tampered_field_or_wrong_secret() {
        let signature = sign_manifest(SECRET, "0.2.0", 42, "abcd", "2026-09-04T00:00:00Z");
        // 各フィールドのどれを変えても検証が通らないこと。
        assert!(!verify_manifest_signature(
            SECRET,
            "0.2.1",
            42,
            "abcd",
            "2026-09-04T00:00:00Z",
            &signature,
        ));
        assert!(!verify_manifest_signature(
            SECRET,
            "0.2.0",
            43,
            "abcd",
            "2026-09-04T00:00:00Z",
            &signature,
        ));
        assert!(!verify_manifest_signature(
            SECRET,
            "0.2.0",
            42,
            "abce",
            "2026-09-04T00:00:00Z",
            &signature,
        ));
        assert!(!verify_manifest_signature(
            SECRET,
            "0.2.0",
            42,
            "abcd",
            "2026-09-04T00:00:01Z",
            &signature,
        ));
        assert!(!verify_manifest_signature(
            b"wrong-secret-wrong-secret-wrong12",
            "0.2.0",
            42,
            "abcd",
            "2026-09-04T00:00:00Z",
            &signature,
        ));
        assert!(!verify_manifest_signature(
            SECRET,
            "0.2.0",
            42,
            "abcd",
            "2026-09-04T00:00:00Z",
            "not-hex",
        ));
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

#[cfg(test)]
mod alert_throttle_tests {
    use super::AlertThrottle;
    use std::time::{Duration, Instant};

    #[test]
    fn fires_at_threshold_and_resets_counter() {
        let mut throttle = AlertThrottle::default();
        let now = Instant::now();

        assert_eq!(throttle.record(now), None);
        assert_eq!(throttle.record(now), None);
        assert_eq!(throttle.record(now), Some(AlertThrottle::DEFAULT_THRESHOLD));
    }

    #[test]
    fn stays_quiet_until_the_interval_has_passed() {
        let interval = Duration::from_secs(3600);
        let mut throttle = AlertThrottle::new(3, interval);
        let start = Instant::now();

        for _ in 0..2 {
            assert_eq!(throttle.record(start), None);
        }
        assert_eq!(throttle.record(start), Some(3));

        // 間隔内は閾値へ再到達しても鳴らさない。
        for _ in 0..6 {
            assert_eq!(
                throttle.record(start + interval - Duration::from_secs(1)),
                None
            );
        }
        // 間隔を過ぎれば、たまっていた件数をまとめて返す。
        assert_eq!(throttle.record(start + interval), Some(7));
    }
}
