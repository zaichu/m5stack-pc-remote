// OTAクライアント: manifest取得 → 署名検証 → 非activeスロットへ書込 → reboot。
// 起動自己診断とvalidマークもここが受け持つ。
//
// **呼ぶ前にTelegram long pollingのHTTPS接続を閉じ切ること。** polling側が
// mbedTLSのヒープを掴んだままだと、OTAのHTTPクライアントと書込バッファに回す
// ヒープが足りなくなる(bridgeへの接続自体はplain HTTPでTLS本数の制約外)。
//
// 2MB級のfirmwareを `Vec` へ全部読むとヒープ不足で落ちるため、`OTA_CHUNK_SIZE`
// 単位でread → write → SHA-256更新を回す。1024Bはメイン8KB/ワーカー12KB級の
// スタックに載る大きさで、LAN内HTTPでは転送律速にならない。
//
// `CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE=y` のため、boot partitionを切り替えても
// 新slotはpendingのままで、`mark_app_valid_after_self_test` が通らない限り
// 次回起動で旧slotへ戻る。

use std::error::Error;
use std::fmt;
use std::time::Duration;

use embedded_svc::http::client::{Client as HttpClient, Response as HttpResponse};
use embedded_svc::http::Method;
use esp_idf_svc::http::client::{Configuration as HttpConfiguration, EspHttpConnection};
use esp_idf_svc::ota::EspOta;

use pc_remote_signing::{
    parse_manifest_json, verify_manifest, verify_ota_image, BootChecks, OtaImageError, OtaManifest,
    OtaManifestError, StreamingSha256,
};

use crate::app_config::AppConfig;

/// 署名付きリクエスト対象のパス。wire protocol上は `pc-remote-signing` の
/// canonical文字列へ入るため、bridge側 (`server.rs`) と一致させること。
pub const MANIFEST_PATH: &str = "/firmware/manifest";
pub const FIRMWARE_PATH: &str = "/firmware";

/// ストリーミング書き込みの1回分。選定根拠はmodule冒頭の「メモリ」を参照。
pub const OTA_CHUNK_SIZE: usize = 1024;

/// ESP32のヒープ保護のため、超過分は読まずにエラーにする。
pub const MANIFEST_MAX_BYTES: usize = 4096;

const MANIFEST_TIMEOUT: Duration = Duration::from_secs(10);
/// 2MB級の転送中はflash書込で間が空くため、manifestより長く取る。
const FIRMWARE_TIMEOUT: Duration = Duration::from_secs(30);

/// OTAの失敗。ログ・エラー文言にsecretや本文・URLを含めない。
#[derive(Debug)]
pub enum OtaError {
    /// NTP未同期。署名してもbridge側のtimestamp検証で弾かれるため送らない。
    ClockNotSynced,
    /// HTTP clientの生成・送信・読み取りの失敗。値はespのエラー文言のみ。
    Transport(String),
    /// 200以外のステータス。`endpoint` は `MANIFEST_PATH` 等の固定ラベル。
    UnexpectedStatus {
        endpoint: &'static str,
        status: u16,
    },
    ResponseTooLarge,
    Manifest(OtaManifestError),
    Image(OtaImageError),
    /// OTA slot操作 (`EspOta`) の失敗。値はespのエラー文言のみ。
    Ota(String),
}

impl OtaError {
    /// Telegramへ返す文言。`Display` と分けているのは、`Transport` / `Ota` が抱える
    /// espのエラー文言にbridgeのLAN IPが混ざり得るため。埋め込んでよいのは
    /// `endpoint` のような固定ラベルと `status` の数値だけ。
    pub fn user_message(&self) -> String {
        match self {
            OtaError::ClockNotSynced => {
                "時刻同期がまだ完了していません。少し待ってからやり直してください。".to_string()
            }
            OtaError::Transport(_) => {
                "PCへの接続に失敗しました。PCがオンでbridgeが動いているか確認してください。"
                    .to_string()
            }
            OtaError::UnexpectedStatus { endpoint, status } => {
                format!("PCが {endpoint} に {status} を返しました。")
            }
            OtaError::ResponseTooLarge => "manifestが大きすぎます。".to_string(),
            OtaError::Manifest(_) => {
                "manifestの検証に失敗しました。配信中のfirmwareを確認してください。".to_string()
            }
            OtaError::Image(_) => {
                "ダウンロードしたfirmwareが壊れています。更新は中止しました。".to_string()
            }
            OtaError::Ota(_) => "firmwareの書き込みに失敗しました。更新は中止しました。".to_string(),
        }
    }
}

impl fmt::Display for OtaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OtaError::ClockNotSynced => write!(f, "system clock is not NTP-synced yet"),
            OtaError::Transport(reason) => write!(f, "firmware fetch failed: {reason}"),
            OtaError::UnexpectedStatus { endpoint, status } => {
                write!(f, "firmware {endpoint} returned {status}")
            }
            OtaError::ResponseTooLarge => write!(f, "firmware manifest is too large"),
            OtaError::Manifest(e) => write!(f, "{e}"),
            OtaError::Image(e) => write!(f, "{e}"),
            OtaError::Ota(reason) => write!(f, "OTA write failed: {reason}"),
        }
    }
}

