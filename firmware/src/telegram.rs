// Telegram Bot APIクライアント。外向きHTTPS long pollingで、受信portを開けずに
// スマホ操作を受け取る。
//
// 守るべき挙動:
//   - `from.id` が許可ユーザーIDと一致するupdateだけ処理する
//   - /reboot と /shutdown と /update は即実行せず、単回使用の確認nonceを発行する
//   - 確認は成功・失敗・期限切れのいずれでも消費し、再利用させない
//   - 起動直後の最初のgetUpdates結果はoffset更新だけにし、起動前に届いた古い命令を実行しない
//   - bot tokenとメッセージ内容をログへ出さない

use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use embedded_svc::http::client::Client as HttpClient;
use embedded_svc::http::Method;
use esp_idf_svc::http::client::{Configuration as HttpConfiguration, EspHttpConnection};
use serde_json::{json, Value};

use pc_remote_signing::AlertThrottle;

use crate::app_config::AppConfig;
use crate::board::Battery;
use crate::bridge_client::{self, PowerAction, PowerActionLabel};
use crate::net;
use crate::settings::RuntimeSettings;
use crate::telegram_root_ca::TELEGRAM_ROOT_CA_PEM;

/// ロック中でも受け付けるコマンド。これ以外は拒否する(default-deny)。
/// 追加するときは「ロック中の利用者に許してよいか」を必ず考えること。
const ALLOWED_WHILE_LOCKED: [&str; 4] = ["/status", "/settings", "/lock", "/unlock"];

const PLACEHOLDER_TOKEN: &str = "replace-with-your-telegram-bot-token";
const PLACEHOLDER_USER_ID: &str = "replace-with-your-telegram-user-id";

/// OTAで配るイメージの版と同じ値になるため、利用者が更新の成否を確認できる。
const FIRMWARE_VERSION: &str = env!("CARGO_PKG_VERSION");

const BACKOFF_MIN: Duration = Duration::from_secs(5);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
const RESPONSE_BUFFER: usize = 4096;
/// ESP32のヒープ保護。超過分は読まずにエラーとし、backoffへ回す。
const RESPONSE_MAX_BYTES: usize = 32 * 1024;

/// UIスレッドへ共有するTelegram状態。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Disabled,
    Polling,
    Error,
}

/// タッチUIとTelegramスレッドからの電源操作を直列化するロック。
pub type PowerLock = Arc<Mutex<()>>;

/// Telegram HTTPS接続の直列化ロック(Issue #127)。
///
/// ESP32のmbedTLSヒープでは実質同時に1本のTLS接続しか張れない。long poll保持中に
/// 2本目を開くと `ESP_ERR_HTTP_CONNECT` で失敗し、**通知が黙って消える**
/// (ログにも残らない)。pollingスレッドと通知スレッドで共有して直列化する。
///
/// トレードオフ: long poll中(最大 `long_poll_timeout+10` 秒)は通知がそのぶん遅れる。
/// 「遅れて届く」は許容し「黙って消える」は許さない、という選択。
///
/// **leafとして扱う。** 握ったまま他のロック(power/state)を取らず、他のロックの
/// 内側でも取らない。std Mutexは再入不可のため、握ったまま `send_message` 系を
/// 呼ぶと自己デッドロックする。送信箇所を増やすときもこの順序を守ること。
pub type HttpsLock = Arc<Mutex<()>>;

/// Telegram HTTPSの排他を取る。poisonしていても排他は維持する。
pub fn lock_https(https_lock: &HttpsLock) -> std::sync::MutexGuard<'_, ()> {
    https_lock.lock().unwrap_or_else(|e| e.into_inner())
}

/// 電源操作の排他を取る。poisonしていても排他は維持する。
///
/// `unwrap()` だと無関係なスレッドのpanicでUIループごと落ちる。守っているのは
/// 値型だけで壊れた状態を引き継ぐ心配がないため `into_inner()` で回復してよい
/// (このファイルの `lock_*` はすべて同じ扱い)。
pub fn lock_power(power_lock: &PowerLock) -> std::sync::MutexGuard<'_, ()> {
    power_lock.lock().unwrap_or_else(|e| e.into_inner())
}

/// UIスレッドへ共有するTelegram状態の排他を取る。poisonしていても排他は維持する。
pub fn lock_state(state: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}

/// UIループが読んだ最新のバッテリー状態。pollingスレッドはI2Cドライバを持たない
/// (AXP192はUIループ側の `axp` 経由でしか読めない)ため、読み取り結果の値だけを共有する。
pub type SharedBattery = Arc<Mutex<Option<Battery>>>;

/// 共有バッテリー状態の排他を取る。poisonしていても排他は維持する。
pub fn lock_battery(
    battery: &Mutex<Option<Battery>>,
) -> std::sync::MutexGuard<'_, Option<Battery>> {
    battery.lock().unwrap_or_else(|e| e.into_inner())
}

/// PCの起動指示後の待機開始時刻。`None`=待機なし。画面ボタンはUIループ、`/wake` は
/// pollingスレッドで入るため共有する。leaf扱い(握ったまま送信系を呼ばない)。
pub type SharedWakeWatch = Arc<Mutex<Option<Instant>>>;

/// **ガードを呼び出した文の外へ持ち出さないこと。** `match *lock_wake_watch(..)` の
/// 腕の中で取り直すと、`match` が終わるまでガードが解放されず自己デッドロックする
/// (実機で再現済み、Issue #182)。呼び出し側は下の3つの公開関数を使う。
fn lock_wake_watch(
    wake_watch: &Mutex<Option<Instant>>,
) -> std::sync::MutexGuard<'_, Option<Instant>> {
    wake_watch.lock().unwrap_or_else(|e| e.into_inner())
}

/// 起動指示後の待機の有無と開始時刻を読む。ガードはこの関数内で手放すため、
/// 呼び出し側がガード保持中の再ロック(自己デッドロック)を起こせない。
pub fn wake_watch_started(wake_watch: &SharedWakeWatch) -> Option<Instant> {
    *lock_wake_watch(wake_watch)
}

/// 起動指示後の待機を終わらせる。ガードはこの関数内で手放す。
pub fn clear_wake_watch(wake_watch: &SharedWakeWatch) {
    *lock_wake_watch(wake_watch) = None;
}

/// 起動指示後の待機を開始・更新する。WOL送信に成功した呼び出し側だけが呼ぶ。
///
/// - すでにPCがオンなら待機しない(残っていた待機があれば終わらせる)。
/// - オフなら待機を開始する。待機中の再指示は開始時刻を今に置き換える
///   (=期限を最後の指示から数え直す)。
pub fn begin_wake_watch(wake_watch: &SharedWakeWatch, pc_online: bool) {
    // `begin()` の結果は `pc_online` だけで決まり直前の待機有無には依らないため、
    // 読み取りのためのロックは取らず書き込みの1回だけにする。
    let (next, _) = wake_check::WakeWatch::idle().begin(pc_online);
    *lock_wake_watch(wake_watch) = next.waiting.then(Instant::now);
}

/// 操作ロック。有効な間はWAKE / REBOOT / SHUTDOWNを一切実行しない。
/// Telegramの `/lock` `/unlock` で切り替え、本体パネル操作にも効く。
///
/// 状態はメモリ上だけで保持し、M5Stackを再起動すると解除される。再起動できる
/// 位置に居るなら本人が近くに居るとみなせるため、永続化はしない。
#[derive(Clone, Default)]
pub struct OperationLock(Arc<AtomicBool>);

