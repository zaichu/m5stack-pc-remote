use std::error::Error;
use std::sync::Arc;
use std::time::{Duration, Instant};

use embedded_svc::http::Method;
use embedded_svc::http::client::Client as HttpClient;
use esp_idf_svc::http::client::{Configuration as HttpConfiguration, EspHttpConnection};
use serde_json::{Value, json};

use crate::app_config::AppConfig;

use super::{HttpsLock, RESPONSE_BUFFER, lock_https};

/// Bot APIへのHTTP呼び出し。pollingスレッドと通知スレッドの両方から使う。
/// 両スレッドは同じ `HttpsLock` を共有し、TLS接続を直列化する(Issue #127)。
pub(crate) struct Api {
    pub(crate) config: Arc<AppConfig>,
    pub(crate) https: HttpsLock,
}
impl Api {
    /// URLにはbot tokenが入るため、絶対にログへ出さない。
    pub(crate) fn api_url(&self, method: &str) -> String {
        format!(
            "https://api.telegram.org/bot{}/{method}",
            self.config.telegram_bot_token
        )
    }

    pub(crate) fn http_client(&self) -> Result<HttpClient<EspHttpConnection>, Box<dyn Error>> {
        Ok(HttpClient::wrap(EspHttpConnection::new(
            &HttpConfiguration {
                use_global_ca_store: true,
                timeout: Some(Duration::from_secs(
                    u64::from(self.config.telegram_long_poll_timeout_seconds) + 10,
                )),
                buffer_size: Some(RESPONSE_BUFFER),
                ..Default::default()
            },
        )?))
    }

    /// Bot APIへJSONをPOSTする。URLとbodyはtokenや本文を含み得るためログへ出さない。
    pub(crate) fn post_json(&self, method: &str, body: &Value) -> Result<(), Box<dyn Error>> {
        // Issue #127: 保持中はHTTP送受信だけを行い、他のロックは取らない(leaf扱い)。
        let _https_guard = lock_https(&self.https);

        let mut client = self.http_client()?;
        self.post_via(&mut client, method, body)
    }

    /// 既存の接続ハンドルでJSONを1回POSTする。**呼び出し側が `HttpsLock` を握ること。**
    /// ログにはmethod名とミリ秒だけを出す(URLはtoken、bodyは本文を含む)。
    /// 未読応答は次回 `request` 時に `initiate_request` が捨てる。
    pub(crate) fn post_via(
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
    pub(crate) fn send_message_returning_id(&self, chat_id: i64, text: &str) -> Option<i64> {
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
    pub(crate) fn edit_message_text(&self, chat_id: i64, message_id: i64, text: &str) -> bool {
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
    pub(crate) fn send_message(&self, chat_id: i64, text: &str) -> bool {
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
    pub(crate) fn send_message_with_keyboard(&self, chat_id: i64, text: &str, rows: Value) {
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

    /// `/status` 専用。`parse_mode=HTML` と操作ボタンを付けて送る。
    /// 既存の送信経路へは `parse_mode` を付けない(本文に `<` を含む通知が壊れるため)。
    pub(crate) fn send_status(&self, chat_id: i64, text: &str, rows: Value) {
        if let Err(e) = self.post_json(
            "sendMessage",
            &json!({
                "chat_id": chat_id,
                "text": text,
                "parse_mode": "HTML",
                "reply_markup": { "inline_keyboard": rows }
            }),
        ) {
            println!("telegram: sendMessage(status) failed: {e}");
        }
    }

    /// 「更新」ボタンの書き換え。内容が同じとき Telegram は 400 を返すので、
    /// 失敗しても呼び出し側は続ける。
    pub(crate) fn edit_status(&self, chat_id: i64, message_id: i64, text: &str, rows: Value) {
        if let Err(e) = self.post_json(
            "editMessageText",
            &json!({
                "chat_id": chat_id,
                "message_id": message_id,
                "text": text,
                "parse_mode": "HTML",
                "reply_markup": { "inline_keyboard": rows }
            }),
        ) {
            println!("telegram: editMessageText(status) failed: {e}");
        }
    }

    pub(crate) fn send_message_with_confirm_buttons(
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
    pub(crate) fn send_force_reply(&self, chat_id: i64, text: &str, placeholder: &str) {
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
    pub(crate) fn answer_callback_query(&self, id: &str, text: &str) {
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
    pub(crate) fn answer_callback_query_and_send(
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
        if chat_id != 0
            && let Err(e) = self.post_via(
                &mut client,
                "sendMessage",
                &json!({ "chat_id": chat_id, "text": send_text }),
            )
        {
            println!("telegram: sendMessage failed: {e}");
        }
    }
}
