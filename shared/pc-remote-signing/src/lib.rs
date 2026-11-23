//! M5Stack firmwareとm5stack-pc-bridgeが共有する実装。
//!
//! 「両側で同一でなければ壊れる」ものをここへ置く。canonical文字列は片方だけ
//! 変えると署名が一致しなくなるため、実装を1箇所にまとめる。
//!
//! ```text
//! METHOD + "\n" + PATH + "\n" + TIMESTAMP + "\n" + NONCE + "\n" + SHA256(BODY)
//! X-Signature = hmac_sha256_hex(shared_secret, canonical)
//! ```

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;
pub type SigningError = hmac::digest::InvalidLength;

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
) -> Result<String, SigningError> {
    let canonical = canonical_string(method, path, timestamp, nonce, body);
    let mut mac = HmacSha256::new_from_slice(secret)?;
    mac.update(canonical.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
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
/// 先頭の `FIRMWARE-MANIFEST-v1` はドメイン分離(無いとmanifest署名がリクエスト署名と
/// 一致し得て、署名の使い回し=クロスプロトコルconfusionを許す)。
///
/// ```text
/// "FIRMWARE-MANIFEST-v1" + "\n" + VERSION + "\n" + SIZE + "\n" + SHA256_HEX + "\n" + CREATED_AT
/// Manifest-Signature = hmac_sha256_hex(shared_secret, canonical)
/// ```
///
/// `SIZE` は10進、`SHA256_HEX` は小文字hex、`CREATED_AT` はRFC3339。
/// 順序はこの関数が決める。呼び出し側で組み立てないこと。
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
) -> Result<String, SigningError> {
    let canonical = manifest_canonical_string(version, size, sha256_hex, created_at);
    let mut mac = HmacSha256::new_from_slice(secret)?;
    mac.update(canonical.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

/// M5Stack firmware側(検証する側)が使う。
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

/// OTA配信のmanifest(`GET /firmware/manifest` の応答)。構造体は1箇所に置く
/// (片方だけfieldを増減すると署名検証以前にparseで壊れる)。
/// 未知fieldは許容する(将来fieldを足しても古いfirmwareのOTAが止まらないように)。
/// 応答側(m5stack-pc-bridge)もこの型を `Serialize` で返し、field宣言を複製しない。
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct OtaManifest {
    pub version: String,
    pub size: u64,
    pub sha256: String,
    pub created_at: String,
    pub signature: String,
}

/// manifestのparse・検証・突き合わせの失敗。
/// エラー文言にmanifest本文やsecretを含めない(本文は署名検証前の未検証データ)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OtaManifestError {
    /// JSONとして読めない、fieldが欠けている、型が違う。
    InvalidJson(String),
    /// JSONとしては読めたが、fieldの値が配信物としてあり得ない。
    /// 中身はfield名(`version` / `size` / `sha256` / `created_at` / `signature`)。
    InvalidField(&'static str),
    /// HMAC署名が一致しない。ダウンロードへ進まず中止すること。
    SignatureMismatch,
}

impl std::fmt::Display for OtaManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidJson(reason) => {
                write!(f, "firmware manifest is not valid JSON: {reason}")
            }
            Self::InvalidField(field) => {
                write!(f, "firmware manifest has an invalid field: {field}")
            }
            Self::SignatureMismatch => {
                write!(f, "firmware manifest signature mismatch")
            }
        }
    }
}

impl std::error::Error for OtaManifestError {}

/// 書き込んだimageとmanifestの突き合わせの失敗。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OtaImageError {
    SizeMismatch { expected: u64, actual: u64 },
    ShaMismatch,
}

impl std::fmt::Display for OtaImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SizeMismatch { expected, actual } => {
                write!(
                    f,
                    "firmware size mismatch: expected {expected}, got {actual}"
                )
            }
            Self::ShaMismatch => write!(f, "firmware sha256 mismatch"),
        }
    }
}

impl std::error::Error for OtaImageError {}

/// `GET /firmware/manifest` の応答JSONをパースし、fieldの sanity を確認する。
///
/// 署名の検証はしない。呼び出し側はこの後に [`verify_manifest`] を必ず呼び、
/// 通ってからダウンロードへ進むこと。
pub fn parse_manifest_json(bytes: &[u8]) -> Result<OtaManifest, OtaManifestError> {
    let manifest: OtaManifest = serde_json::from_slice(bytes)
        .map_err(|e| OtaManifestError::InvalidJson(short_json_error(&e)))?;
    check_manifest_fields(&manifest)?;
    Ok(manifest)
}

