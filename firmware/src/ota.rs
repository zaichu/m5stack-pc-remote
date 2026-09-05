// OTA Phase 3 クライアント: manifest取得 → 署名検証 → 非activeスロットへ書込 → reboot。
// 起動自己診断とvalidマーク (Phase 4) もこのmoduleが受け持つ。
//
// 公開関数:
//   - `run_ota_update`: Telegram `/update` の確認後に呼ぶ (Phase 4で配線済み)。
//   - `fetch_verified_manifest`: `/update` が確認前にversion/sizeを提示するための取得。
//   - `mark_app_valid_after_self_test`: 起動自己診断の通過時にだけvalidマークする。
//
// ## 接続とヒープの制約 (必読)
//
// ESP32 の mbedTLS ヒープでは実質同時に1本のTLS接続しか張れず、2本目を開くと
// `ESP_ERR_HTTP_CONNECT` で失敗する(実機確認済み。`telegram.rs` の `poll_once`
// 内コメントを参照)。
//
// ただし bridge への接続は plain HTTP (`http://`) なので、この module が張る
// manifest取得とバイナリ取得の2本は mbedTLS を使わず、TLS本数の制約には当たらない。
// それでも1本ずつ開閉する構造にしてあるのは、2MB転送中のピークヒープを下げるため。
// (各fetchを `{}` スコープへ閉じ込め、検証・書込は接続を持たない状態で行う。)
//
// 呼び出し側 (Phase 4) への制約はこちらが本命:
// **Telegram long polling のHTTPS接続を開いたままこの関数を呼ばないこと。**
// polling 側が mbedTLS のヒープを掴んだままだと、OTAのHTTPクライアントと
// OTA書込バッファに回すヒープが足りなくなる。呼ぶ前に polling 接続を閉じ切ること。
//
// ## メモリ
//
// firmwareは2MB級のため、`Vec` へ全体を読んでから書くことはしない(ヒープ不足で
// 落ちる)。`OTA_CHUNK_SIZE` 単位で read → OTA slotへwrite → SHA-256へupdate を
// 回す。チャンクを1024Bにした根拠:
//   - メイン8KB / ワーカー12KB級のスタックに載る小ささ (telegram.rs も
//     getUpdatesの読み取りを512Bチャンクにしている)
//   - `esp_ota_write` は任意長を受け付けるため、小さくしても正しく書ける
//   - LAN内HTTPでは1KBずつでも転送律速にならない
//
// ## OTA backendの選択
//
// `esp_idf_svc::ota::EspOta` を使う。`esp_ota_begin/write/end/set_boot_partition`
// のsafe wrapperで、新しい依存も `esp_idf_sys` のunsafe直呼びも要らない。
// 中断時は `EspOtaUpdate` を `complete` せずDropするだけで `esp_ota_abort` が
// 走るため、後始末が漏れない。
//
// ## ロールバック
//
// `sdkconfig.defaults` の `CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE=y` により、
// `complete()` でboot partitionを切り替えても新slotはpending扱いのままで、
// 起動後に自分をvalidとマークしない限り再起動時に旧slotへ戻る。
// validマークは `mark_app_valid_after_self_test` が起動自己診断の通過時にだけ行う。
//
// ## 純粋関数とテスト
//
// manifestのJSONパース・署名検証・size/sha256突き合わせ・ストリーミングSHA-256は
// ハードウェアに依存しないため `pc-remote-signing` に置き、hostでテストしている
// (`cargo test --manifest-path shared/pc-remote-signing/Cargo.toml` の `ota_tests`)。
// このmoduleはESP-IDF上でのHTTP取得とOTA slot操作だけを受け持つ。
// Phase 4でTelegram `/update` へ配線したため、dead_code抑制は外してある。

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

/// manifest応答の受け入れ上限。manifestは数百BのJSONなので十分に余裕がある。
/// ESP32のヒープ保護のため、超過分は読まずにエラーにする
/// (`telegram.rs` の `RESPONSE_MAX_BYTES` と同じ考え方)。
pub const MANIFEST_MAX_BYTES: usize = 4096;

/// manifest取得の受信タイムアウト。数百Bの応答がLANで返るのを待つだけ。
const MANIFEST_TIMEOUT: Duration = Duration::from_secs(10);
/// バイナリ取得の受信タイムアウト。2MB級の転送中にflash書込で間が空くため、
/// manifestより長めに取る。
const FIRMWARE_TIMEOUT: Duration = Duration::from_secs(30);
/// シリアルへ進捗ログを出す間隔。2MBを1KBずつ読むと2000回を超えるため、毎回出さない。
/// Telegramへの通知はこれとは別に、割合ベースで刻む
/// (`pc_remote_signing::OTA_PROGRESS_STEP_PERCENT`)。