impl OperationLock {
    pub fn is_locked(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    fn set(&self, locked: bool) {
        self.0.store(locked, Ordering::Relaxed);
    }
}

/// M5Stackの起動通知の文面。`firmware/Cargo.toml` の `version` が正本で、
/// `/status` のバージョン表示(`FIRMWARE_VERSION`)と同じ値を使う。
/// OTA後に新版が動いているかの確認と、M5Stackの予期しない再起動の検知が目的。
/// 用語は `docs/glossary.md` が正本(M5Stack自身の立ち上がりは「M5Stackが起動しました」)。
pub fn boot_notification_text() -> String {
    format!("M5Stackが起動しました ({FIRMWARE_VERSION})")
}

pub fn is_configured(config: &AppConfig) -> bool {
    // trimしないと ` token ` のような値がplaceholder判定をすり抜ける(Issue #130-1)。
    let token = config.telegram_bot_token.trim();
    let user_id =
        config_validation::normalize_telegram_user_id(&config.telegram_allowed_user_id);
    !token.is_empty()
        && token != PLACEHOLDER_TOKEN
        && !user_id.is_empty()
        && user_id != PLACEHOLDER_USER_ID
}

/// ピン留めしたルートCAをesp-tlsのglobal CA storeへ登録する。
/// HTTPSリクエスト前に1回だけ実行する。
fn install_root_ca() -> Result<(), Box<dyn Error>> {
    let pem = TELEGRAM_ROOT_CA_PEM.as_bytes();
    esp_idf_sys::esp!(unsafe { esp_idf_sys::esp_tls_init_global_ca_store() })?;
    esp_idf_sys::esp!(unsafe {
        esp_idf_sys::esp_tls_set_global_ca_store(pem.as_ptr(), pem.len() as u32)
    })?;
    Ok(())
}

/// `Once` は使わない。失敗しても「完了」扱いになり、heap不足等で1回失敗すると
/// 以降どのスレッドも再試行できずCA store未設定のままHTTPSを使ってしまう。
static ROOT_CA_INSTALLED: AtomicBool = AtomicBool::new(false);
static ROOT_CA_LOCK: Mutex<()> = Mutex::new(());

fn ensure_root_ca() -> Result<(), Box<dyn Error>> {
    let _guard = ROOT_CA_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if ROOT_CA_INSTALLED.load(Ordering::Acquire) {
        return Ok(());
    }
    install_root_ca()?;
    ROOT_CA_INSTALLED.store(true, Ordering::Release);
    Ok(())
}

/// private chatではchat_idがuser_idと一致するため、許可ユーザーIDをそのまま
/// 送信先として使う。能動送信(pollingの応答ではない通知)で必要になる。
fn allowed_chat_id(config: &AppConfig) -> Option<i64> {
    // Issue #130-1: 正規化を `config_validation` に一本化する(3箇所が別々にtrimするとずれる)。
    config_validation::parse_telegram_user_id(&config.telegram_allowed_user_id)
}

/// Bot APIへのHTTP呼び出し。pollingスレッドと通知スレッドの両方から使う。
/// 両スレッドは同じ `HttpsLock` を共有し、TLS接続を直列化する(Issue #127)。
struct Api {
    config: Arc<AppConfig>,
    https: HttpsLock,
}

/// 確認待ちの操作。電源操作(REBOOT/SHUTDOWN)と設定変更(/set_*)とfirmware更新
/// (/update)が、同じ「単回使用nonce付き確認」フローを共有する。同時に保留できるのは1件のみ。
enum PendingKind {
    Power(PowerAction),
    Config(ConfigChange),
    /// 確認時に提示した版を保持し、実行直前に取り直したmanifestと突き合わせる。
    /// 正規に署名された別版への差し替えは署名検証をすり抜けるため(Issue #180)。
    FirmwareUpdate {
        version: String,
    },
}

impl PendingKind {
    fn label_ja(&self) -> String {
        match self {
            PendingKind::Power(action) => action.label_ja().to_string(),
            PendingKind::Config(change) => change.label_ja().to_string(),
            PendingKind::FirmwareUpdate { .. } => "firmware更新".to_string(),
        }
    }
}

/// ボタンから届くcallback_dataの種類。値そのもの(新しいIP等)は載せず、
/// 識別子とnonceだけにする(Codexレビュー方針)。実際の中身は`pending`側が持つ。
enum Callback {
    /// `confirm:<target>:<nonce>` / `cancel:<target>:<nonce>`
    Decision {
        confirm: bool,
        target: CallbackTarget,
        nonce: String,
    },
    /// `setedit:<slug>`。設定値の入力を開始する。
    EditSetting(SettingKind),
    /// `lock:on` / `lock:off`。操作ロックの切り替え。
    SetLock(bool),
}

impl Callback {
    /// ログ用の種別名。nonceや値は含めない。
    fn log_label(&self) -> &'static str {
        match self {
            Callback::Decision { confirm: true, .. } => "confirm",
            Callback::Decision { confirm: false, .. } => "cancel",
            Callback::EditSetting(_) => "setedit",
            Callback::SetLock(true) => "lock:on",
            Callback::SetLock(false) => "lock:off",
        }
    }
}

/// callback_dataが指す確認の対象。値そのもの(新しいIP等)はcallback_dataに
/// 入れない(Codexレビュー方針)。nonceだけで、実際の中身は`pending`側が持つ。
enum CallbackTarget {
    Power(PowerAction),
    Config,
    FirmwareUpdate,
}

impl CallbackTarget {
    /// ボタンが指す対象と、実際に保留されている確認が一致するか。
    /// 一致しないボタン(古いメッセージ、別種類の保留との衝突)は無効として扱う。
    fn matches(&self, kind: &PendingKind) -> bool {
        match (self, kind) {
            (CallbackTarget::Power(a), PendingKind::Power(b)) => a == b,
            (CallbackTarget::Config, PendingKind::Config(_)) => true,
            (CallbackTarget::FirmwareUpdate, PendingKind::FirmwareUpdate { .. }) => true,
            _ => false,
        }
    }
}

/// 変更できる設定項目の識別子。値を持たないので、ボタンの`callback_data`と
/// 「いまどの項目の入力を待っているか」の記録に使える。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingKind {
    PcIp,
    WolPort,
    Brightness,
}

impl SettingKind {
    const ALL: [SettingKind; 3] = [
        SettingKind::PcIp,
        SettingKind::WolPort,
        SettingKind::Brightness,
    ];

    fn slug(self) -> &'static str {
        match self {
            SettingKind::PcIp => "pc_ip",
            SettingKind::WolPort => "wol_port",
            SettingKind::Brightness => "brightness",
        }
    }

    fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.slug() == slug)
    }

    fn label_ja(self) -> &'static str {
        match self {
            SettingKind::PcIp => "PC IPアドレス",
            SettingKind::WolPort => "WOLポート",
            SettingKind::Brightness => "画面の明るさ",
        }
    }

    /// 入力例。入力欄のplaceholderと案内文へ出す。
    fn example(self) -> &'static str {
        match self {
            SettingKind::PcIp => "192.168.1.50",
            SettingKind::WolPort => "9",
            SettingKind::Brightness => "80",
        }
    }

    fn current(self, settings: &RuntimeSettings) -> String {
        match self {
            SettingKind::PcIp => settings.pc_ip_address(),
            SettingKind::WolPort => settings.wol_port().to_string(),
            SettingKind::Brightness => settings.brightness_percent().to_string(),
        }
    }

    /// 入力値を検証し、確認待ちの変更内容へ変換する。
    fn parse(self, raw: &str) -> Result<ConfigChange, String> {
        match self {
            SettingKind::PcIp => {
                config_validation::validate_ipv4(raw).map(ConfigChange::PcIpAddress)
            }
            SettingKind::WolPort => {
                config_validation::validate_wol_port(raw).map(ConfigChange::WolPort)
            }
            SettingKind::Brightness => {
                config_validation::validate_brightness_percent(raw).map(ConfigChange::Brightness)
            }
        }
    }
}

/// 値の入力を待っている状態。ボタンを押してから次の1通を値として受け取る。
struct PendingInput {
    kind: SettingKind,
    expires_at: Instant,
}

/// `/set_ip` 等で保留される設定変更。値は検証済みのものだけがここに入る
/// (確認nonce発行前に `config_validation` を通す)。
#[derive(Clone)]
enum ConfigChange {
    PcIpAddress(String),
    WolPort(u16),
    Brightness(u8),
}

impl ConfigChange {
    fn label_ja(&self) -> &'static str {
        match self {
            ConfigChange::PcIpAddress(_) => "PC IPアドレス",
            ConfigChange::WolPort(_) => "WOLポート",
            ConfigChange::Brightness(_) => "画面の明るさ",
        }
    }

    fn display_value(&self) -> String {
        match self {
            ConfigChange::PcIpAddress(value) => value.clone(),
            ConfigChange::WolPort(value) => value.to_string(),
            ConfigChange::Brightness(value) => value.to_string(),
        }
    }

    /// NVSへ永続化し、成功したときだけメモリ上の値も更新する。
    /// 明るさの画面への反映はUIループ側(pollingスレッドはI2Cドライバを持たない)。
    fn apply(&self, settings: &RuntimeSettings) -> Result<(), esp_idf_sys::EspError> {
        match self {
            ConfigChange::PcIpAddress(value) => settings.set_pc_ip_address(value.clone()),
            ConfigChange::WolPort(value) => settings.set_wol_port(*value),
            ConfigChange::Brightness(value) => settings.set_brightness_percent(*value),
        }
    }
}

/// 確認待ちは**同時に1件だけ**。操作ごとにスロットを分けると古い確認が生き続け、
/// 「いつ押されるか分からないボタン」が増える。最後の1件だけを有効にする。
struct Pending {
    kind: PendingKind,
    nonce: String,
    expires_at: Instant,
}

pub struct Client {
    last_update_id: i64,
    initial_sync_done: bool,
    pending: Option<Pending>,
    /// 設定変更ボタンを押した後、値の入力を待っている項目。
    pending_input: Option<PendingInput>,
    power_lock: PowerLock,
    operation_lock: OperationLock,
    config: Arc<AppConfig>,
    settings: Arc<RuntimeSettings>,
    api: Api,
    /// UIループが更新する最新のバッテリー状態(`/status` で読むだけ)。
    battery: SharedBattery,
    /// 起動指示後の待機開始時刻(UIループと共有。`/wake` で書き、UIループで読む)。
    wake_watch: SharedWakeWatch,
    /// 未許可アクセスの検知数と、直近でアラートを送った時刻。
    unauthorized_alerts: AlertThrottle,
}

