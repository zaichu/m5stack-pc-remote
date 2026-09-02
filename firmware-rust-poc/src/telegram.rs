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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use embedded_svc::http::client::Client as HttpClient;
use embedded_svc::http::Method;
use esp_idf_svc::http::client::{Configuration as HttpConfiguration, EspHttpConnection};
use serde_json::{json, Value};

use crate::agent::{self, PowerAction};
use crate::config::{
    PC_MAC_ADDRESS, TELEGRAM_ALLOWED_USER_ID, TELEGRAM_BOT_TOKEN, TELEGRAM_CONFIRM_TTL_SECS,
    TELEGRAM_LONG_POLL_TIMEOUT_SECONDS, WOL_PORT,
};
use crate::net;
use crate::telegram_root_ca::TELEGRAM_ROOT_CA_PEM;

const PLACEHOLDER_TOKEN: &str = "replace-with-your-telegram-bot-token";
const PLACEHOLDER_USER_ID: &str = "replace-with-your-telegram-user-id";

const BACKOFF_MIN: Duration = Duration::from_secs(5);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
const RESPONSE_BUFFER: usize = 4096;

/// UIスレッドへ共有するTelegram状態。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Disabled,
    Polling,
    Error,
}

/// タッチUIとTelegramスレッドからの電源操作を直列化するロック。
pub type PowerLock = Arc<Mutex<()>>;

pub fn is_configured() -> bool {
    !TELEGRAM_BOT_TOKEN.is_empty()
        && TELEGRAM_BOT_TOKEN != PLACEHOLDER_TOKEN
        && !TELEGRAM_ALLOWED_USER_ID.is_empty()
        && TELEGRAM_ALLOWED_USER_ID != PLACEHOLDER_USER_ID
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
}

impl Client {
    pub fn new(power_lock: PowerLock) -> Self {
        Self {
            last_update_id: 0,
            initial_sync_done: false,
            pending: None,
            power_lock,
        }
    }

    /// URLにはbot tokenが入るため、絶対にログへ出さない。
    fn api_url(method: &str) -> String {
        format!("https://api.telegram.org/bot{TELEGRAM_BOT_TOKEN}/{method}")
    }

    fn http_client() -> Result<HttpClient<EspHttpConnection>, Box<dyn Error>> {
        Ok(HttpClient::wrap(EspHttpConnection::new(
            &HttpConfiguration {
                use_global_ca_store: true,
                timeout: Some(Duration::from_secs(
                    TELEGRAM_LONG_POLL_TIMEOUT_SECONDS as u64 + 10,
                )),
                buffer_size: Some(RESPONSE_BUFFER),
                ..Default::default()
            },
        )?))
    }