/// OTAの失敗。ログ・エラーメッセージにsecretや本文は含めない。
/// URL全体も持たず、endpointは識別用の固定ラベルだけにする。
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
    /// Telegramへ返す文言。`Display` と分けているのは、`Transport` / `Ota` が
    /// 抱えるespのエラー文言に接続先(bridgeのLAN IP)が混ざる可能性があり、
    /// 生の文字列を外へ出したくないため。詳細はシリアルログ側にだけ残す。
    ///
    /// ここで埋め込んでよいのは、`endpoint` のような固定ラベルと `status` の
    /// 数値だけにすること。
    pub fn user_message(&self) -> String {
        match self {
            OtaError::ClockNotSynced => {
                "時刻同期がまだ完了していません。少し待ってからやり直してください。".to_string()
            }
            OtaError::Transport(_) => {
                "PCへの接続に失敗しました。PCが起動しbridgeが動いているか確認してください。"
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

/// OTAを実行する。Phase 4 (UI / Telegramトリガー) から呼ぶ公開関数。
///
/// 1. `GET /firmware/manifest` を署名付きリクエストで取得し、署名を検証する。
///    検証に失敗したらダウンロードへ進まず即座に中止する。
/// 2. `GET /firmware` でバイナリをストリーミング取得し、非activeスロットへ書く。
///    途中は `on_progress` で進捗バーを出し、ループを抜けた直後に刻みと無関係に
///    必ず1回呼んで100%にする (端数が閾値に届かず94%で止まった事故の再発防止)。
/// 3. 書きながら計算したSHA-256と受信サイズをmanifestと突き合わせ、
///    不一致ならboot切替をせず中止する。
/// 4. すべて成功したら `on_applying` で適用通知を出してからbootパーティションを
///    切り替えてrebootする。成功時はこの関数は戻らない。
///    通知は同期POSTのため、戻った時点で送信済みであり再起動で欠けない。
/// 5. 失敗時だけ `Err` で戻り、呼び出し側が結果文を送る。
///
/// `on_progress` と `on_applying` はどちらも戻り値を持たない。通知の失敗で
/// OTAを止めない方針であり、呼び出し側でエラーを握り潰す契約にしてある。
/// 完了通知も `editMessageText` で同一メッセージを書き換える前提
/// (新しいメッセージを増やさない)。100%バーと適用通知の文言は変えてあり、
/// Telegramの同一内容編集(400)にならない
/// (`pc_remote_signing::ota_applying_text` のコメント参照)。
///
/// # 前提
///
/// 呼び出し時点で他のTLS接続 (特にTelegram long polling のHTTPS接続) が
/// 開いていないこと。開いたままだとmbedTLSのヒープ不足で2本目の接続が
/// `ESP_ERR_HTTP_CONNECT` で失敗する。Phase 4側でpolling接続を閉じ切ってから
/// 呼ぶこと。詳細はmodule冒頭の「TLS同時接続の制約」を参照。
///
/// `pc_ip_address` はTelegram経由で実行時に変更できるため、`AppConfig` ではなく
/// 呼び出し側から都度渡してもらう (`bridge_client::send_command` と同じ扱い)。
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
/// `checks` の合否は `pc_remote_signing::boot_self_test_passed` が決める
/// (判定式はhostでテストしている)。通らなかったら何もせず `Ok(false)` を返す。
/// 自己診断が失敗しうる経路で無条件にマークすると、壊れたfirmwareが居座るため、
/// この関数の外側で条件を緩めて呼ばないこと。
///
/// pendingでない通常起動で呼んでも無害である。ESP-IDF v5.3.2 の
/// `esp_ota_mark_app_valid_cancel_rollback` は内部で
/// `esp_ota_current_ota_is_workable(true)` を呼び、実行中slotのstateがすでに
/// `VALID` のときはotadataへ書き込まず `ESP_OK` を返す(未pending時のno-op)。
/// 初回USB書き込み直後などotadata自体が無効な場合は `ESP_FAIL` が返るが、
/// その場合は次回起動時に再試行すればよく、呼び出し側はpanicせずログに留めること。
/// いずれの失敗も「次回起動で旧slotへ戻る」方向であり、実機をbrickしない。
///
/// 戻り値の `bool` は「今回validマークを試みたか」(自己診断の通過=true)。
/// マーク自体の成否は `Result` で返す。
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
/// `run_ota_update` の前段と、Telegram `/update` が確認前にversion/sizeを
/// 提示するための取得で共有する。確認時の表示と実行時の検証が同じ関数を
/// 通るため、表示と検証の食い違いが起きない。
/// (実行時は `run_ota_update` が改めて取得し直す。確認から確定までの間に
/// manifestが差し替わっても、検証を通ったものだけを書き込む。)
///
/// HTTP接続はこの関数内の `{}` スコープで閉じ切る。パースと署名検証は
/// 接続を持たない状態で行い、検証失敗時はバイナリ取得へ進まない。
pub fn fetch_verified_manifest(
    config: &AppConfig,
    pc_ip_address: &str,
) -> Result<OtaManifest, OtaError> {
    // HTTP接続は `with_signed_get` の中で閉じ切る。パースと署名検証は
    // 接続を持たない状態で行い、検証失敗時はバイナリ取得へ進まない。
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

/// 署名検証済みのmanifestだけを受け取り、バイナリを非activeスロットへ書く。
///
/// `manifest` は必ず `fetch_verified_manifest` で検証済みのものを渡すこと。
/// 未検証のmanifestで呼ぶと、攻撃者の用意したimageを書き込みかねない。
/// HTTP接続は `{}` スコープで閉じ切り、突き合わせは接続を持たない状態で行う。
/// size/sha256のどちらかが不一致なら `complete` せずに抜ける:
/// `EspOtaUpdate` のDropが `esp_ota_abort` し、boot partitionは切り替わらない
/// (`esp_ota_end` を呼んでいないためbootloaderはそのslotを新規imageとして扱わない)。
///
/// 進捗は `on_progress`、適用開始は `on_applying` で知らせる。どちらも戻り値を
/// 持たず、通知の失敗でOTAを止めない (呼び出し側で握り潰す契約)。
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
    // 直近で通知したパーセント。バイト数ではなく割合で刻むことで、
    // イメージのサイズによらず同じ回数だけバーが動く。
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
                // Issue #130-2: manifest申告を超える分はflashへ書かず即座に
                // 打ち切る。終端まで書いてから突き合わせで落とすと、slot上限まで
                // 無駄な消去・書き込みが続く。判定式はhostでテストできるよう
                // `config_validation` へ置いてある。早期returnで `update` が
                // Dropされ `esp_ota_abort` するため、boot切替は起きない。
                // 書く前に判定し、超過分を1バイトも書かない。
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
                    // 進捗の通知は補助であり、失敗してもOTAを中止しない。
                    // コールバック側でエラーを握り潰す契約にしてある(戻り値を
                    // 持たせない)。ここで `?` を使うと、Telegramが一時的に
                    // 応答しないだけで更新全体が巻き戻ることになる。
                    on_progress(manifest, received);
                }
            }
            Ok(())
        },
    )?;
    // 2本目の接続はここで閉じた。以降はflash済みデータの突き合わせだけを行う。

    // 刻みに届かない最後の端数を残さないよう、ループ後に必ず1回通知して
    // 100%にする。Issue #143 の実機では 1,388,544B を256KB刻みで送ると
    // 最終通知が 1,310,720B = 94%で止まり、残り 77,824B が閾値に届かず
    // 通知されないまま再起動した。割合ベースでも端数は必ず出るため、
    // 刻みを細かくしても解決しない。ループ内で既に100%を通知済みなら
    // 呼び出し側の同一内容ガードで送らない (Telegramの400回避)。
    // 通知は補助であり、失敗してもOTAを止めない (戻り値を持たない契約)。
    on_progress(manifest, received);
    println!("ota: download complete ({received}/{} bytes)", manifest.size);

    // size と sha256 の両方を見る。どちらか不一致なら `?` で抜け、
    // `update` のDrop (= abort) によりboot切替は行われない。
    verify_ota_image(manifest, received, &hashing.finish_hex())?;
    println!("ota: image verified ({received} bytes), activating");
    update
        .complete()
        .map_err(|e| OtaError::Ota(e.to_string()))?;
    // 検証・書き込み完了の通知。`edit_message_text` は応答を待つ同期POSTのため、
    // この呼び出しが戻った時点で送信は終わっており、この後の `restart()` で
    // 通知が欠けることはない。文言は100%バーと変えてある
    // (`pc_remote_signing::ota_applying_text` のコメント参照)。
    // 失敗しても再起動は止めない (戻り値を持たない契約)。
    on_applying(manifest);
    Ok(())
}

/// HMAC署名付きGETを送り、応答の読み取りを `f` に任せる。bodyは空
/// (`GET /firmware*` は読み取り専用で署名対象bodyは空。bridge側の
/// `verify_headers` も `b""` で検証する)。
///
/// `f` から戻った時点でclient/responseはdropされ、接続は完全に閉じる。
/// manifest取得とバイナリ取得はこの関数を別々に呼ぶため、同時には1本しか
/// 開かない (module冒頭の「TLS同時接続の制約」を参照)。
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
    // `f` の実行中だけ接続が開く。戻ったらclientごとdropして閉じ切る。
    f(&mut response)
}