impl Api {
    /// URLにはbot tokenが入るため、絶対にログへ出さない。
    fn api_url(&self, method: &str) -> String {
        format!(
            "https://api.telegram.org/bot{}/{method}",
            self.config.telegram_bot_token
        )
    }

    fn http_client(&self) -> Result<HttpClient<EspHttpConnection>, Box<dyn Error>> {
        Ok(HttpClient::wrap(EspHttpConnection::new(
            &HttpConfiguration {
                use_global_ca_store: true,
                timeout: Some(Duration::from_secs(
                    self.config.telegram_long_poll_timeout_seconds as u64 + 10,
                )),
                buffer_size: Some(RESPONSE_BUFFER),
                ..Default::default()
            },
        )?))
    }

    /// Bot APIへJSONをPOSTする。URLとbodyはtokenや本文を含み得るためログへ出さない。
    fn post_json(&self, method: &str, body: &Value) -> Result<(), Box<dyn Error>> {
        // Issue #127: 保持中はHTTP送受信だけを行い、他のロックは取らない(leaf扱い)。
        let _https_guard = lock_https(&self.https);

        let mut client = self.http_client()?;
        self.post_via(&mut client, method, body)
    }

    /// 既存の接続ハンドルでJSONを1回POSTする。**呼び出し側が `HttpsLock` を握ること。**
    /// ログにはmethod名とミリ秒だけを出す(URLはtoken、bodyは本文を含む)。
    /// 未読応答は次回 `request` 時に `initiate_request` が捨てる。
    fn post_via(
        &self,
        client: &mut HttpClient<EspHttpConnection>,
        method: &str,
        body: &Value,
    ) -> Result<(), Box<dyn Error>> {
        use esp_idf_svc::io::Write;

        let started = Instant::now();
        let payload = serde_json::to_string(body)?;
        let url = self.api_url(method);
        let content_length = payload.len().to_string();
        let headers = [
            ("Content-Type", "application/json"),
            ("Content-Length", content_length.as_str()),
        ];
        let mut request = client.request(Method::Post, &url, &headers)?;
        request.write_all(payload.as_bytes())?;
        request.flush()?;
        let response = request.submit()?;
        let status = response.status();
        println!(
            "telegram: {method} took {}ms",
            started.elapsed().as_millis()
        );
        if !(200..300).contains(&status) {
            // 呼び出し側が再送要否を判断できるよう、失敗はエラーとして返す。
            return Err(format!("telegram {method} failed: {status}").into());
        }
        Ok(())
    }

    /// `sendMessage` を送り、作られたメッセージの `message_id` を返す。
    ///
    /// 進捗表示のように、後から `editMessageText` で同じメッセージを書き換える
    /// 用途で使う。応答は数百Bなので上限を小さく取り、超えたら読み捨てる
    /// (ヒープを守るため。本文が要るのは `message_id` だけ)。
    fn send_message_returning_id(&self, chat_id: i64, text: &str) -> Option<i64> {
        use esp_idf_svc::io::Write;

        let _https_guard = lock_https(&self.https);

        let started = Instant::now();
        let body = json!({ "chat_id": chat_id, "text": text });
        let payload = serde_json::to_string(&body).ok()?;
        let url = self.api_url("sendMessage");
        let mut client = self.http_client().ok()?;
        let content_length = payload.len().to_string();
        let headers = [
            ("Content-Type", "application/json"),
            ("Content-Length", content_length.as_str()),
        ];
        let mut request = client.request(Method::Post, &url, &headers).ok()?;
        request.write_all(payload.as_bytes()).ok()?;
        request.flush().ok()?;
        let mut response = request.submit().ok()?;
        let status = response.status();
        // 本文・chat_idは出さない。method名とミリ秒だけ(Issue #163 c0)。
        println!(
            "telegram: sendMessage took {}ms",
            started.elapsed().as_millis()
        );
        if !(200..300).contains(&status) {
            println!("telegram: sendMessage failed: {status}");
            return None;
        }

        const MAX_BYTES: usize = 2048;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 256];
        loop {
            let read = response.read(&mut chunk).ok()?;
            if read == 0 || buf.len() + read > MAX_BYTES {
                break;
            }
            buf.extend_from_slice(&chunk[..read]);
        }
        let parsed: Value = serde_json::from_slice(&buf).ok()?;
        parsed["result"]["message_id"].as_i64()
    }

    /// 既存メッセージの本文を差し替える。進捗表示の更新に使う。
    ///
    /// 失敗しても呼び出し側は処理を続けること。進捗表示は補助であり、
    /// これを理由にOTAを中止しない。Telegramは同一内容への編集を400にするため、
    /// 呼び出し側は内容が変わったときだけ呼ぶ。
    fn edit_message_text(&self, chat_id: i64, message_id: i64, text: &str) -> bool {
        match self.post_json(
            "editMessageText",
            &json!({ "chat_id": chat_id, "message_id": message_id, "text": text }),
        ) {
            Ok(()) => true,
            Err(e) => {
                println!("telegram: editMessageText failed: {e}");
                false
            }
        }
    }

    /// 送信できたらtrue。定期レポートのように「落としたくない」通知が
    /// 再送を判断できるようにする。
    ///
    /// エラー文言にURL(bot tokenを含む)は入らない。`post_json`が組み立てる
    /// エラーはmethod名とHTTPステータスのみ。
    fn send_message(&self, chat_id: i64, text: &str) -> bool {
        match self.post_json("sendMessage", &json!({ "chat_id": chat_id, "text": text })) {
            Ok(()) => true,
            Err(e) => {
                println!("telegram: sendMessage failed: {e}");
                false
            }
        }
    }

    /// Telegramの確認ボタンを1行で送る。callback_dataはTelegram上限内に収める。
    /// インラインキーボード付きで送る。`rows` は行の配列。
    fn send_message_with_keyboard(&self, chat_id: i64, text: &str, rows: Value) {
        if let Err(e) = self.post_json(
            "sendMessage",
            &json!({
                "chat_id": chat_id,
                "text": text,
                "reply_markup": { "inline_keyboard": rows }
            }),
        ) {
            println!("telegram: sendMessage(keyboard) failed: {e}");
        }
    }

    fn send_message_with_confirm_buttons(
        &self,
        chat_id: i64,
        text: &str,
        confirm_label: &str,
        confirm_data: &str,
        cancel_data: &str,
    ) {
        self.send_message_with_keyboard(
            chat_id,
            text,
            json!([[
                { "text": confirm_label, "callback_data": confirm_data },
                { "text": "キャンセル", "callback_data": cancel_data },
            ]]),
        );
    }

    /// 返信欄を開いた状態でプロンプトを送る。値の入力を1往復で終わらせるため、
    /// コマンドを打ち直させずTelegram側の返信UIへ誘導する。
    fn send_force_reply(&self, chat_id: i64, text: &str, placeholder: &str) {
        if let Err(e) = self.post_json(
            "sendMessage",
            &json!({
                "chat_id": chat_id,
                "text": text,
                "reply_markup": {
                    "force_reply": true,
                    "input_field_placeholder": placeholder,
                }
            }),
        ) {
            println!("telegram: sendMessage(force_reply) failed: {e}");
        }
    }

    /// callback_queryへ応答し、Telegramクライアント側の読み込み表示を終わらせる。
    fn answer_callback_query(&self, id: &str, text: &str) {
        let mut body = json!({ "callback_query_id": id });
        if !text.is_empty() {
            body["text"] = json!(text);
        }
        // 失敗を握り潰すと「ボタンを押しても何も起きない」だけになり、
        // 原因の切り分けができない。応答本文は出さず、失敗の事実だけ残す。
        if let Err(e) = self.post_json("answerCallbackQuery", &body) {
            println!("telegram: answerCallbackQuery failed: {e}");
        }
    }

    /// `answerCallbackQuery` と `sendMessage` を1接続で連続送信する(Issue #163)。
    /// 接続を張り直さないぶんハンドシェイク1回分速い。同時TLSは1本のまま。
    /// `chat_id` が0のときはanswerだけ送る。
    fn answer_callback_query_and_send(
        &self,
        id: &str,
        answer_text: &str,
        chat_id: i64,
        send_text: &str,
    ) {
        let _https_guard = lock_https(&self.https);
        let mut client = match self.http_client() {
            Ok(client) => client,
            Err(e) => {
                println!("telegram: answer+send client failed: {e}");
                return;
            }
        };
        let mut answer_body = json!({ "callback_query_id": id });
        if !answer_text.is_empty() {
            answer_body["text"] = json!(answer_text);
        }
        // 失敗を握り潰すと「ボタンを押しても何も起きない」だけになり、
        // 原因の切り分けができない。応答本文は出さず、失敗の事実だけ残す。
        if let Err(e) = self.post_via(&mut client, "answerCallbackQuery", &answer_body) {
            println!("telegram: answerCallbackQuery failed: {e}");
            // 送信途中の失敗では接続の状態が不定(`State::Request` のまま残り得る)
            // のため、2通目は新しいハンドルで送る(従来の挙動へ切り戻す)。
            // 応答を受け取っての失敗(`State::Response`)なら使い回せたが、
            // 区別せず作り直すほうが単純で安全。
            client = match self.http_client() {
                Ok(client) => client,
                Err(e) => {
                    println!("telegram: sendMessage skipped: {e}");
                    return;
                }
            };
        }
        if chat_id != 0 {
            if let Err(e) = self.post_via(
                &mut client,
                "sendMessage",
                &json!({ "chat_id": chat_id, "text": send_text }),
            ) {
                println!("telegram: sendMessage failed: {e}");
            }
        }
    }
}

