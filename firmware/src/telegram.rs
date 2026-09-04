// Telegram Bot APIクライアント。外向きHTTPS long pollingで、受信portを開けずに
// スマホ操作を受け取る。
//
// 守るべき挙動:
//   - `from.id` が許可ユーザーIDと一致するupdateだけ処理する
//   - /reboot と /shutdown は即実行せず、単回使用の確認nonceを発行する
//   - 確認は成功・失敗・期限切れのいずれでも消費し、再利用させない
//   - 起動直後の最初のgetUpdates結果はoffset更新だけにし、オフライン中の古い命令を実行しない
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
use crate::bridge_client::{self, PowerAction, PowerActionLabel};
use crate::net;
use crate::settings::RuntimeSettings;
use crate::telegram_root_ca::TELEGRAM_ROOT_CA_PEM;

const PLACEHOLDER_TOKEN: &str = "replace-with-your-telegram-bot-token";
const PLACEHOLDER_USER_ID: &str = "replace-with-your-telegram-user-id";

const BACKOFF_MIN: Duration = Duration::from_secs(5);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
const RESPONSE_BUFFER: usize = 4096;
/// getUpdates応答の受け入れ上限。ESP32のヒープは小さく、応答をVecへ無制限に
/// ためると枯渇し得る。超過分は読まずにエラーとし、backoffへ回す。
/// 通常のupdateは数KB程度で、上限に当たるのは異常時だけ。
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

/// 電源操作の排他を取る。poisonしていても排他は維持する。
///
/// `lock()` の `Result` をそのまま束縛すると、poison時(保持中に他スレッドが
/// panic)にguardを得られないまま処理が進み、排他が外れる。逆に `unwrap()` は
/// UIループごとpanicさせてしまう。このMutexが守っているのは `()` で、
/// 壊れた状態を引き継ぐ心配がないため `into_inner()` で回復してよい。
pub fn lock_power(power_lock: &PowerLock) -> std::sync::MutexGuard<'_, ()> {
    power_lock.lock().unwrap_or_else(|e| e.into_inner())
}

