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

use crate::app_config::AppConfig;
use crate::bridge_client::{self, PowerAction};
use crate::net;
use crate::telegram_root_ca::TELEGRAM_ROOT_CA_PEM;

const PLACEHOLDER_TOKEN: &str = "replace-with-your-telegram-bot-token";
const PLACEHOLDER_USER_ID: &str = "replace-with-your-telegram-user-id";

/// 未許可ユーザーからのアクセスが何回たまったらアラートを送るか。
/// 1回目から送ると、無関係なbot巡回でも鳴ってしまう。
const UNAUTHORIZED_ALERT_THRESHOLD: u32 = 3;
/// アラートの最短送信間隔。スキャンや連投で通知が埋まらないようにする。
const UNAUTHORIZED_ALERT_INTERVAL: Duration = Duration::from_secs(3600);

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

struct Pending {
    action: PowerAction,
    nonce: String,
    expires_at: Instant,
}

pub struct Client {
    last_update_id: i64,
    initial_sync_done: bool,
    pending: Option<Pending>,
    power_lock: PowerLock,
    operation_lock: OperationLock,
    config: Arc<AppConfig>,
    api: Api,
    /// 未許可アクセスの検知数と、直近でアラートを送った時刻。
    unauthorized_count: u32,
    unauthorized_alert_at: Option<Instant>,
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
    fn send_message_with_confirm_buttons(
        &self,
        chat_id: i64,
        text: &str,
        confirm_label: &str,
        confirm_data: &str,
        cancel_data: &str,
    ) {
        let _ = self.post_json(
            "sendMessage",
            &json!({
                "chat_id": chat_id,
                "text": text,
                "reply_markup": {
                    "inline_keyboard": [[
                        { "text": confirm_label, "callback_data": confirm_data },
                        { "text": "キャンセル", "callback_data": cancel_data },
                    ]]
                }
            }),
        );
    }