/// 定期レポートの時刻判定を行う間隔。長すぎると送信時刻がずれ、短すぎると
/// スレッドが無駄に起きる。1分あれば時刻単位の判定には十分。
const DAILY_REPORT_CHECK_INTERVAL: Duration = Duration::from_secs(60);

/// 1日1回の定期レポートの送信判定。
///
/// SNTP同期後のwall clockから「ローカル時刻の何日目か」と「何時か」を整数演算で
/// 求める。tzデータベースを持たない環境なので、設定のUTCオフセットだけを使う。
struct DailyReport {
    /// 最後に送った日(UNIX epochからのローカル日数)。同じ日には二度送らない。
    last_sent_day: Option<i64>,
}

impl DailyReport {
    fn new(config: &AppConfig) -> Self {
        // 起動時点で既に送信時刻を過ぎているなら、その日の分は送信済みとして扱う。
        // M5Stackの再起動のたびに同じ日のレポートが届くのを防ぐため。
        //
        // まだ送信時刻より前なら未送信のままにする。ここで無条件に送信済みへ
        // すると、送信時刻の前にM5Stackを再起動しただけでその日の分が飛んでしまう
        // (例: 8時に再起動 → 9時のレポートが翌日まで来ない)。
        let last_sent_day = Self::local_now(config)
            .and_then(|(day, hour)| (hour >= config.daily_report_hour).then_some(day));
        Self { last_sent_day }
    }

    /// (ローカル日数, ローカル時)を返す。NTP未同期なら None。
    fn local_now(config: &AppConfig) -> Option<(i64, i64)> {
        let unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs() as i64;
        if !net::is_ntp_synced(unix) {
            return None;
        }
        let local = unix + config.timezone_offset_hours * 3600;
        Some((local.div_euclid(86_400), local.rem_euclid(86_400) / 3600))
    }

    /// 送信すべき時刻なら(対象日, レポート本文)を返す。
    ///
    /// ここでは「送信済み」にしない。送信に失敗した日を既済にしてしまうと、
    /// その日のレポートが黙って落ちる。成功後に`mark_sent()`を呼ぶこと。
    fn due_report(&self, config: &AppConfig, settings: &RuntimeSettings) -> Option<(i64, String)> {
        if !(0..=23).contains(&config.daily_report_hour) {
            return None;
        }
        let (day, hour) = Self::local_now(config)?;
        if hour != config.daily_report_hour || self.last_sent_day == Some(day) {
            return None;
        }

        let online =
            net::check_pc_online(&settings.pc_status_addr(), net::STATUS_PROBE_TIMEOUT);
        Some((
            day,
            format!("定期レポート\nPC: {}", net::pc_online_label_ja(online)),
        ))
    }

    /// 送信に成功した日を記録する。以降その日は送らない。
    fn mark_sent(&mut self, day: i64) {
        self.last_sent_day = Some(day);
    }
}

/// UIスレッドからTelegramへ能動的に通知を送るためのハンドル。
///
/// 送信はHTTPSで数秒かかり得るため、UIループを止めないよう専用スレッドへ
/// channelで渡す。送信スレッドが落ちていても`notify`は失敗を無視する。
#[derive(Clone)]
pub struct Notifier {
    tx: Arc<mpsc::Sender<String>>,
}

impl Notifier {
    pub fn notify(&self, text: String) {
        let _ = self.tx.send(text);
    }
}

/// 通知送信スレッドを起動し、UIスレッド用のハンドルを返す。
/// Telegram未設定、または許可ユーザーIDがchat_idとして使えない場合はNone。
/// `https` はpollingスレッドと共有するTLS直列化ロック(Issue #127)。
pub fn start_notifier(
    config: Arc<AppConfig>,
    settings: Arc<RuntimeSettings>,
    https: HttpsLock,
) -> Option<Notifier> {
    if !is_configured(config.as_ref()) {
        return None;
    }
    let chat_id = allowed_chat_id(config.as_ref())?;

    let (tx, rx) = mpsc::channel::<String>();
    let tx = Arc::new(tx);
    // 通知スレッド自身が送信側を保持して、切断時の終了を妨げないようにする。
    let weak_tx = Arc::downgrade(&tx);
    let api = Api { config, https };
    let spawned = std::thread::Builder::new()
        .stack_size(12 * 1024)
        .spawn(move || {
            let mut schedule = DailyReport::new(&api.config);
            let started = Instant::now();
            let mut firmware_schedule = pc_remote_signing::FirmwareCheckSchedule::default();
            let mut firmware_notice = pc_remote_signing::FirmwareNotice::default();
            loop {
                // 定期レポートの時刻判定のため、通知が無くても定期的に起きる。
                let queued = match rx.recv_timeout(DAILY_REPORT_CHECK_INTERVAL) {
                    Ok(text) => Some(text),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    // 送信側(UIスレッド)が全て落ちた場合は終了する。
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                };

                let (next_schedule, due) = firmware_schedule.poll(started.elapsed().as_secs());
                firmware_schedule = next_schedule;
                if due {
                    let pc_ip_address = settings.pc_ip_address();
                    let manifest = crate::ota::fetch_verified_manifest(&api.config, &pc_ip_address);
                    if manifest.is_err() {
                        println!("ota: firmware availability check failed");
                    }
                    let offered = manifest.as_ref().ok().map(|value| value.version.as_str());
                    let (next_notice, notify) = firmware_notice.observe(FIRMWARE_VERSION, offered);
                    firmware_notice = next_notice;
                    if notify {
                        if let (Some(tx), Some(version)) = (weak_tx.upgrade(), offered) {
                            Notifier { tx }.notify(pc_remote_signing::firmware_available_text(
                                FIRMWARE_VERSION,
                                version,
                            ));
                        }
                    }
                }

                // CA storeの確認は`due_report()`より前に行う。`due_report()`は
                // 返した時点で「その日は送信済み」と記録するため、後段で失敗すると
                // その日のレポートが黙って落ちる。
                if let Err(e) = ensure_root_ca() {
                    println!("telegram: root CA install failed: {e}");
                    continue;
                }

                if let Some(text) = queued {
                    // Issue #127: 以前は戻り値を捨てており、TLS衝突時の失敗が
                    // 黙って消えた。直列化した今も送信自体は失敗し得るため、
                    // 失敗の事実だけログへ残す(本文・tokenは出さない)。
                    if !api.send_message(chat_id, &text) {
                        println!("telegram: queued notify failed");
                    }
                }
                // 送信できた日だけ既済にする。失敗した場合は次のループで
                // 再送する(送信時刻のうちは何度でも試す)。
                if let Some((day, text)) = schedule.due_report(&api.config, &settings) {
                    if api.send_message(chat_id, &text) {
                        schedule.mark_sent(day);
                    }
                }
            }
        });

    match spawned {
        Ok(_) => Some(Notifier { tx }),
        Err(e) => {
            println!("telegram: failed to start notifier thread: {e}");
            None
        }
    }
}

impl Client {
    pub fn new(
        power_lock: PowerLock,
        operation_lock: OperationLock,
        config: Arc<AppConfig>,
        settings: Arc<RuntimeSettings>,
        https: HttpsLock,
        battery: SharedBattery,
        wake_watch: SharedWakeWatch,
    ) -> Self {
        Self {
            last_update_id: 0,
            initial_sync_done: false,
            pending: None,
            pending_input: None,
            power_lock,
            operation_lock,
            api: Api {
                config: Arc::clone(&config),
                https,
            },
            config,
            settings,
            battery,
            wake_watch,
            // 抑制ポリシー(閾値・間隔)はbridgeと共有する。
            unauthorized_alerts: AlertThrottle::default(),
        }
    }

