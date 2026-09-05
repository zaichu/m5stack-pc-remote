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

/// OTA配信のmanifest(`GET /firmware/manifest` の応答)。
///
/// JSONの形の正本はm5stack-pc-bridgeの `firmware::FirmwareManifest` とここで
/// 共有する。片方だけfieldを増減すると署名の検証以前にparseで壊れるため、
/// 構造体は1箇所に置く。未知fieldは許容する(bridge側が将来fieldを足しても、
/// 古いfirmwareのOTAがparse失敗で止まらないようにするため)。
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
pub struct OtaManifest {
    pub version: String,
    pub size: u64,
    pub sha256: String,
    pub created_at: String,
    pub signature: String,
}

/// manifestのparse・検証・突き合わせの失敗。
///
/// エラー文言にはmanifest本文もsecretも含めない。本文は署名検証前の
/// 未検証データであり、ログへ出すのはfield名と理由だけに留める。
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
            OtaManifestError::InvalidJson(reason) => {
                write!(f, "firmware manifest is not valid JSON: {reason}")
            }
            OtaManifestError::InvalidField(field) => {
                write!(f, "firmware manifest has an invalid field: {field}")
            }
            OtaManifestError::SignatureMismatch => {
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
            OtaImageError::SizeMismatch { expected, actual } => {
                write!(f, "firmware size mismatch: expected {expected}, got {actual}")
            }
            OtaImageError::ShaMismatch => write!(f, "firmware sha256 mismatch"),
        }
    }
}

impl std::error::Error for OtaImageError {}

/// `GET /firmware/manifest` の応答JSONをパースし、fieldの sanity を確認する。
///
/// 署名の検証はしない。呼び出し側はこの後に [`verify_manifest`] を必ず呼び、
/// 通ってからダウンロードへ進むこと。
pub fn parse_manifest_json(bytes: &[u8]) -> Result<OtaManifest, OtaManifestError> {
    let manifest: OtaManifest =
        serde_json::from_slice(bytes).map_err(|e| OtaManifestError::InvalidJson(short_json_error(&e)))?;
    check_manifest_fields(&manifest)?;
    Ok(manifest)
}

/// parse済みmanifestのHMAC署名を検証する。失敗したらダウンロードへ進まないこと。
pub fn verify_manifest(
    manifest: &OtaManifest,
    secret: &[u8],
) -> Result<(), OtaManifestError> {
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

/// OTAの進捗率(0-100)。表示専用なので、異常な入力でもpanicせず丸める。
///
/// `total` が0のとき0除算になるため、進捗不明として0を返す。manifestのsizeは
/// 検証前の値を表示に使うことがあり、0や実際より小さい値が来ても落とさない
/// (不一致は `verify_ota_image` が書き込み後に弾く。表示はそれより手前で動く)。
pub fn ota_progress_percent(received: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    // u64同士の乗算はオーバーフローし得る。`saturating_mul` だと巨大な値で
    // 飽和して比率が壊れる(received == total でも100にならない)ため、
    // u128へ広げてから計算する。
    let received = received.min(total);
    ((received as u128 * 100) / total as u128) as u8
}

/// 進捗を通知する刻み(パーセント)。
///
/// バイト数ではなく割合で刻む。バイト数固定だとファイルサイズによって
/// 更新回数が変わり、小さいイメージでは数回しか動かない(実測: 256KB刻みだと
/// 1.4MBのイメージで5回しか更新されず、1回で18%飛んだ)。
///
/// 細かくしすぎないこと。1回の通知ごとにTLSハンドシェイクが入り、その間
/// ダウンロードが止まる。5%ならバーが20回動き、追加コストは20回分で済む。
pub const OTA_PROGRESS_STEP_PERCENT: u8 = 5;

/// Telegramへ出す進捗テキスト。1行のバーとパーセント、受信量を返す。
///
/// `editMessageText` で同じメッセージを書き換え続ける前提。Telegramは同一内容への
/// 編集をエラー(400)にするため、呼び出し側は内容が変わるときだけ送ること。
pub fn ota_progress_text(version: &str, received: u64, total: u64) -> String {
    // セル数は刻みと合わせる。刻みより粗いとバーが動かない回が出て、
    // 細かいとバーだけ動いて数字が変わらない回が出る。
    // `OTA_PROGRESS_STEP_PERCENT` から計算することで、定数だけ変えて
    // バーを古いままにする事故を防ぐ (テストでもセル数を固定している)。
    const CELLS: usize = (100 / OTA_PROGRESS_STEP_PERCENT) as usize;
    let percent = ota_progress_percent(received, total);
    let filled = (percent as usize * CELLS).div_ceil(100).min(CELLS);
    let bar: String = "█".repeat(filled) + &"░".repeat(CELLS - filled);
    format!(
        "firmware更新中 ({version})\n[{bar}] {percent}%  {}KB / {}KB",
        received / 1024,
        total / 1024
    )
}

/// 検証・書き込み完了後に `editMessageText` で同じメッセージへ出す文言。
///
/// ダウンロード完了の100%バーとは別の文字列にすること。Telegramは同一内容への
/// 編集を400にするため、同じ文言だと再起動前の最後の通知が届かない。
/// 呼び出し側はこの通知が戻った後に `restart()` するため、送信の完了を
/// 待たずに再起動して通知が欠けることはない。
pub fn ota_applying_text(version: &str) -> String {
    format!("firmware更新を適用します。再起動します ({version})")
}

/// ダウンロードしながらSHA-256を計算するストリーミングハーシャー。
///
/// 背景: firmware(2MB級)を `Vec` へ全部読んでから `body_sha256_hex` すると
/// ESP32のヒープが足りない。OTAクライアントはチャンク単位で `update` し、
/// 書き終わったら `finish_hex` して [`verify_ota_image`] へ渡す。
/// `sha2` crateはこの共有crateが既に持つため、firmware側へ新しい依存を
/// 足さずに使える。ハードウェアに依存しないのでhostでテストできる。
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

/// 起動自己診断で観測する信号。ハードウェアの読み取り自体はfirmware側で行い、
/// 合否の判定式だけをここに置くことでhostの `cargo test` で検証できる。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootChecks {
    /// 画面の初期化と最初の描画が成功した。
    pub display_ok: bool,
    /// Wi-Fiに接続できた。
    pub wifi_connected: bool,
}