/// parse済みmanifestのHMAC署名を検証する。失敗したらダウンロードへ進まないこと。
pub fn verify_manifest(manifest: &OtaManifest, secret: &[u8]) -> Result<(), OtaManifestError> {
    if verify_manifest_signature(
        secret,
        &manifest.version,
        manifest.size,
        &manifest.sha256,
        &manifest.created_at,
        &manifest.signature,
    ) {
        Ok(())
    } else {
        Err(OtaManifestError::SignatureMismatch)
    }
}

/// 書き込んだimageの `size` と `sha256` の両方がmanifestと一致するか。
///
/// 署名検証済みのmanifestを渡すこと。不一致なら呼び出し側はslotの更新を
/// 中止し(boot partitionを切り替えず)、書きかけのslotを無効化すること。
pub fn verify_ota_image(
    manifest: &OtaManifest,
    actual_size: u64,
    actual_sha256_hex: &str,
) -> Result<(), OtaImageError> {
    if actual_size != manifest.size {
        return Err(OtaImageError::SizeMismatch {
            expected: manifest.size,
            actual: actual_size,
        });
    }
    if actual_sha256_hex != manifest.sha256 {
        return Err(OtaImageError::ShaMismatch);
    }
    Ok(())
}

/// ダウンロードしながらSHA-256を計算する。2MB級のimageを `Vec` へ全部読むと
/// ESP32のヒープが足りないため、チャンク単位で `update` して `finish_hex` へ渡す。
pub struct StreamingSha256(Sha256);

impl StreamingSha256 {
    pub fn new() -> Self {
        Self(Sha256::new())
    }

    pub fn update(&mut self, chunk: &[u8]) {
        self.0.update(chunk);
    }

    /// 小文字hexのダイジェスト。manifestの `sha256` と同じ形式。
    pub fn finish_hex(self) -> String {
        hex::encode(self.0.finalize())
    }
}

impl Default for StreamingSha256 {
    fn default() -> Self {
        Self::new()
    }
}

/// 起動自己診断で観測する信号(読み取りはfirmware側、判定式だけここに置きhostでテスト)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootChecks {
    pub display_ok: bool,
    pub wifi_connected: bool,
}

/// 起動自己診断の合否。通ったときだけfirmwareは自分をvalidとマークする
/// (`esp_ota_mark_app_valid_cancel_rollback` 相当)。
///
/// あえて甘めに倒す: 目的は「起動不能・即落ちのfirmwareを旧slotへ戻す」ことであり、
/// 「main loop到達+画面+Wi-Fi」で十分。bridge到達性は健全性信号ではない(PC OFFは
/// 正常状態であり、要求すると正常firmwareが戻ってしまう)。一部不調が残るリスクは
/// 次回OTAで修復できるため、厳しすぎて正常firmwareを戻す実害の方が大きい。
pub const fn boot_self_test_passed(checks: &BootChecks) -> bool {
    checks.display_ok && checks.wifi_connected
}

/// manifestの各fieldが配信物としてあり得る値か。
/// `size == 0` はapp imageとして成立しないため拒否。`sha256` は署名対象の文字列
/// そのままを使うため「64文字のhex」だけを見て大小文字の正規化はしない。
fn check_manifest_fields(manifest: &OtaManifest) -> Result<(), OtaManifestError> {
    if manifest.version.is_empty() {
        return Err(OtaManifestError::InvalidField("version"));
    }
    if manifest.size == 0 {
        return Err(OtaManifestError::InvalidField("size"));
    }
    if manifest.sha256.len() != 64 || hex::decode(&manifest.sha256).is_err() {
        return Err(OtaManifestError::InvalidField("sha256"));
    }
    if manifest.created_at.is_empty() {
        return Err(OtaManifestError::InvalidField("created_at"));
    }
    if manifest.signature.is_empty() {
        return Err(OtaManifestError::InvalidField("signature"));
    }
    Ok(())
}