    /// 未許可ユーザーからのアクセスを記録し、閾値を超えたらアラートを送る。
    ///
    /// 通知には送信者のIDやメッセージ本文を一切含めない。相手が自由に決められる
    /// 文字列をそのまま自分のチャットへ流すと、なりすましや誘導の材料になるため。
    fn record_unauthorized_access(&mut self) {
        let Some(count) = self.unauthorized_alerts.record(Instant::now()) else {
            return;
        };
        println!("telegram: unauthorized access detected ({count})");

        if let Some(chat_id) = allowed_chat_id(self.config.as_ref()) {
            self.api.send_message(
                chat_id,
                &format!(
                    "未許可ユーザーからのアクセスを{count}回検知しました。\n操作は実行されていません。"
                ),
            );
        }
    }

    fn generate_nonce() -> String {
        const CHARSET: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
        (0..6)
            .map(|_| {
                let r = unsafe { esp_idf_sys::esp_random() } as usize;
                CHARSET[r % CHARSET.len()] as char
            })
            .collect()
    }

    fn status_text(&self) -> String {
        let online =
            net::check_pc_online(&self.settings.pc_status_addr(), net::STATUS_PROBE_TIMEOUT);
        let battery_line = match *lock_battery(&self.battery) {
            Some(battery) => {
                let state = battery::classify(battery.charging, battery.powered);
                battery::status_ja(battery.percent, state)
            }
            None => "バッテリー: 不明".to_string(),
        };
        let locked = self.operation_lock.is_locked();
        // ロック中・PCがオフなら結果を使わないので、bridgeへの800msの待ちを作らない。
        let bridge_online = !locked
            && online
            && bridge_client::check_bridge_online(
                self.config.as_ref(),
                &self.settings.pc_ip_address(),
            );
        let snapshot = pc_remote_signing::PcStatusSnapshot {
            pc_online: online,
            bridge_online,
            locked,
        };
        pc_remote_signing::status_text_ja(
            snapshot.into(),
            net::pc_online_label_ja(online),
            &battery_line,
            FIRMWARE_VERSION,
        )
    }

    /// `/settings` の応答。現在値と、そこから実行できる操作をボタンで出す。
    ///
    /// `/set_*` と `/lock` `/unlock` はTelegramのコマンド一覧(setMyCommands)へ
    /// 登録しない。日常的に使うのは電源操作だけで、設定変更とロックまで一覧へ出すと
    /// 選びにくくなる。またコマンド一覧から `/set_ip` をタップすると引数なしで
    /// 送信されてしまい、値の入力手段としては使えない。代わりにこのメニューを
    /// 入口にして、ボタン→値の入力→確認、の流れで完結させる。
    fn send_settings_menu(&self, chat_id: i64) {
        let locked = self.operation_lock.is_locked();
        let (pc_ip_address, wol_port, brightness_percent) = self.settings.snapshot();
        let text = format!(
            "現在の設定\n\
             ・PC IPアドレス: {pc_ip_address}\n\
             ・WOLポート: {wol_port}\n\
             ・画面の明るさ: {brightness_percent}%\n\
             ・操作ロック: {}\n\
             \n変更したい項目のボタンを押してください。",
            if locked { "ロック中" } else { "解除中" }
        );

        let mut rows: Vec<Value> = SettingKind::ALL
            .into_iter()
            .map(|kind| {
                json!([{
                    "text": format!("{}を変更", kind.label_ja()),
                    "callback_data": format!("setedit:{}", kind.slug()),
                }])
            })
            .collect();
        rows.push(if locked {
            json!([{ "text": "ロックを解除", "callback_data": "lock:off" }])
        } else {
            json!([{ "text": "電源操作をロック", "callback_data": "lock:on" }])
        });

        self.api
            .send_message_with_keyboard(chat_id, &text, json!(rows));
    }

    /// 値の入力待ちを開始する。次に届いた非コマンドのテキストを値として扱う。
    fn start_setting_input(&mut self, chat_id: i64, kind: SettingKind) {
        self.pending_input = Some(PendingInput {
            kind,
            expires_at: Instant::now()
                + Duration::from_secs(self.config.telegram_confirm_ttl_secs),
        });
        let text = format!(
            "{}の新しい値を送信してください。\n現在: {}\n例: {}",
            kind.label_ja(),
            kind.current(&self.settings),
            kind.example()
        );
        self.api.send_force_reply(chat_id, &text, kind.example());
    }

    /// 入力待ちを取り出す。期限切れなら消費だけして`None`を返す。
    fn take_pending_input(&mut self) -> Option<SettingKind> {
        let pending = self.pending_input.take()?;
        (Instant::now() < pending.expires_at).then_some(pending.kind)
    }

    /// 入力された値を検証し、通れば既存の確認フロー(確認/キャンセルボタン)へ渡す。
    /// 検証に落ちた場合は入力待ちを張り直し、打ち直せるようにする。
    fn handle_setting_input(&mut self, chat_id: i64, kind: SettingKind, raw: &str) {
        match kind.parse(raw) {
            Ok(change) => {
                let current = kind.current(&self.settings);
                self.request_config_confirmation(chat_id, change, current);
            }
            Err(message) => {
                self.pending_input = Some(PendingInput {
                    kind,
                    expires_at: Instant::now()
                        + Duration::from_secs(self.config.telegram_confirm_ttl_secs),
                });
                self.api.send_force_reply(chat_id, &message, kind.example());
            }
        }
    }

    /// `/set_*` の直接入力。引数が無ければボタンと同じ入力待ちへ倒す。
    fn handle_set_command(&mut self, chat_id: i64, kind: SettingKind, args: &str) {
        if args.is_empty() {
            self.start_setting_input(chat_id, kind);
            return;
        }
        self.handle_setting_input(chat_id, kind, args);
    }

    /// 操作ロックを切り替える。ロック時は保留中の確認と入力待ちも捨てる
    /// (ロック直前に発行したものを解除後に使い回せてしまうのを防ぐ)。
    fn set_operation_lock(&mut self, locked: bool) {
        self.operation_lock.set(locked);
        if locked {
            self.pending = None;
            self.pending_input = None;
        }
    }

    fn request_confirmation(&mut self, chat_id: i64, action: PowerAction) {
        let nonce = Self::generate_nonce();
        self.pending = Some(Pending {
            kind: PendingKind::Power(action),
            nonce: nonce.clone(),
            expires_at: Instant::now() + Duration::from_secs(self.config.telegram_confirm_ttl_secs),
        });

        let confirm_command = match action {
            PowerAction::Reboot => "/confirm_reboot",
            PowerAction::Shutdown => "/confirm_shutdown",
        };
        let text = format!(
            "PCを{}しますか？\nボタンを押すと実行します。\n手入力する場合: {confirm_command} {nonce}",
            action.label_ja()
        );
        let confirm_data = format!("confirm:{}:{nonce}", action.slug());
        let cancel_data = format!("cancel:{}:{nonce}", action.slug());
        self.api.send_message_with_confirm_buttons(
            chat_id,
            &text,
            action.label_ja(),
            &confirm_data,
            &cancel_data,
        );
    }

    /// 検証済みの設定変更に対して確認を発行する。`current_value` は変更前の値
    /// (確認メッセージの「現在」欄に出すだけで、検証や書き込みには使わない)。
    fn request_config_confirmation(&mut self, chat_id: i64, change: ConfigChange, current_value: String) {
        let nonce = Self::generate_nonce();
        let label = change.label_ja();
        let new_value = change.display_value();
        self.pending = Some(Pending {
            kind: PendingKind::Config(change),
            nonce: nonce.clone(),
            expires_at: Instant::now() + Duration::from_secs(self.config.telegram_confirm_ttl_secs),
        });

        let text = format!(
            "{label}を変更しますか？\n現在: {current_value}\n変更後: {new_value}\n\
             ボタンを押すと反映します。\n手入力する場合: /confirm_set {nonce}"
        );
        let confirm_data = format!("confirm:config:{nonce}");
        let cancel_data = format!("cancel:config:{nonce}");
        // ボタンのラベルは動作にする。電源操作は「再起動」「シャットダウン」が
        // そのまま動作として読めるが、設定変更で項目名(「PC IPアドレス」)を
        // 出すと「キャンセル」と並んだときに何が起きるか読めない。
        self.api
            .send_message_with_confirm_buttons(chat_id, &text, "登録", &confirm_data, &cancel_data);
    }

    /// 確認nonceを検証し、結果に関わらず消費する(再利用させないため)。
    /// 有効ならどの操作に対する確認だったかを返す。
    fn consume_pending(&mut self, supplied: &str) -> Option<PendingKind> {
        let pending = self.pending.take()?;
        if !supplied.is_empty() && pending.nonce == supplied && Instant::now() < pending.expires_at {
            Some(pending.kind)
        } else {
            None
        }
    }