    /// callback_queryへ応答し、Telegramクライアント側の読み込み表示を終わらせる。
    fn answer_callback_query(&self, id: &str, text: &str) {
        let mut body = json!({ "callback_query_id": id });
        if !text.is_empty() {
            body["text"] = json!(text);
        }
        let _ = self.post_json("answerCallbackQuery", &body);
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
    fn due_report(&self, config: &AppConfig) -> Option<(i64, String)> {
        if !(0..=23).contains(&config.daily_report_hour) {
            return None;
        }
        let (day, hour) = Self::local_now(config)?;
        if hour != config.daily_report_hour || self.last_sent_day == Some(day) {
            return None;
        }

        let online = net::check_pc_online(&config.pc_status_addr, Duration::from_millis(800));
        Some((
            day,
            format!(
                "定期レポート\nPC: {}",
                if online {
                    "オンライン"
                } else {
                    "オフライン"
                }
            ),
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
pub fn start_notifier(config: Arc<AppConfig>) -> Option<Notifier> {
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
                if let Some((day, text)) = schedule.due_report(&api.config) {
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
    ) -> Self {
        Self {
            last_update_id: 0,
            initial_sync_done: false,
            pending: None,
            power_lock,
            operation_lock,
            api: Api {
                config: Arc::clone(&config),
            },
            config,
            unauthorized_count: 0,
            unauthorized_alert_at: None,
        }
    }

    /// 未許可ユーザーからのアクセスを記録し、閾値を超えたらアラートを送る。
    ///
    /// 通知には送信者のIDやメッセージ本文を一切含めない。相手が自由に決められる
    /// 文字列をそのまま自分のチャットへ流すと、なりすましや誘導の材料になるため。
    fn record_unauthorized_access(&mut self) {
        self.unauthorized_count += 1;
        if self.unauthorized_count < UNAUTHORIZED_ALERT_THRESHOLD {
            return;
        }

        let now = Instant::now();
        if let Some(sent_at) = self.unauthorized_alert_at {
            if now.duration_since(sent_at) < UNAUTHORIZED_ALERT_INTERVAL {
                return;
            }
        }

        let count = self.unauthorized_count;
        self.unauthorized_count = 0;
        self.unauthorized_alert_at = Some(now);
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
        let online = net::check_pc_online(&self.config.pc_status_addr, Duration::from_millis(800));
        format!(
            "PC: {}\n操作: {}\nM5Stack: Rust firmware",
            if online {
                "オンライン"
            } else {
                "オフライン"
            },
            if self.operation_lock.is_locked() {
                "ロック中"
            } else {
                "可能"
            }
        )
    }

    fn request_confirmation(&mut self, chat_id: i64, action: PowerAction) {
        let nonce = Self::generate_nonce();
        self.pending = Some(Pending {
            action,
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

    /// 確認nonceを検証し、結果に関わらず消費する。
    fn consume_pending(&mut self, action: PowerAction, supplied: &str) -> bool {
        let pending = self.pending.take();
        match pending {
            Some(p) => {
                p.action == action
                    && !supplied.is_empty()
                    && p.nonce == supplied
                    && Instant::now() < p.expires_at
            }
            None => false,
        }
    }

    fn run_power_action(&self, action: PowerAction) -> String {
        let _guard = lock_power(&self.power_lock);
        match bridge_client::send_command(action, self.config.as_ref()) {
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

    fn handle_confirmation(&mut self, chat_id: i64, action: PowerAction, supplied: &str) {
        if !self.consume_pending(action, supplied) {
            let reply = format!(
                "有効な{}確認がありません。期限切れ、使用済み、またはnonce不一致です。\nもう一度 /{} から実行してください。",
                action.label_ja(),
                action.slug()
            );
            self.api.send_message(chat_id, &reply);
            return;
        }
        let reply = self.run_power_action(action);
        self.api.send_message(chat_id, &reply);
    }

    fn dispatch_command(&mut self, chat_id: i64, command: &str, args: &str) {
        // 操作系コマンドはロック中に一切実行しない。ロックの切り替えと状態確認は
        // ロック中でも受け付ける(そうしないと解除できない)。
        const POWER_COMMANDS: [&str; 5] = [
            "/wake",
            "/reboot",
            "/shutdown",
            "/confirm_reboot",
            "/confirm_shutdown",
        ];
        if self.operation_lock.is_locked() && POWER_COMMANDS.contains(&command) {
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
            "/lock" => {
                self.operation_lock.set(true);
                // 保留中の確認も無効化しておく(ロック直前に発行された確認を
                // 解除後に使い回せてしまうのを防ぐ)。
                self.pending = None;
                self.api
                    .send_message(chat_id, "操作をロックしました。/unlock で解除できます。");
            }
            "/unlock" => {
                self.operation_lock.set(false);
                self.api.send_message(chat_id, "操作のロックを解除しました。");
            }
            "/wake" => {
                let _guard = lock_power(&self.power_lock);
                let reply = match net::send_wake_on_lan(
                    &self.config.pc_mac_address,
                    self.config.wol_port,
                ) {
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
            // 未知のコマンドは静かに無視する。
            _ => {}
        }
    }

    /// callback_dataを解析する。形式外、古いボタン、別bot由来の値は拒否する。
    fn parse_callback_data(data: &str) -> Option<(bool, PowerAction, String)> {
        let mut parts = data.splitn(3, ':');
        let kind = parts.next()?;
        let slug = parts.next()?;
        let nonce = parts.next()?;
        if nonce.is_empty() {
            return None;
        }
        let is_confirm = match kind {
            "confirm" => true,
            "cancel" => false,
            _ => return None,
        };
        Some((is_confirm, PowerAction::from_slug(slug)?, nonce.to_string()))
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

        if self.operation_lock.is_locked() {
            // ロック中はボタンからの実行も拒否する。pendingは/lockで既に破棄済み。
            self.api.answer_callback_query(&id, "操作はロック中です");
            return;
        }

        let data = callback["data"].as_str().unwrap_or_default();
        let Some((is_confirm, action, nonce)) = Self::parse_callback_data(data) else {
            self.api.answer_callback_query(&id, "無効なボタンです");
            return;
        };

        let chat_id = callback["message"]["chat"]["id"]
            .as_i64()
            .unwrap_or_default();
        let valid = self.consume_pending(action, &nonce);

        if !is_confirm {
            self.api.answer_callback_query(
                &id,
                if valid {
                    "キャンセルしました"
                } else {
                    "処理済みです"
                },
            );
            if chat_id != 0 {
                let reply = if valid {
                    format!("{}をキャンセルしました。", action.label_ja())
                } else {
                    format!(
                        "有効な{}確認がありません。期限切れ、使用済み、またはnonce不一致です。",
                        action.label_ja()
                    )
                };
                self.api.send_message(chat_id, &reply);
            }
            return;
        }

        if !valid {
            self.api.answer_callback_query(&id, "期限切れまたは処理済みです");
            if chat_id != 0 {
                let reply = format!(
                    "有効な{}確認がありません。期限切れ、使用済み、またはnonce不一致です。\nもう一度 /{} から実行してください。",
                    action.label_ja(),
                    action.slug()
                );
                self.api.send_message(chat_id, &reply);
            }
            return;
        }

        let result = self.run_power_action(action);
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
        let url = format!(
            "{}?timeout={}&offset={}",
            self.api.api_url("getUpdates"),
            self.config.telegram_long_poll_timeout_seconds,
            self.last_update_id
        );
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
                return Err(format!("getUpdates response exceeds {RESPONSE_MAX_BYTES} bytes").into());
            }
            body.extend_from_slice(&chunk[..read]);
        }

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
            *state.lock().unwrap() = State::Error;
            return;
        }

        // NTP同期を短時間だけ待つ。未同期なら電源操作の送信側で拒否する。
        net::wait_for_time_sync(Duration::from_secs(10));

        let mut backoff = BACKOFF_MIN;
        *state.lock().unwrap() = State::Polling;

        loop {
            match self.poll_once() {
                Ok(()) => {
                    backoff = BACKOFF_MIN;
                    *state.lock().unwrap() = State::Polling;
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    println!("telegram poll error: {e}");
                    *state.lock().unwrap() = State::Error;
                    std::thread::sleep(backoff);
                    backoff = (backoff * 2).min(BACKOFF_MAX);
                }
            }
        }
    }
}