/// UIスレッドへ共有するTelegram状態の排他を取る。poisonしていても排他は維持する。
///
/// このMutexが守るのは `State` 一つだけで、書きかけの壊れた状態を引き継ぐ心配が
/// ない。ここで `unwrap()` すると、無関係なスレッドのpanicに巻き込まれてUIループや
/// pollingスレッドごと落ち、端末が止まる。`lock_power` と同じ扱いにそろえる。
pub fn lock_state(state: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    state.lock().unwrap_or_else(|e| e.into_inner())
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

pub fn is_configured(config: &AppConfig) -> bool {
    !config.telegram_bot_token.is_empty()
        && config.telegram_bot_token != PLACEHOLDER_TOKEN
        && !config.telegram_allowed_user_id.is_empty()
        && config.telegram_allowed_user_id != PLACEHOLDER_USER_ID
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

/// global CA storeはプロセス全体で1つなので、pollingスレッドと通知スレッドの
/// どちらが先に走っても登録が1回になるようにする。
///
/// `Once`は使わない。`Once`は失敗しても「完了」扱いになるため、最初の呼び出しが
/// 一時的なエラー(heap不足等)で失敗すると、以降どのスレッドも再試行できず、
/// CA store未設定のままHTTPSを使い続けてしまう。成功フラグ + Mutexにして、
/// 失敗した場合は次の呼び出しで再試行できるようにする。
static ROOT_CA_INSTALLED: AtomicBool = AtomicBool::new(false);
static ROOT_CA_LOCK: Mutex<()> = Mutex::new(());

fn ensure_root_ca() -> Result<(), Box<dyn Error>> {
    // 毒されたMutexでも初期化は続行してよい(共有している状態はフラグだけ)。
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
    config.telegram_allowed_user_id.parse::<i64>().ok()
}

/// Bot APIへのHTTP呼び出し。pollingスレッドと通知スレッドの両方から使う。
struct Api {
    config: Arc<AppConfig>,
}

/// 確認待ちの操作。電源操作(REBOOT/SHUTDOWN)と設定変更(/set_*)の両方が、
/// 同じ「単回使用nonce付き確認」フローを共有する。同時に保留できるのは1件のみ。
enum PendingKind {
    Power(PowerAction),
    Config(ConfigChange),
}

impl PendingKind {
    fn label_ja(&self) -> String {
        match self {
            PendingKind::Power(action) => action.label_ja().to_string(),
            PendingKind::Config(change) => change.label_ja().to_string(),
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
}

impl CallbackTarget {
    /// ボタンが指す対象と、実際に保留されている確認が一致するか。
    /// 一致しないボタン(古いメッセージ、別種類の保留との衝突)は無効として扱う。
    fn matches(&self, kind: &PendingKind) -> bool {
        match (self, kind) {
            (CallbackTarget::Power(a), PendingKind::Power(b)) => a == b,
            (CallbackTarget::Config, PendingKind::Config(_)) => true,
            _ => false,
        }
    }
}

/// 変更できる設定項目の識別子。値を持たないので、ボタンの`callback_data`と
/// 「いまどの項目の入力を待っているか」の記録に使える。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingKind {
    PcIp,
    StatusAddr,
    WolPort,
}

impl SettingKind {
    const ALL: [SettingKind; 3] = [
        SettingKind::PcIp,
        SettingKind::StatusAddr,
        SettingKind::WolPort,
    ];

    fn slug(self) -> &'static str {
        match self {
            SettingKind::PcIp => "pc_ip",
            SettingKind::StatusAddr => "status_addr",
            SettingKind::WolPort => "wol_port",
        }
    }

    fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.slug() == slug)
    }

    fn label_ja(self) -> &'static str {
        match self {
            SettingKind::PcIp => "PC IPアドレス",
            SettingKind::StatusAddr => "STATUS確認先",
            SettingKind::WolPort => "WOLポート",
        }
    }

    /// 入力例。入力欄のplaceholderと案内文へ出す。
    fn example(self) -> &'static str {
        match self {
            SettingKind::PcIp => "192.168.1.50",
            SettingKind::StatusAddr => "192.168.1.50:80",
            SettingKind::WolPort => "9",
        }
    }

    fn current(self, settings: &RuntimeSettings) -> String {
        match self {
            SettingKind::PcIp => settings.pc_ip_address(),
            SettingKind::StatusAddr => settings.pc_status_addr(),
            SettingKind::WolPort => settings.wol_port().to_string(),
        }
    }

    /// 入力値を検証し、確認待ちの変更内容へ変換する。
    fn parse(self, raw: &str) -> Result<ConfigChange, String> {
        match self {
            SettingKind::PcIp => {
                config_validation::validate_ipv4(raw).map(ConfigChange::PcIpAddress)
            }
            SettingKind::StatusAddr => {
                config_validation::validate_status_addr(raw).map(ConfigChange::PcStatusAddr)
            }
            SettingKind::WolPort => {
                config_validation::validate_wol_port(raw).map(ConfigChange::WolPort)
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
    PcStatusAddr(String),
    WolPort(u16),
}

impl ConfigChange {
    fn label_ja(&self) -> &'static str {
        match self {
            ConfigChange::PcIpAddress(_) => "PC IPアドレス",
            ConfigChange::PcStatusAddr(_) => "STATUS確認先",
            ConfigChange::WolPort(_) => "WOLポート",
        }
    }

    fn display_value(&self) -> String {
        match self {
            ConfigChange::PcIpAddress(value) | ConfigChange::PcStatusAddr(value) => value.clone(),
            ConfigChange::WolPort(value) => value.to_string(),
        }
    }

    /// NVSへ永続化し、成功したときだけ`settings`上の値も更新する
    /// (`RuntimeSettings`側の書き込み成功後だけメモリを更新する方針をそのまま踏襲)。
    fn apply(&self, settings: &RuntimeSettings) -> Result<(), esp_idf_sys::EspError> {
        match self {
            ConfigChange::PcIpAddress(value) => settings.set_pc_ip_address(value.clone()),
            ConfigChange::PcStatusAddr(value) => settings.set_pc_status_addr(value.clone()),
            ConfigChange::WolPort(value) => settings.set_wol_port(*value),
        }
    }
}

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
        use esp_idf_svc::io::Write;

        let payload = serde_json::to_string(body)?;
        let url = self.api_url(method);
        let mut client = self.http_client()?;
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
        if !(200..300).contains(&status) {
            // 呼び出し側が再送要否を判断できるよう、失敗はエラーとして返す。
            return Err(format!("telegram {method} failed: {status}").into());
        }
        Ok(())
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
        // 再起動のたびに同じ日のレポートが届くのを防ぐため。
        //
        // まだ送信時刻より前なら未送信のままにする。ここで無条件に送信済みへ
        // すると、送信時刻の前に再起動しただけでその日の分が飛んでしまう
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
        if unix < net::MIN_VALID_UNIX_TIME as i64 {
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
    tx: mpsc::Sender<String>,
}

impl Notifier {
    pub fn notify(&self, text: String) {
        let _ = self.tx.send(text);
    }
}

/// 通知送信スレッドを起動し、UIスレッド用のハンドルを返す。
/// Telegram未設定、または許可ユーザーIDがchat_idとして使えない場合はNone。
pub fn start_notifier(config: Arc<AppConfig>, settings: Arc<RuntimeSettings>) -> Option<Notifier> {
    if !is_configured(config.as_ref()) {
        return None;
    }
    let chat_id = allowed_chat_id(config.as_ref())?;

    let (tx, rx) = mpsc::channel::<String>();
    let api = Api { config };
    let spawned = std::thread::Builder::new()
        .stack_size(12 * 1024)
        .spawn(move || {
            let mut schedule = DailyReport::new(&api.config);
            loop {
                // 定期レポートの時刻判定のため、通知が無くても定期的に起きる。
                let queued = match rx.recv_timeout(DAILY_REPORT_CHECK_INTERVAL) {
                    Ok(text) => Some(text),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    // 送信側(UIスレッド)が全て落ちた場合は終了する。
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                };

                // CA storeの確認は`due_report()`より前に行う。`due_report()`は
                // 返した時点で「その日は送信済み」と記録するため、後段で失敗すると
                // その日のレポートが黙って落ちる。
                if let Err(e) = ensure_root_ca() {
                    println!("telegram: root CA install failed: {e}");
                    continue;
                }

                if let Some(text) = queued {
                    api.send_message(chat_id, &text);
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
            },
            config,
            settings,
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
        format!(
            "PC: {}\n操作: {}\nM5Stack: Rust firmware",
            net::pc_online_label_ja(online),
            if self.operation_lock.is_locked() {
                "ロック中"
            } else {
                "可能"
            }
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
        let (pc_ip_address, pc_status_addr, wol_port) = self.settings.snapshot();
        let text = format!(
            "現在の設定\n\
             ・PC IPアドレス: {pc_ip_address}\n\
             ・STATUS確認先: {pc_status_addr}\n\
             ・WOLポート: {wol_port}\n\
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
                format!("{}を受け付けました。", action.label_ja())
            }
            Ok(code) => format!("{}に失敗しました。({code})", action.label_ja()),
            Err(e) => {
                println!("bridge command failed: {e}");
                format!("{}に失敗しました。", action.label_ja())
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
                "有効な{}確認がありません。期限切れ、使用済み、またはnonce不一致です。\nもう一度 /{} から実行してください。",
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

    fn dispatch_command(&mut self, chat_id: i64, command: &str, args: &str) {
        // 操作系コマンドはロック中に一切実行しない。ロックの切り替えと状態確認は
        // ロック中でも受け付ける(そうしないと解除できない)。`/settings`は
        // `/status`と同枠の参照系として、ロック中でも読める。
        const LOCKED_COMMANDS: [&str; 9] = [
            "/wake",
            "/reboot",
            "/shutdown",
            "/confirm_reboot",
            "/confirm_shutdown",
            "/set_ip",
            "/set_status_addr",
            "/set_wol_port",
            "/confirm_set",
        ];
        if self.operation_lock.is_locked() && LOCKED_COMMANDS.contains(&command) {
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
                let reply = match net::send_wake_on_lan(&self.config.pc_mac_address, wol_port) {
                    Ok(()) => "WOLを送信しました。",
                    Err(e) => {
                        println!("WOL failed: {e}");
                        "WOL送信に失敗しました。"
                    }
                };
                drop(_guard);
                self.api.send_message(chat_id, reply);
            }
            "/reboot" => self.request_confirmation(chat_id, PowerAction::Reboot),
            "/shutdown" => self.request_confirmation(chat_id, PowerAction::Shutdown),
            "/confirm_reboot" => self.handle_confirmation(chat_id, PowerAction::Reboot, args),
            "/confirm_shutdown" => self.handle_confirmation(chat_id, PowerAction::Shutdown, args),
            "/set_ip" => self.handle_set_command(chat_id, SettingKind::PcIp, args),
            "/set_status_addr" => self.handle_set_command(chat_id, SettingKind::StatusAddr, args),
            "/set_wol_port" => self.handle_set_command(chat_id, SettingKind::WolPort, args),
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
        if from_id.to_string() != self.config.telegram_allowed_user_id {
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
            self.api.answer_callback_query(
                &id,
                if valid.is_some() {
                    "キャンセルしました"
                } else {
                    "処理済みです"
                },
            );
            if chat_id != 0 {
                let reply = match &valid {
                    Some(kind) => format!("{}をキャンセルしました。", kind.label_ja()),
                    None => "有効な確認がありません。期限切れ、使用済み、またはnonce不一致です。"
                        .to_string(),
                };
                self.api.send_message(chat_id, &reply);
            }
            return;
        }

        let Some(kind) = valid else {
            self.api
                .answer_callback_query(&id, "期限切れまたは処理済みです");
            if chat_id != 0 {
                self.api.send_message(
                    chat_id,
                    "有効な確認がありません。期限切れ、使用済み、またはnonce不一致です。\
                     \nもう一度実行してください。",
                );
            }
            return;
        };

        let result = match kind {
            PendingKind::Power(action) => self.run_power_action(action),
            PendingKind::Config(change) => self.apply_config_change(&change),
        };
        self.api.answer_callback_query(&id, &result);
        if chat_id != 0 {
            self.api.send_message(chat_id, &result);
        }
    }

    fn handle_message(&mut self, message: &Value) {
        let from_id = message["from"]["id"].as_i64().unwrap_or_default();
        if from_id.to_string() != self.config.telegram_allowed_user_id {
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
        let body = {
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
            body
        };

        let parsed: Value = serde_json::from_slice(&body)?;
        let results = parsed["result"].as_array().cloned().unwrap_or_default();
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