    fn run_power_action(&self, action: PowerAction) -> String {
        let _guard = lock_power(&self.power_lock);
        let pc_ip_address = self.settings.pc_ip_address();
        match bridge_client::send_command(action, self.config.as_ref(), &pc_ip_address) {
            Ok(code) if bridge_client::is_accepted(code) => {
                bridge_client::accepted_text(action)
            }
            Ok(code) => bridge_client::rejected_text(action, code),
            Err(e) => {
                println!("bridge command failed: {e}");
                bridge_client::failed_text(action)
            }
        }
    }

    /// 検証・確認済みの設定変更をNVSへ反映する。
    fn apply_config_change(&self, change: &ConfigChange) -> String {
        match change.apply(&self.settings) {
            Ok(()) => format!(
                "{}を変更しました。\n新しい値: {}",
                change.label_ja(),
                change.display_value()
            ),
            Err(e) => {
                println!("settings: failed to persist {}: {e}", change.label_ja());
                format!(
                    "{}の保存に失敗しました。設定は変更されていません。",
                    change.label_ja()
                )
            }
        }
    }

    fn handle_confirmation(&mut self, chat_id: i64, action: PowerAction, supplied: &str) {
        let reply = match self.consume_pending(supplied) {
            Some(PendingKind::Power(pending_action)) if pending_action == action => {
                self.run_power_action(action)
            }
            _ => format!(
                "有効なPCの{}確認がありません。期限切れ、使用済み、またはnonce不一致です。\nもう一度 /{} から実行してください。",
                action.label_ja(),
                action.slug()
            ),
        };
        self.api.send_message(chat_id, &reply);
    }

    /// `/confirm_set <nonce>` の手入力フォールバック。ボタンが押せない場合に使う。
    fn handle_config_confirmation(&mut self, chat_id: i64, supplied: &str) {
        let reply = match self.consume_pending(supplied) {
            Some(PendingKind::Config(change)) => self.apply_config_change(&change),
            _ => "有効な設定変更確認がありません。期限切れ、使用済み、または\
                  nonce不一致です。\nもう一度 /set_ip 等から実行してください。"
                .to_string(),
        };
        self.api.send_message(chat_id, &reply);
    }

    /// `/update`: manifestを取得・検証し、現在の版と比較して確認を求める。
    /// 同じ版なら確認ボタンもnonceも出さない。**無確認で更新はしない。**
    fn handle_update_command(&mut self, chat_id: i64) {
        let pc_ip_address = self.settings.pc_ip_address();
        let manifest =
            match crate::ota::fetch_verified_manifest(self.config.as_ref(), &pc_ip_address) {
                Ok(manifest) => manifest,
                Err(e) => {
                    println!("ota: manifest fetch failed: {e}");
                    self.api
                        .send_message(chat_id, "更新情報の取得に失敗しました。");
                    return;
                }
            };
        let order = pc_remote_signing::compare_versions(FIRMWARE_VERSION, &manifest.version);
        let body =
            pc_remote_signing::ota_confirm_text(FIRMWARE_VERSION, &manifest.version, manifest.size);
        if !order.shows_confirm_button() {
            self.api.send_message(chat_id, &body);
            return;
        }
        let nonce = Self::generate_nonce();
        self.pending = Some(Pending {
            kind: PendingKind::FirmwareUpdate {
                version: manifest.version,
            },
            nonce: nonce.clone(),
            expires_at: Instant::now() + Duration::from_secs(self.config.telegram_confirm_ttl_secs),
        });

        let text = format!("{body}\n手入力する場合: /confirm_update {nonce}");
        let confirm_data = format!("confirm:update:{nonce}");
        let cancel_data = format!("cancel:update:{nonce}");
        self.api.send_message_with_confirm_buttons(
            chat_id,
            &text,
            "更新",
            &confirm_data,
            &cancel_data,
        );
    }

    /// `/confirm_update <nonce>` の手入力フォールバック。ボタンが押せない場合に使う。
    fn handle_update_confirmation(&mut self, chat_id: i64, supplied: &str) {
        let confirmed = self.consume_pending(supplied).and_then(|kind| match kind {
            PendingKind::FirmwareUpdate { version } => Some(version),
            _ => None,
        });
        let Some(confirmed_version) = confirmed else {
            self.api.send_message(
                chat_id,
                "有効な更新確認がありません。期限切れ、使用済み、またはnonce不一致です。\
                 \nもう一度 /update から実行してください。",
            );
            return;
        };
        self.execute_confirmed_ota_update(chat_id, &confirmed_version);
    }

    /// 確認時に合意した版と実行時のmanifestが食い違っていないか確認してから
    /// OTAを実行する。ボタン経路と `/confirm_update` 手入力経路の共通 choke point。
    ///
    /// 確認から確定までの間にbridgeの配信物が差し替わると、正規に署名された
    /// 別版でも署名検証は通る。合意した版と違う版を黙って適用しないため、
    /// 実行直前に取得し直したmanifestの版の一致を要求する。不一致・取得失敗の
    /// ときは更新せず、`/update` のやり直しを求める(確認は消費済みのため)。
    fn execute_confirmed_ota_update(&self, chat_id: i64, confirmed_version: &str) {
        let pc_ip_address = self.settings.pc_ip_address();
        match crate::ota::fetch_verified_manifest(self.config.as_ref(), &pc_ip_address) {
            Ok(manifest) if manifest.version == confirmed_version => {
                self.execute_ota_update(chat_id);
            }
            Ok(manifest) => {
                println!(
                    "ota: manifest version changed since confirm (confirmed={confirmed_version} now={})",
                    manifest.version
                );
                self.api.send_message(
                    chat_id,
                    "更新情報が確認時から変わりました。更新は行いません。\
                     \nもう一度 /update から実行してください。",
                );
            }
            Err(e) => {
                println!("ota: manifest re-fetch failed: {e}");
                self.api.send_message(
                    chat_id,
                    "更新情報の取得に失敗しました。更新は行いません。\
                     \nもう一度 /update から実行してください。",
                );
            }
        }
    }

    /// 確認済みのfirmware更新を開始する。成功時はrebootして戻らない。
    ///
    /// 呼び出し時点でlong pollingのHTTPS接続が閉じていること(ヒープ制約、`ota.rs` 冒頭)。
    fn execute_ota_update(&self, chat_id: i64) {
        // 進捗表示用のメッセージを1つ立て、以降は editMessageText で書き換える。
        // 新しいメッセージを毎回送るとチャットが進捗で埋まるため。
        //
        // message_id が取れなくても更新は続ける(進捗が出ないだけ)。
        let progress_message_id = if chat_id != 0 {
            self.api.send_message_returning_id(
                chat_id,
                "firmware更新を開始します。完了すると自動でM5Stackを再起動します。",
            )
        } else {
            None
        };

        let result = self.run_ota_update(chat_id, progress_message_id);
        if chat_id != 0 {
            self.api.send_message(chat_id, &result);
        }
    }

    /// 進捗メッセージを `editMessageText` で書き換える。進捗バーと適用通知の
    /// 両方から使う共通 choke point (新しいメッセージは増やさない)。
    fn edit_progress_message(
        &self,
        chat_id: i64,
        message_id: i64,
        last_text: &mut String,
        text: String,
    ) {
        // Telegramは同一内容への editMessageText を400にするため、
        // 変化したときだけ送る。
        if text == *last_text {
            return;
        }
        // 失敗しても握り潰す。進捗表示のためにOTAを止めない。
        // `edit_message_text` の中でログは出る。
        //
        // 1回の通知にかかった時間を出す。通知の間はTLSハンドシェイクで
        // ダウンロードが止まるため、刻みを細かくしてよいかの判断材料になる
        // (`OTA_PROGRESS_STEP_PERCENT` を変えるときはこの値を見ること)。
        let started = std::time::Instant::now();
        self.api.edit_message_text(chat_id, message_id, &text);
        println!(
            "ota: progress notify took {}ms",
            started.elapsed().as_millis()
        );
        *last_text = text;
    }