impl Error for OtaError {}

impl From<OtaManifestError> for OtaError {
    fn from(e: OtaManifestError) -> Self {
        OtaError::Manifest(e)
    }
}

impl From<OtaImageError> for OtaError {
    fn from(e: OtaImageError) -> Self {
        OtaError::Image(e)
    }
}

/// OTAを実行する。**成功時は戻らない**(rebootする)。失敗時だけ `Err` を返す。
///
/// 呼ぶ前にTelegram long pollingのHTTPS接続を閉じ切ること(module冒頭参照)。
/// `on_progress` / `on_applying` の失敗ではOTAを止めない契約。
pub fn run_ota_update(
    config: &AppConfig,
    pc_ip_address: &str,
    on_progress: &mut dyn FnMut(&OtaManifest, u64),
    on_applying: &mut dyn FnMut(&OtaManifest),
) -> Result<(), OtaError> {
    let manifest = fetch_verified_manifest(config, pc_ip_address)?;
    download_and_flash(
        &manifest,
        config,
        pc_ip_address,
        on_progress,
        on_applying,
    )?;
    println!("ota: update complete, rebooting");
    esp_idf_svc::hal::reset::restart()
}

/// 起動自己診断を通ったときだけ実行中アプリをvalidとマークする。
/// main loopからWi-Fi接続中に定期的に呼ぶ。クラッシュループするfirmwareは
/// ここへ届かないため、そのままではvalidにならず次回起動で旧slotへ戻る。
///
/// **外側で条件を緩めて呼ばないこと。** 自己診断が失敗しうる経路で無条件に
/// マークすると、壊れたfirmwareが居座る。
///
/// 通常起動で呼んでも無害(未pendingならno-op)。失敗しても「次回起動で旧slotへ
/// 戻る」方向なのでpanicせずログに留めてよい。戻り値はマークを試みたかどうか。
pub fn mark_app_valid_after_self_test(checks: &BootChecks) -> Result<bool, String> {
    if !pc_remote_signing::boot_self_test_passed(checks) {
        return Ok(false);
    }
    let mut ota = EspOta::new().map_err(|e| e.to_string())?;
    ota.mark_running_slot_valid().map_err(|e| e.to_string())?;
    Ok(true)
}

/// 署名付き `GET /firmware/manifest` でmanifestを取得・検証する。
///
/// `/update` の確認表示と実行時の検証が同じ関数を通るため、表示と検証が食い違わない。
/// パースと署名検証は接続を閉じてから行い、検証失敗時はバイナリ取得へ進まない。
pub fn fetch_verified_manifest(
    config: &AppConfig,
    pc_ip_address: &str,
) -> Result<OtaManifest, OtaError> {
    let body = with_signed_get(
        config,
        pc_ip_address,
        MANIFEST_PATH,
        MANIFEST_TIMEOUT,
        |response| {
            if response.status() != 200 {
                return Err(OtaError::UnexpectedStatus {
                    endpoint: MANIFEST_PATH,
                    status: response.status(),
                });
            }
            let mut body = Vec::new();
            let mut chunk = [0u8; 512];
            loop {
                let read = response
                    .read(&mut chunk)
                    .map_err(|e| OtaError::Transport(e.to_string()))?;
                if read == 0 {
                    break;
                }
                if body.len() + read > MANIFEST_MAX_BYTES {
                    return Err(OtaError::ResponseTooLarge);
                }
                body.extend_from_slice(&chunk[..read]);
            }
            Ok(body)
        },
    )?;
    let manifest = parse_manifest_json(&body)?;
    verify_manifest(&manifest, config.bridge_shared_secret.as_bytes())?;
    println!(
        "ota: manifest version={} size={} verified",
        manifest.version, manifest.size
    );
    Ok(manifest)
}