/// `serde_json::Error` の表示は入力の抜粋を含み得る。未検証のmanifest本文を
/// ログへ出さないよう、行・列だけに落とす。
fn short_json_error(e: &serde_json::Error) -> String {
    format!("line {} column {}", e.line(), e.column())
}

#[cfg(test)]
mod test_support {
    use super::{sign_manifest as sign_manifest_result, sign_request as sign_request_result};

    pub fn sign_request(
        secret: &[u8],
        method: &str,
        path: &str,
        timestamp: i64,
        nonce: &str,
        body: &[u8],
    ) -> String {
        sign_request_result(secret, method, path, timestamp, nonce, body)
            .expect("test HMAC initialization succeeds")
    }

    pub fn sign_manifest(
        secret: &[u8],
        version: &str,
        size: u64,
        sha256_hex: &str,
        created_at: &str,
    ) -> String {
        sign_manifest_result(secret, version, size, sha256_hex, created_at)
            .expect("test HMAC initialization succeeds")
    }
}

#[cfg(test)]
mod request_binding_tests {
    use super::{canonical_string, test_support::sign_request, verify_signature};

    const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";
    const TIMESTAMP: i64 = 1_700_000_000;
    const NONCE: &str = "binding-nonce";
    const BODY: &[u8] = br#"{"confirm":true}"#;

    #[test]
    fn canonical_string_binds_path() {
        // PATH を変えると canonical 文字列が変わる。署名の使い回し防止の要。
        assert_ne!(
            canonical_string("POST", "/reboot", TIMESTAMP, NONCE, BODY),
            canonical_string("POST", "/shutdown", TIMESTAMP, NONCE, BODY),
        );
    }

    #[test]
    fn canonical_string_binds_method() {
        // METHOD を変えると canonical 文字列が変わる。署名の使い回し防止の要。
        assert_ne!(
            canonical_string("POST", "/firmware", TIMESTAMP, NONCE, b""),
            canonical_string("GET", "/firmware", TIMESTAMP, NONCE, b""),
        );
    }

    #[test]
    fn rejects_signature_reused_for_another_path() {
        let signature = sign_request(SECRET, "POST", "/reboot", TIMESTAMP, NONCE, BODY);
        assert!(!verify_signature(
            SECRET,
            "POST",
            "/shutdown",
            TIMESTAMP,
            NONCE,
            BODY,
            &signature,
        ));
    }

    #[test]
    fn rejects_signature_reused_for_another_method() {
        let signature = sign_request(SECRET, "POST", "/firmware", TIMESTAMP, NONCE, b"");
        assert!(!verify_signature(
            SECRET,
            "GET",
            "/firmware",
            TIMESTAMP,
            NONCE,
            b"",
            &signature,
        ));
    }
}

#[cfg(test)]
mod manifest_tests {

    use super::{
        manifest_canonical_string, test_support::sign_manifest, verify_manifest_signature,
    };

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
mod ota_tests {
    use super::{
        BootChecks, OtaImageError, OtaManifest, OtaManifestError, StreamingSha256,
        boot_self_test_passed, parse_manifest_json, test_support::sign_manifest, verify_manifest,
        verify_ota_image,
    };

    const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";
    const VERSION: &str = "0.2.0";
    const SIZE: u64 = 19;
    const IMAGE: &[u8] = b"fake-firmware-image";
    const CREATED_AT: &str = "2026-09-04T00:00:00Z";

    fn sha_hex() -> String {
        super::body_sha256_hex(IMAGE)
    }

    fn valid_json() -> Vec<u8> {
        let signature = sign_manifest(SECRET, VERSION, SIZE, &sha_hex(), CREATED_AT);
        serde_json::to_vec(&serde_json::json!({
            "version": VERSION,
            "size": SIZE,
            "sha256": sha_hex(),
            "created_at": CREATED_AT,
            "signature": signature,
        }))
        .unwrap()
    }

    #[test]
    fn valid_manifest_passes_parse_and_verify() {
        let manifest = parse_manifest_json(&valid_json()).unwrap();
        assert_eq!(
            manifest,
            OtaManifest {
                version: VERSION.to_string(),
                size: SIZE,
                sha256: sha_hex(),
                created_at: CREATED_AT.to_string(),
                signature: sign_manifest(SECRET, VERSION, SIZE, &sha_hex(), CREATED_AT),
            }
        );
        verify_manifest(&manifest, SECRET).unwrap();
    }