    /// 電源操作ロックを取り、OTAを実行する。成功時は `restart()` で戻らない。
    fn run_ota_update(&self, chat_id: i64, progress_message_id: Option<i64>) -> String {
        let _guard = lock_power(&self.power_lock);
        let pc_ip_address = self.settings.pc_ip_address();

        // 直前に送った本文。進捗バーと適用通知で別々に持つが、両者の文言自体を
        // 変えてある (`pc_remote_signing::ota_applying_text` 参照) ため、
        // 段階が進んでも同一内容の400にならない。
        let mut last_progress_text = String::new();
        let mut last_applying_text = String::new();
        let mut on_progress = |manifest: &pc_remote_signing::OtaManifest, received: u64| {
            let Some(message_id) = progress_message_id else {
                return;
            };
            let text = pc_remote_signing::ota_progress_text(
                &manifest.version,
                received,
                manifest.size,
            );
            self.edit_progress_message(chat_id, message_id, &mut last_progress_text, text);
        };
        // 検証・書き込み完了の通知。同じメッセージを書き換える。
        // `edit_message_text` は応答を待つ同期POSTのため、戻った時点で送信済みで
        // あり、この後の `restart()` で欠けない。失敗してもM5Stackの再起動は止めない
        // (コールバックは戻り値を持たない契約)。
        let mut on_applying = |manifest: &pc_remote_signing::OtaManifest| {
            let Some(message_id) = progress_message_id else {
                return;
            };
            let text = pc_remote_signing::ota_applying_text(&manifest.version);
            self.edit_progress_message(chat_id, message_id, &mut last_applying_text, text);
        };

        match crate::ota::run_ota_update(
            self.config.as_ref(),
            &pc_ip_address,
            &mut on_progress,
            &mut on_applying,
        ) {
            // 成功時は `restart()` で戻らないため、ここへは来ない。防御的に残す。
            Ok(()) => "更新が完了しました。M5Stackを再起動します。".to_string(),
            Err(e) => {
                // 詳細(espのエラー文言を含む)はシリアルログにだけ残し、Telegramへは
                // 接続先が混ざらない固定ラベルを返す。`user_message` のコメント参照。
                println!("ota: update failed: {e}");
                format!("firmware更新に失敗しました。\n{}", e.user_message())
            }
        }
    }

    fn dispatch_command(&mut self, chat_id: i64, command: &str, args: &str) {
        // default-deny: 通すものだけを列挙する。禁止側を列挙すると、コマンド追加時に
        // 書き忘れたものがロック中に実行できてしまう(失敗の向きが危険側になる)。
        if self.operation_lock.is_locked() && !ALLOWED_WHILE_LOCKED.contains(&command) {
            self.api.send_message(
                chat_id,
                "操作はロック中です。/unlock で解除してから実行してください。",
            );
            return;
        }

        match command {
            "/status" => {
                self.api.send_message(chat_id, &self.status_text());
            }
            "/settings" => self.send_settings_menu(chat_id),
            "/lock" => {
                self.set_operation_lock(true);
                self.api
                    .send_message(chat_id, "操作をロックしました。/unlock で解除できます。");
            }
            "/unlock" => {
                self.set_operation_lock(false);
                self.api
                    .send_message(chat_id, "操作のロックを解除しました。");
            }
            "/wake" => {
                let _guard = lock_power(&self.power_lock);
                let wol_port = self.settings.wol_port();
                let wol_ok = match net::send_wake_on_lan(&self.config.pc_mac_address, wol_port) {
                    Ok(()) => {
                        println!("WOL sent");
                        true
                    }
                    Err(e) => {
                        println!("WOL failed: {e}");
                        false
                    }
                };
                // 文言の正本は `wake_check::wake_request_text` /
                // `wake_check::wake_request_failed_text`(用語集に従いWOLを使わない)。
                let reply = if wol_ok {
                    wake_check::wake_request_text()
                } else {
                    wake_check::wake_request_failed_text()
                };
                drop(_guard);
                if wol_ok {
                    // すでにオンなら待機しない。pollingスレッドはUI側の最新状態を
                    // 持たないため、その場でSTATUS相当の疎通確認をする
                    // (最大 `STATUS_PROBE_TIMEOUT`。`/status` と同じ)。
                    // ロックの順序: powerは既に離し、wakeは `begin_wake_watch` 内で
                    // 短時間だけ握り、`send_message`(`HttpsLock`取得)の前には離す。
                    let online_now = net::check_pc_online(
                        &self.settings.pc_status_addr(),
                        net::STATUS_PROBE_TIMEOUT,
                    );
                    begin_wake_watch(&self.wake_watch, online_now);
                }
                self.api.send_message(chat_id, reply);
            }
            "/reboot" => self.request_confirmation(chat_id, PowerAction::Reboot),
            "/shutdown" => self.request_confirmation(chat_id, PowerAction::Shutdown),
            "/confirm_reboot" => self.handle_confirmation(chat_id, PowerAction::Reboot, args),
            "/confirm_shutdown" => self.handle_confirmation(chat_id, PowerAction::Shutdown, args),
            "/update" => self.handle_update_command(chat_id),
            "/confirm_update" => self.handle_update_confirmation(chat_id, args),
            "/set_ip" => self.handle_set_command(chat_id, SettingKind::PcIp, args),
            "/set_wol_port" => self.handle_set_command(chat_id, SettingKind::WolPort, args),
            "/set_brightness" => self.handle_set_command(chat_id, SettingKind::Brightness, args),
            "/confirm_set" => self.handle_config_confirmation(chat_id, args),
            // 未知のコマンドは静かに無視する。
            _ => {}
        }
    }

    /// callback_dataを解析する。形式外、古いボタン、別bot由来の値は拒否する。
    fn parse_callback_data(data: &str) -> Option<Callback> {
        fn decision(confirm: bool, target: &str, nonce: &str) -> Option<Callback> {
            let target = if target == "config" {
                CallbackTarget::Config
            } else if target == "update" {
                CallbackTarget::FirmwareUpdate
            } else {
                CallbackTarget::Power(PowerAction::from_slug(target)?)
            };
            Some(Callback::Decision {
                confirm,
                target,
                nonce: nonce.to_string(),
            })
        }

        let parts: Vec<&str> = data.split(':').collect();
        match parts.as_slice() {
            ["confirm", target, nonce] if !nonce.is_empty() => decision(true, target, nonce),
            ["cancel", target, nonce] if !nonce.is_empty() => decision(false, target, nonce),
            ["setedit", slug] => Some(Callback::EditSetting(SettingKind::from_slug(slug)?)),
            ["lock", "on"] => Some(Callback::SetLock(true)),
            ["lock", "off"] => Some(Callback::SetLock(false)),
            _ => None,
        }
    }

    fn handle_callback_query(&mut self, callback: &Value) {
        let id = callback["id"].as_str().unwrap_or_default().to_string();
        let from_id = callback["from"]["id"].as_i64().unwrap_or_default();
        // Issue #130-1: 設定値の前後空白を除いてから照合する。正規化は
        // `config_validation` に一本化し、`is_configured`・chat_id化と同じ値を使う。
        if !config_validation::telegram_user_id_matches(
            &self.config.telegram_allowed_user_id,
            from_id,
        ) {
            // 権限がない場合はpending確認を触らず、Telegram側の読み込み状態だけ終わらせる。
            self.api.answer_callback_query(&id, "権限がありません");
            self.record_unauthorized_access();
            return;
        }

        let data = callback["data"].as_str().unwrap_or_default();
        let Some(parsed) = Self::parse_callback_data(data) else {
            // nonceを含み得るためdata本体は出さない。種類だけ分かれば切り分けできる。
            println!("telegram: callback rejected (unparsable)");
            self.api.answer_callback_query(&id, "無効なボタンです");
            return;
        };
        println!("telegram: callback {}", parsed.log_label());

        // ロック中はボタンからの実行も拒否する。ただし解除ボタンだけは通さないと
        // ロックから戻れなくなる(`/unlock` がロック中でも通るのと同じ扱い)。
        if self.operation_lock.is_locked() && !matches!(parsed, Callback::SetLock(false)) {
            self.api.answer_callback_query(&id, "操作はロック中です");
            return;
        }

        let chat_id = callback["message"]["chat"]["id"]
            .as_i64()
            .unwrap_or_default();

        let (confirm, target, nonce) = match parsed {
            Callback::Decision {
                confirm,
                target,
                nonce,
            } => (confirm, target, nonce),
            Callback::EditSetting(kind) => {
                self.api.answer_callback_query(&id, kind.label_ja());
                if chat_id != 0 {
                    self.start_setting_input(chat_id, kind);
                }
                return;
            }
            Callback::SetLock(locked) => {
                self.set_operation_lock(locked);
                let reply = if locked {
                    "操作をロックしました"
                } else {
                    "操作のロックを解除しました"
                };
                self.api.answer_callback_query(&id, reply);
                if chat_id != 0 {
                    // 切り替え後の状態でメニューを出し直す。
                    self.send_settings_menu(chat_id);
                }
                return;
            }
        };

        // nonceが一致しても、ボタンが指す対象(target)とpendingの中身が一致しない
        // 限り有効扱いにしない(古いボタンや別種類の保留との取り違えを防ぐ)。
        let valid = self.consume_pending(&nonce).filter(|kind| target.matches(kind));
        let is_confirm = confirm;

        if !is_confirm {
            let answer_text = if valid.is_some() {
                "キャンセルしました"
            } else {
                "処理済みです"
            };
            let reply = match &valid {
                Some(kind) => format!("{}をキャンセルしました。", kind.label_ja()),
                None => "有効な確認がありません。期限切れ、使用済み、またはnonce不一致です。"
                    .to_string(),
            };
            // Issue #163 a0: answerとsendを1ハンドルで連続送信する。
            if chat_id != 0 {
                self.api
                    .answer_callback_query_and_send(&id, answer_text, chat_id, &reply);
            } else {
                self.api.answer_callback_query(&id, answer_text);
            }
            return;
        }

        let Some(kind) = valid else {
            // Issue #163 a0: answerとsendを1ハンドルで連続送信する。
            if chat_id != 0 {
                self.api.answer_callback_query_and_send(
                    &id,
                    "期限切れまたは処理済みです",
                    chat_id,
                    "有効な確認がありません。期限切れ、使用済み、またはnonce不一致です。\
                      \nもう一度実行してください。",
                );
            } else {
                self.api
                    .answer_callback_query(&id, "期限切れまたは処理済みです");
            }
            return;
        };

        let result = match kind {
            PendingKind::Power(action) => self.run_power_action(action),
            PendingKind::Config(change) => self.apply_config_change(&change),
            PendingKind::FirmwareUpdate { version } => {
                // OTAはrebootを伴うため、通常の「結果をanswer+送信」とは別経路にする。
                // 先に開始をanswerして接続を閉じてから `execute_ota_update` へ渡す。
                // ヒープ制約(long polling接続を開いたままOTAを呼ばない)の詳細は
                // `ota.rs` の冒頭コメントと `poll_once` 内のコメントを参照。
                // 実行直前にmanifestを取り直し、合意した版との一致を要求する
                // (`execute_confirmed_ota_update` のコメント参照)。
                self.api.answer_callback_query(&id, "更新を開始します");
                self.execute_confirmed_ota_update(chat_id, &version);
                return;
            }
        };
        // Issue #163 a0: answerとsendを1ハンドルで連続送信する。
        // OTAはrebootを伴うため対象外で、上でanswer後に別経路へ渡している。
        if chat_id != 0 {
            self.api
                .answer_callback_query_and_send(&id, &result, chat_id, &result);
        } else {
            self.api.answer_callback_query(&id, &result);
        }
    }