/// バイナリを非activeスロットへ書く。
///
/// **`manifest` は必ず `fetch_verified_manifest` で検証済みのものを渡すこと。**
/// 未検証のmanifestで呼ぶと、攻撃者の用意したimageを書き込みかねない。
/// size/sha256が不一致なら `complete` せずに抜ける(Dropが `esp_ota_abort` し、
/// boot partitionは切り替わらない)。
fn download_and_flash(
    manifest: &OtaManifest,
    config: &AppConfig,
    pc_ip_address: &str,
    on_progress: &mut dyn FnMut(&OtaManifest, u64),
    on_applying: &mut dyn FnMut(&OtaManifest),
) -> Result<(), OtaError> {
    let mut ota = EspOta::new().map_err(|e| OtaError::Ota(e.to_string()))?;
    let mut update = ota
        .initiate_update()
        .map_err(|e| OtaError::Ota(e.to_string()))?;

    let mut hashing = StreamingSha256::new();
    let mut received: u64 = 0;
    // 割合で刻む。バイト数だとイメージのサイズでバーの動く回数が変わる。
    let mut reported_percent: u8 = 0;
    with_signed_get(
        config,
        pc_ip_address,
        FIRMWARE_PATH,
        FIRMWARE_TIMEOUT,
        |response| {
            if response.status() != 200 {
                return Err(OtaError::UnexpectedStatus {
                    endpoint: FIRMWARE_PATH,
                    status: response.status(),
                });
            }
            // 早期return時は `update` がDropされて `esp_ota_abort` する。
            let mut chunk = [0u8; OTA_CHUNK_SIZE];
            loop {
                let read = response
                    .read(&mut chunk)
                    .map_err(|e| OtaError::Transport(e.to_string()))?;
                if read == 0 {
                    break;
                }
                // Issue #130-2: 申告サイズを超える分は1バイトも書かずに打ち切る。
                // 終端まで書いてから突き合わせると、slot上限まで無駄な書込が続く。
                let incoming = received + read as u64;
                if config_validation::ota_received_too_large(incoming, manifest.size) {
                    return Err(OtaImageError::SizeMismatch {
                        expected: manifest.size,
                        actual: incoming,
                    }
                    .into());
                }
                hashing.update(&chunk[..read]);
                update
                    .write(&chunk[..read])
                    .map_err(|e| OtaError::Ota(e.to_string()))?;
                received += read as u64;
                let percent = pc_remote_signing::ota_progress_percent(received, manifest.size);
                if percent >= reported_percent.saturating_add(
                    pc_remote_signing::OTA_PROGRESS_STEP_PERCENT,
                ) {
                    reported_percent = percent;
                    println!("ota: received {received}/{} bytes ({percent}%)", manifest.size);
                    // `?` を使うと、Telegramが一時的に応答しないだけで更新が巻き戻る。
                    on_progress(manifest, received);
                }
            }
            Ok(())
        },
    )?;
    // 2本目の接続はここで閉じた。以降はflash済みデータの突き合わせだけを行う。

    // Issue #143: 刻みに届かない端数が残り、94%で止まったまま再起動した。
    // 端数は必ず出るので、ループ後に刻みと無関係に1回通知して100%にする。
    on_progress(manifest, received);
    println!("ota: download complete ({received}/{} bytes)", manifest.size);

    // 不一致なら `?` で抜け、`update` のDrop(=abort)でboot切替は行われない。
    verify_ota_image(manifest, received, &hashing.finish_hex())?;
    println!("ota: image verified ({received} bytes), activating");
    update
        .complete()
        .map_err(|e| OtaError::Ota(e.to_string()))?;
    // 同期POSTのため、戻った時点で送信済み。直後の `restart()` で通知が欠けない。
    on_applying(manifest);
    Ok(())
}

/// HMAC署名付きGETを送り、応答の読み取りを `f` に任せる。署名対象bodyは空
/// (bridge側の `verify_headers` も `b""` で検証する)。
///
/// `f` から戻った時点で接続は閉じる。manifest取得とバイナリ取得を別々に呼ぶため、
/// 同時に開くのは1本だけ(module冒頭のヒープの制約)。
fn with_signed_get<T>(
    config: &AppConfig,
    pc_ip_address: &str,
    path: &'static str,
    timeout: Duration,
    f: impl FnOnce(&mut HttpResponse<&mut EspHttpConnection>) -> Result<T, OtaError>,
) -> Result<T, OtaError> {
    let timestamp = crate::bridge_client::unix_now()
        .map_err(|_| OtaError::ClockNotSynced)?;
    let request_nonce = crate::bridge_client::nonce();
    let signature = pc_remote_signing::sign_request(
        config.bridge_shared_secret.as_bytes(),
        "GET",
        path,
        timestamp as i64,
        &request_nonce,
        b"",
    );

    // URL全体はログへ出さない (IPを含むため)。エラー時はendpointラベルだけ使う。
    let url = format!("http://{pc_ip_address}:{}{path}", config.bridge_port);
    let mut client = HttpClient::wrap(
        EspHttpConnection::new(&HttpConfiguration {
            timeout: Some(timeout),
            ..Default::default()
        })
        .map_err(|e| OtaError::Transport(e.to_string()))?,
    );

    let timestamp_text = timestamp.to_string();
    let headers = [
        ("X-Timestamp", timestamp_text.as_str()),
        ("X-Nonce", request_nonce.as_str()),
        ("X-Signature", signature.as_str()),
    ];
    let request = client
        .request(Method::Get, &url, &headers)
        .map_err(|e| OtaError::Transport(e.to_string()))?;
    let mut response = request
        .submit()
        .map_err(|e| OtaError::Transport(e.to_string()))?;
    f(&mut response)
}