    #[test]
    fn rejects_tampered_signature() {
        let mut value: serde_json::Value = serde_json::from_slice(&valid_json()).unwrap();
        let signature = value["signature"].as_str().unwrap().to_owned();
        let tampered = format!("{}0", &signature[..signature.len() - 1]);
        value["signature"] = serde_json::Value::String(tampered);
        let manifest = parse_manifest_json(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(
            verify_manifest(&manifest, SECRET),
            Err(OtaManifestError::SignatureMismatch)
        );
    }

    #[test]
    fn rejects_tampered_fields_before_download() {
        // 公開値だけを書き換えたmanifestは署名が合わず、ダウンロードへ進めない。
        let mut version = serde_json::from_slice::<serde_json::Value>(&valid_json()).unwrap();
        version["version"] = serde_json::Value::String("0.2.1".to_string());
        let manifest = parse_manifest_json(&serde_json::to_vec(&version).unwrap()).unwrap();
        assert_eq!(
            verify_manifest(&manifest, SECRET),
            Err(OtaManifestError::SignatureMismatch)
        );

        let mut size = serde_json::from_slice::<serde_json::Value>(&valid_json()).unwrap();
        size["size"] = serde_json::Value::Number((SIZE + 1).into());
        let manifest = parse_manifest_json(&serde_json::to_vec(&size).unwrap()).unwrap();
        assert_eq!(
            verify_manifest(&manifest, SECRET),
            Err(OtaManifestError::SignatureMismatch)
        );

        let mut created = serde_json::from_slice::<serde_json::Value>(&valid_json()).unwrap();
        created["created_at"] = serde_json::Value::String("2026-09-04T00:00:01Z".to_string());
        let manifest = parse_manifest_json(&serde_json::to_vec(&created).unwrap()).unwrap();
        assert_eq!(
            verify_manifest(&manifest, SECRET),
            Err(OtaManifestError::SignatureMismatch)
        );
        // sha256の書き換えは署名前に形式検査で落ちる場合と署名不一致の場合がある。
        // どちらにせよダウンロードへ進まないことが重要。
        let mut value: serde_json::Value = serde_json::from_slice(&valid_json()).unwrap();
        value["sha256"] = serde_json::Value::String("0".repeat(64));
        let manifest = parse_manifest_json(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(
            verify_manifest(&manifest, SECRET),
            Err(OtaManifestError::SignatureMismatch)
        );
    }

    #[test]
    fn rejects_wrong_secret() {
        let manifest = parse_manifest_json(&valid_json()).unwrap();
        assert_eq!(
            verify_manifest(&manifest, b"wrong-secret-wrong-secret-wrong12"),
            Err(OtaManifestError::SignatureMismatch)
        );
    }

    #[test]
    fn rejects_size_mismatch() {
        let manifest = parse_manifest_json(&valid_json()).unwrap();
        verify_manifest(&manifest, SECRET).unwrap();
        assert_eq!(
            verify_ota_image(&manifest, SIZE + 1, &sha_hex()),
            Err(OtaImageError::SizeMismatch {
                expected: SIZE,
                actual: SIZE + 1,
            })
        );
        assert_eq!(
            verify_ota_image(&manifest, SIZE - 1, &sha_hex()),
            Err(OtaImageError::SizeMismatch {
                expected: SIZE,
                actual: SIZE - 1,
            })
        );
    }

    #[test]
    fn rejects_sha_mismatch() {
        let manifest = parse_manifest_json(&valid_json()).unwrap();
        verify_manifest(&manifest, SECRET).unwrap();
        let mut tampered = sha_hex();
        tampered.replace_range(0..1, if &tampered[0..1] == "0" { "1" } else { "0" });
        assert_eq!(
            verify_ota_image(&manifest, SIZE, &tampered),
            Err(OtaImageError::ShaMismatch)
        );
    }

    #[test]
    fn accepts_matching_size_and_sha() {
        let manifest = parse_manifest_json(&valid_json()).unwrap();
        verify_ota_image(&manifest, SIZE, &sha_hex()).unwrap();
    }

    #[test]
    fn rejects_invalid_json() {
        // JSONですらない。
        assert!(matches!(
            parse_manifest_json(b"{not json"),
            Err(OtaManifestError::InvalidJson(_))
        ));
        // 空。
        assert!(matches!(
            parse_manifest_json(b""),
            Err(OtaManifestError::InvalidJson(_))
        ));
        // field不足。
        assert!(matches!(
            parse_manifest_json(b"{}"),
            Err(OtaManifestError::InvalidJson(_))
        ));
        // 型違い。
        let mut value: serde_json::Value = serde_json::from_slice(&valid_json()).unwrap();
        value["size"] = serde_json::Value::String("19".to_string());
        assert!(matches!(
            parse_manifest_json(&serde_json::to_vec(&value).unwrap()),
            Err(OtaManifestError::InvalidJson(_))
        ));
        // 値としてあり得ないもの。
        for (field, bad) in [
            ("version", ""),
            ("sha256", "xyz"),
            ("created_at", ""),
            ("signature", ""),
        ] {
            let mut value: serde_json::Value = serde_json::from_slice(&valid_json()).unwrap();
            value[field] = serde_json::Value::String(bad.to_string());
            assert!(
                matches!(
                    parse_manifest_json(&serde_json::to_vec(&value).unwrap()),
                    Err(OtaManifestError::InvalidField(_))
                ),
                "{field}"
            );
        }
        // size 0 のimageは成立しない。
        let mut value: serde_json::Value = serde_json::from_slice(&valid_json()).unwrap();
        value["size"] = serde_json::Value::Number(0.into());
        assert!(matches!(
            parse_manifest_json(&serde_json::to_vec(&value).unwrap()),
            Err(OtaManifestError::InvalidField("size"))
        ));
    }

    // Issue #136: `InvalidJson` の表示に manifest 本文の断片を含めない。
    // `serde_json::Error` の表示は入力の抜粋を含み得るため、
    // `short_json_error` で行・列だけに落としている。
    // エラーへ本文抜粋を載せる変異を入れても既存テストは緑のままだったため、
    // `Display` と `Debug` の両方で本文マーカーが出ないことを固定する。
    // `app_config_tests.rs` の `parse_error_reports_line_number_without_leaking_values`
    // と同種の守り (あのファイル自体は触らない)。
    #[test]
    fn invalid_json_error_does_not_leak_manifest_body() {
        let marker = "leak-check-marker-version-xyz";
        // 型違い:本文にマーカーを含むが、エラーは行・列だけになること。
        let body = format!(r#"{{"version": "{marker}", "size": "not-a-number"}}"#);
        let err = parse_manifest_json(body.as_bytes()).unwrap_err();
        let display = format!("{err}");
        let debug = format!("{err:?}");
        assert!(!display.contains(marker), "display={display}");
        assert!(!debug.contains(marker), "debug={debug}");

        // JSONですらない本文でも同様。
        let garbage = format!("{{not json {marker}");
        let err = parse_manifest_json(garbage.as_bytes()).unwrap_err();
        let display = format!("{err}");
        let debug = format!("{err:?}");
        assert!(!display.contains(marker), "display={display}");
        assert!(!debug.contains(marker), "debug={debug}");
    }

    #[test]
    fn streaming_hash_matches_oneshot_hash() {
        let mut streaming = StreamingSha256::new();
        for chunk in IMAGE.chunks(5) {
            streaming.update(chunk);
        }
        assert_eq!(streaming.finish_hex(), sha_hex());
        assert_eq!(streaming_default_is_usable(), sha_hex());
    }

    fn streaming_default_is_usable() -> String {
        let mut hashing = StreamingSha256::default();
        hashing.update(IMAGE);
        hashing.finish_hex()
    }

    #[test]
    fn boot_self_test_needs_display_and_wifi() {
        // 両方そろったときだけ通る。
        assert!(boot_self_test_passed(&BootChecks {
            display_ok: true,
            wifi_connected: true,
        }));
        // 片方でも欠けたら通さない。壊れたfirmwareを居座らせない。
        for checks in [
            BootChecks {
                display_ok: false,
                wifi_connected: true,
            },
            BootChecks {
                display_ok: true,
                wifi_connected: false,
            },
            BootChecks {
                display_ok: false,
                wifi_connected: false,
            },
        ] {
            assert!(!boot_self_test_passed(&checks), "{checks:?}");
        }
    }
}