    fn handle_message(&mut self, message: &Value) {
        let from_id = message["from"]["id"].as_i64().unwrap_or_default();
        // Issue #130-1: callback側と同じ正規化で照合する。
        if !config_validation::telegram_user_id_matches(
            &self.config.telegram_allowed_user_id,
            from_id,
        ) {
            // 権限がないユーザーには返信しない(相手にbotの存在を確かめさせない)。
            // 自分宛のアラートだけ、閾値を超えたときに送る。
            self.record_unauthorized_access();
            return;
        }
        let chat_id = message["chat"]["id"].as_i64().unwrap_or_default();
        let text = message["text"].as_str().unwrap_or_default().trim();
        if text.is_empty() {
            return;
        }

        // 設定変更ボタンの直後に届いた非コマンドのテキストは、値の入力として扱う。
        if !text.starts_with('/') {
            if let Some(kind) = self.take_pending_input() {
                if self.operation_lock.is_locked() {
                    self.api.send_message(
                        chat_id,
                        "操作はロック中です。/unlock で解除してから実行してください。",
                    );
                    return;
                }
                self.handle_setting_input(chat_id, kind, text);
            }
            // 入力待ちが無いテキストは静かに無視する(コマンドではないため)。
            return;
        }

        let (command, args) = match text.split_once(' ') {
            Some((c, a)) => (c, a.trim()),
            None => (text, ""),
        };
        // グループチャットでTelegramが付ける `@botname` suffixを外す。
        let command = command.split('@').next().unwrap_or(command);
        self.dispatch_command(chat_id, command, args);
    }

    fn process_updates(&mut self, results: &[Value], dispatch: bool) {
        for item in results {
            let update_id = item["update_id"].as_i64().unwrap_or_default();
            if update_id >= self.last_update_id {
                self.last_update_id = update_id + 1;
            }
            if !dispatch {
                // 起動直後の最初のバッチはoffset更新だけ行う。
                continue;
            }

            // 受信した種類だけ残す。本文やnonceは出さない。
            // 「ボタンを押しても無反応」のとき、updateが届いていないのか
            // 処理側で落ちているのかを切り分けるために必要。
            let is_callback = item.get("callback_query").is_some_and(|v| !v.is_null());
            println!(
                "telegram: update {}",
                if is_callback {
                    "callback_query"
                } else if item.get("message").is_some_and(|v| !v.is_null()) {
                    "message"
                } else {
                    "other"
                }
            );

            if let Some(callback) = item.get("callback_query") {
                if !callback.is_null() {
                    self.handle_callback_query(callback);
                    continue;
                }
            }
            if let Some(message) = item.get("message") {
                if !message.is_null() {
                    self.handle_message(message);
                }
            }
        }
    }

    fn poll_once(&mut self) -> Result<(), Box<dyn Error>> {
        // allowed_updatesはBot API側に前回値が残り続ける。省略すると、過去に
        // 誰かが別の値で呼んだ設定を引きずってcallback_queryが届かなくなり得るため、
        // 必要な種類を毎回明示する。値はJSON配列をpercent-encodeした固定文字列。
        const ALLOWED_UPDATES: &str = "%5B%22message%22%2C%22callback_query%22%5D";
        let url = format!(
            "{}?timeout={}&offset={}&allowed_updates={ALLOWED_UPDATES}",
            self.api.api_url("getUpdates"),
            self.config.telegram_long_poll_timeout_seconds,
            self.last_update_id
        );
        // long pollingの接続はここで閉じ切ってから、updateの処理へ移る。
        //
        // 処理側(answerCallbackQuery / sendMessage)は新しいHTTPS接続を張る。
        // long pollingの接続を開いたまま2本目を張ると、ESP32ではmbedTLSの
        // ヒープが足りず `ESP_ERR_HTTP_CONNECT` で失敗する。実機では
        // 「ボタンを押してもトーストが出ない(answerCallbackQueryだけ落ちる)」
        // という形で表面化した。clientとresponseをこのブロックへ閉じ込め、
        // dropさせてから `process_updates` を呼ぶ。
        //
        // Issue #127: このブロックは通知スレッドとも共有のHTTPSロックで守る。
        // 以前の構造は単一スレッド内の順序付けで、通知スレッドの `sendMessage`
        // とは同期していなかった。`process_updates` の送信はこのブロックの
        // 外=ロックを離してから行い、再取得にする(std Mutexは再入不可のため、
        // 握ったまま送ると自己デッドロックする)。
        let body = {
            // long pollは最大 `long_poll_timeout+10` 秒この接続を保持するため、
            // その間通知スレッドはロック待ちになる(通知が遅れる上限)。
            // 握らないと2本同時TLSで通知側が落ち、黙って消える。
            let _https_guard = lock_https(&self.api.https);
            // Issue #163 c0: 接続確立から応答完了までの所要時間を出す。
            // URL(tokenを含む)は出さず、method名とミリ秒だけ。
            let started = Instant::now();
            let mut client = self.api.http_client()?;
            let request = client.request(Method::Get, &url, &[])?;
            let mut response = request.submit()?;
            let status = response.status();
            if status != 200 {
                return Err(format!("getUpdates failed: {status}").into());
            }

            let mut body = Vec::new();
            let mut chunk = [0u8; 512];
            loop {
                let read = response.read(&mut chunk)?;
                if read == 0 {
                    break;
                }
                if body.len() + read > RESPONSE_MAX_BYTES {
                    return Err(
                        format!("getUpdates response exceeds {RESPONSE_MAX_BYTES} bytes").into(),
                    );
                }
                body.extend_from_slice(&chunk[..read]);
            }
            println!(
                "telegram: getUpdates took {}ms",
                started.elapsed().as_millis()
            );
            body
        };

        let parsed: Value = serde_json::from_slice(&body)?;
        // `result` が配列でない応答(APIエラー本文など)を空扱いで握り潰すと、
        // 異常に気づけないまま無言で回り続ける。エラーにしてbackoffへ回す。
        let Some(results) = parsed["result"].as_array().cloned() else {
            return Err("getUpdates response has no result array".into());
        };
        let dispatch = self.initial_sync_done;
        self.process_updates(&results, dispatch);
        self.initial_sync_done = true;
        Ok(())
    }

    /// 専用スレッドでlong pollingを継続する。
    pub fn run(mut self, state: Arc<Mutex<State>>) {
        if let Err(e) = ensure_root_ca() {
            println!("telegram: root CA install failed: {e}");
            *lock_state(&state) = State::Error;
            return;
        }

        // NTP同期を短時間だけ待つ。未同期なら電源操作の送信側で拒否する。
        net::wait_for_time_sync(Duration::from_secs(10));

        let mut backoff = BACKOFF_MIN;
        *lock_state(&state) = State::Polling;

        loop {
            match self.poll_once() {
                Ok(()) => {
                    backoff = BACKOFF_MIN;
                    *lock_state(&state) = State::Polling;
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    println!("telegram poll error: {e}");
                    *lock_state(&state) = State::Error;
                    std::thread::sleep(backoff);
                    backoff = (backoff * 2).min(BACKOFF_MAX);
                }
            }
        }
    }
}