/// 起動自己診断の合否。通ったときだけfirmwareは自分をvalidとマークする
/// (`esp_ota_mark_app_valid_cancel_rollback` 相当)。
///
/// 判定はあえて甘め(この2条件だけ)に倒している。理由:
/// - 主目的は「起動できない・すぐ落ちるfirmwareを旧slotへ戻す」ことであり、
///   その検出には「main loopへ到達し、画面とWi-Fiが動いた」で十分。
/// - bridge到達性やNTP同期はfirmwareの健全性信号ではない。PC OFFはこの端末に
///   とって正常状態であり、bridge到達を要求すると正常なfirmwareが戻ってしまう。
///   署名付きで叩けるbridgeの `/status` 相当API自体、いまは存在しない。
/// - 起動するが一部不調のfirmwareが残るリスクはあるが、次回OTAで修復できるため
///   brickではなく、厳しすぎて正常firmwareを戻す実害の方が大きい。
pub fn boot_self_test_passed(checks: &BootChecks) -> bool {
    checks.display_ok && checks.wifi_connected
}

/// `/update` 実行前にmanifestのversionとsizeを提示する確認文。
/// version/sizeは公開情報でありsecretではない。sha256や署名は載せない
/// (Telegramへの送信文に不要な情報を増やさない)。
pub fn ota_confirm_text(version: &str, size: u64) -> String {
    format!(
        "firmware更新があります。\nversion: {version}\nsize: {size} bytes\n\
         更新しますか？\nボタンを押すと開始します。完了すると自動で再起動します。"
    )
}

/// manifestの各fieldが配信物としてあり得る値か。
///
/// bridge側は `version` が無いときに `"unknown"` を入れるため空にはならない。
/// `size == 0` のimageはESP-IDFのapp imageとして成立しないため拒否する。
/// `sha256` は署名対象の文字列そのままを使うので、ここでは「64文字のhex」
/// だけを見て、大小文字の正規化はしない(署名検証が exact match で担保する)。
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
mod request_binding_tests {
    use super::{canonical_string, sign_request, verify_signature};

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

#[cfg(test)]
mod ota_tests {
    use super::{
        boot_self_test_passed, ota_confirm_text, parse_manifest_json, sign_manifest,
        verify_manifest, verify_ota_image, BootChecks, OtaImageError, OtaManifest, OtaManifestError,
        StreamingSha256,
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

    #[test]
    fn ota_confirm_text_shows_version_and_size() {
        let text = ota_confirm_text(VERSION, SIZE);
        assert!(text.contains(VERSION), "{text}");
        assert!(text.contains(&SIZE.to_string()), "{text}");
    }
}

#[cfg(test)]
mod ota_progress_tests {
    use super::{
        ota_applying_text, ota_progress_percent, ota_progress_text, OTA_PROGRESS_STEP_PERCENT,
    };