    /// Bot APIへJSONをPOSTする。URLとbodyはtokenや本文を含み得るためログへ出さない。
    fn post_json(method: &str, body: &Value) -> Result<(), Box<dyn Error>> {
        use esp_idf_svc::io::Write;

        let payload = serde_json::to_string(body)?;
        let url = Self::api_url(method);
        let mut client = Self::http_client()?;
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
            println!("telegram {method} failed: {status}");
        }
        Ok(())
    }

    fn send_reply(chat_id: i64, text: &str) {
        let _ = Self::post_json("sendMessage", &json!({ "chat_id": chat_id, "text": text }));
    }

    /// Sends `text` with one row of two inline buttons. Both callback_data
    /// values stay well inside Telegram's 1-64 byte limit.
    fn send_reply_with_confirm_buttons(
        chat_id: i64,
        text: &str,
        confirm_label: &str,
        confirm_data: &str,
        cancel_data: &str,
    ) {
        let _ = Self::post_json(
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

    /// Telegram requires every callback_query to be acknowledged, or the
    /// tapping client keeps showing a loading spinner.
    fn answer_callback_query(id: &str, text: &str) {
        let mut body = json!({ "callback_query_id": id });
        if !text.is_empty() {
            body["text"] = json!(text);
        }
        let _ = Self::post_json("answerCallbackQuery", &body);
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
            net::check_pc_online(crate::config::PC_STATUS_ADDR, Duration::from_millis(800));
        format!(
            "PC: {}\nM5Stack: Rust firmware",
            if online {
                "オンライン"
            } else {
                "オフライン"
            }
        )
    }

    fn request_confirmation(&mut self, chat_id: i64, action: PowerAction) {
        let nonce = Self::generate_nonce();
        self.pending = Some(Pending {
            action,
            nonce: nonce.clone(),
            expires_at: Instant::now() + Duration::from_secs(TELEGRAM_CONFIRM_TTL_SECS),
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
        Self::send_reply_with_confirm_buttons(
            chat_id,
            &text,
            action.label_ja(),
            &confirm_data,
            &cancel_data,
        );
    }

    /// Checks `action`/`supplied` against the pending confirmation and always
    /// consumes it, so a nonce cannot be replayed or brute-forced across
    /// attempts regardless of the outcome.
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
        let _guard = self.power_lock.lock();
        match agent::send_command(action) {
            Ok(code) if agent::is_accepted(code) => {
                format!("{}を受け付けました。", action.label_ja())
            }
            Ok(code) => format!("{}に失敗しました。({code})", action.label_ja()),
            Err(e) => {
                println!("agent command failed: {e}");
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
            Self::send_reply(chat_id, &reply);
            return;
        }
        let reply = self.run_power_action(action);
        Self::send_reply(chat_id, &reply);
    }

    fn dispatch_command(&mut self, chat_id: i64, command: &str, args: &str) {
        match command {
            "/status" => Self::send_reply(chat_id, &self.status_text()),
            "/wake" => {
                let _guard = self.power_lock.lock();
                let reply = match net::send_wake_on_lan(PC_MAC_ADDRESS, WOL_PORT) {
                    Ok(()) => "WOLを送信しました。",
                    Err(e) => {
                        println!("WOL failed: {e}");
                        "WOL送信に失敗しました。"
                    }
                };
                drop(_guard);
                Self::send_reply(chat_id, reply);
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
        if from_id.to_string() != TELEGRAM_ALLOWED_USER_ID {
            // 権限がない場合はpending確認を触らず、Telegram側の読み込み状態だけ終わらせる。
            Self::answer_callback_query(&id, "権限がありません");
            return;
        }

        let data = callback["data"].as_str().unwrap_or_default();
        let Some((is_confirm, action, nonce)) = Self::parse_callback_data(data) else {
            Self::answer_callback_query(&id, "無効なボタンです");
            return;
        };

        let chat_id = callback["message"]["chat"]["id"]
            .as_i64()
            .unwrap_or_default();
        let valid = self.consume_pending(action, &nonce);

        if !is_confirm {
            Self::answer_callback_query(
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
                Self::send_reply(chat_id, &reply);
            }
            return;
        }

        if !valid {
            Self::answer_callback_query(&id, "期限切れまたは処理済みです");
            if chat_id != 0 {
                let reply = format!(
                    "有効な{}確認がありません。期限切れ、使用済み、またはnonce不一致です。\nもう一度 /{} から実行してください。",
                    action.label_ja(),
                    action.slug()
                );
                Self::send_reply(chat_id, &reply);
            }
            return;
        }

        let result = self.run_power_action(action);
        Self::answer_callback_query(&id, &result);
        if chat_id != 0 {
            Self::send_reply(chat_id, &result);
        }
    }

    fn handle_message(&mut self, message: &Value) {
        let from_id = message["from"]["id"].as_i64().unwrap_or_default();
        if from_id.to_string() != TELEGRAM_ALLOWED_USER_ID {
            // 権限がないユーザーには返信しない。
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
            Self::api_url("getUpdates"),
            TELEGRAM_LONG_POLL_TIMEOUT_SECONDS,
            self.last_update_id
        );
        let mut client = Self::http_client()?;
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
        if let Err(e) = install_root_ca() {
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