    #[test]
    fn percent_handles_boundaries_without_panicking() {
        // total=0 は0除算になる。進捗不明として0を返す。
        assert_eq!(ota_progress_percent(0, 0), 0);
        assert_eq!(ota_progress_percent(100, 0), 0);
        assert_eq!(ota_progress_percent(0, 1000), 0);
        assert_eq!(ota_progress_percent(500, 1000), 50);
        assert_eq!(ota_progress_percent(1000, 1000), 100);
        // 受信が想定を超えても100で頭打ちにする(表示が101%にならない)。
        assert_eq!(ota_progress_percent(2000, 1000), 100);
        // u64の上限付近でも saturating_mul でオーバーフローしない。
        assert_eq!(ota_progress_percent(u64::MAX, u64::MAX), 100);
    }

    #[test]
    fn text_shows_bar_percent_and_size() {
        let text = ota_progress_text("0.3.0", 512 * 1024, 1024 * 1024);
        assert!(text.contains("0.3.0"), "{text}");
        assert!(text.contains("50%"), "{text}");
        assert!(text.contains("512KB / 1024KB"), "{text}");
        assert!(text.contains('█') && text.contains('░'), "{text}");
    }

    #[test]
    fn bar_is_empty_at_zero_and_full_at_hundred() {
        // 20セル = 100 / OTA_PROGRESS_STEP_PERCENT(5)。
        let zero = ota_progress_text("v", 0, 1000);
        assert!(zero.contains("[░░░░░░░░░░░░░░░░░░░░]"), "{zero}");
        let full = ota_progress_text("v", 1000, 1000);
        assert!(full.contains("[████████████████████]"), "{full}");
    }

    #[test]
    fn text_changes_as_download_advances() {
        // 同一内容への editMessageText はTelegramが400にする。刻みごとに
        // 文字列が変わることを固定しておく。
        let a = ota_progress_text("v", 100 * 1024, 1000 * 1024);
        let b = ota_progress_text("v", 200 * 1024, 1000 * 1024);
        assert_ne!(a, b);
    }

    #[test]
    fn bar_cell_count_matches_step_percent() {
        // 描画セル数を `100 / OTA_PROGRESS_STEP_PERCENT` に固定する。
        // 定数だけ変えてバーを古いままにすると、通知回数と見た目がずれる。
        let expected = (100 / OTA_PROGRESS_STEP_PERCENT) as usize;
        for (received, total) in [(0, 1000), (500, 1000), (1000, 1000)] {
            let text = ota_progress_text("v", received, total);
            let bar = text.split('[').nth(1).unwrap().split(']').next().unwrap();
            assert_eq!(bar.chars().count(), expected, "{text}");
        }
    }

    #[test]
    fn text_changes_at_every_step() {
        // STEP刻みで進めたとき、毎回テキストが変わること。同一内容だと
        // Telegramが400を返すため、1回でも変わらない刻みがあってはならない。
        let total = 1000u64;
        let mut previous = ota_progress_text("v", 0, total);
        let mut percent = 0u8;
        let mut steps = 0u32;
        while percent < 100 {
            percent = percent.saturating_add(OTA_PROGRESS_STEP_PERCENT);
            let received = percent as u64 * total / 100;
            // 刻み通りに進んでいることの sanity (受信量の計算ミス検出用)。
            assert_eq!(ota_progress_percent(received, total), percent.min(100));
            let text = ota_progress_text("v", received, total);
            assert_ne!(text, previous, "percent={percent}");
            previous = text;
            steps += 1;
        }
        assert_eq!(steps, (100 / OTA_PROGRESS_STEP_PERCENT) as u32);
    }

    #[test]
    fn final_notification_reaches_hundred_despite_remainder() {
        // Issue #143追加分: 1,388,544B の実機サイズでは刻みの端数が残り、
        // 最終通知なしでは94%で止まった。割合ベースでも端数は必ず出るため、
        // 受信量=サイズでの最終通知が100%になることを固定する
        // (`ota.rs` はループ後に刻みと無関係に必ず1回呼ぶ)。
        let total = 1_388_544u64;
        assert_eq!(ota_progress_percent(total, total), 100);
        let text = ota_progress_text("v", total, total);
        assert!(text.contains("100%"), "{text}");
        let expected = (100 / OTA_PROGRESS_STEP_PERCENT) as usize;
        let bar = text.split('[').nth(1).unwrap().split(']').next().unwrap();
        assert_eq!(bar.chars().count(), expected, "{text}");
        assert!(bar.chars().all(|c| c == '█'), "{text}");
    }

    #[test]
    fn applying_text_differs_from_full_bar() {
        // 再起動前の最後の通知が直前の100%バーと同じ内容だとTelegramが400に
        // するため、文言が変わることを固定する。
        let full = ota_progress_text("v", 1000, 1000);
        let applying = ota_applying_text("v");
        assert_ne!(full, applying, "{full} vs {applying}");
    }
}
