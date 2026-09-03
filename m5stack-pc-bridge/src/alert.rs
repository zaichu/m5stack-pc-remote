//! HTTP認証失敗が続いたときに、Telegramへアラートを送る。
//!
//! bot tokenをWindows側にも置くことになるが、
//! - このconfig.tomlには既に`shared_secret`(電源操作を直接authorizeする、より強い鍵)がある
//! - ファイルを読める攻撃者は既にそのPC上におり、`shutdown.exe`を直接実行できる
//! - 同じtokenはM5Stack側のflash(暗号化なし)に平文で載っており、そちらの方が保護が弱い
//!
//! ため、全体のリスクはほとんど変わらないと判断して許容している。詳細は
//! `docs/security.md` と Issue #43 を参照。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pc_remote_signing::AlertThrottle;

use crate::app_config::AgentConfig;

/// Telegram APIへの接続・応答待ちの上限。電源操作の応答を巻き込まないよう短くする。
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

pub struct AlertNotifier {
    bot_token: String,
    chat_id: i64,
    /// 抑制ポリシー(閾値・間隔)はfirmwareと共有する。
    throttle: Mutex<AlertThrottle>,
}

impl AlertNotifier {
    /// bot tokenとchat_idが両方設定されているときだけ有効にする。
    pub fn from_config(config: &AgentConfig) -> Option<Self> {
        let bot_token = config.telegram_bot_token.clone()?;
        let chat_id = config.telegram_chat_id?;
        if bot_token.trim().is_empty() {
            return None;
        }
        Some(Self {
            bot_token,
            chat_id,
            throttle: Mutex::new(AlertThrottle::default()),
        })
    }

    /// 認証失敗を1件記録し、閾値と送信間隔を満たしていれば送信すべき件数を返す。
    fn record(&self) -> Option<u32> {
        // Mutexが毒された場合でもリクエスト処理は続けたいので、失敗時は通知を諦める。
        self.throttle.lock().ok()?.record(Instant::now())
    }

    /// 認証失敗を記録し、必要ならバックグラウンドでアラートを送る。
    ///
    /// 送信はHTTPSで数秒かかり得るため、リクエスト処理スレッドでは待たない。
    /// 送信タスクへ渡すため`Arc<Self>`のメソッドにしている。
    pub fn record_auth_failure(self: &Arc<Self>) {
        let Some(count) = self.record() else {
            return;
        };

        let notifier = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            if let Err(e) = notifier.send_alert(count) {
                tracing::warn!(
                    "failed to send auth failure alert: {}",
                    notifier.redact(&e.to_string())
                );
            }
        });
    }

    /// エラー文字列からbot tokenを伏せる。
    ///
    /// `ureq::Error`は変種によってURIをそのまま含む(`BadUri`など)。URLには
    /// bot tokenが入るため、ログへ出す前に必ず通す。
    fn redact(&self, message: &str) -> String {
        message.replace(&self.bot_token, "[REDACTED]")
    }

    #[cfg(test)]
    fn for_test(bot_token: &str) -> Self {
        Self {
            bot_token: bot_token.to_string(),
            chat_id: 1,
            throttle: Mutex::new(AlertThrottle::default()),
        }
    }

    fn send_alert(&self, count: u32) -> Result<(), ureq::Error> {
        // 通知本文には送信元IPやヘッダー値を含めない。攻撃者が自由に決められる
        // 文字列を自分のチャットへ流すと、なりすましや誘導の材料になるため。
        let text = format!(
            "m5stack-pc-bridge: 認証に失敗したリクエストを{count}件検知しました。\n電源操作は実行されていません。"
        );

        // URLにbot tokenが入る。エラーを記録する側は必ず`redact`を通すこと。
        let url = format!("https://api.telegram.org/bot{}/sendMessage", self.bot_token);
        ureq::post(&url)
            .config()
            .timeout_global(Some(SEND_TIMEOUT))
            .build()
            .send_json(serde_json::json!({
                "chat_id": self.chat_id,
                "text": text,
            }))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_bot_token_from_error_text() {
        let notifier = AlertNotifier::for_test("123456:SECRET-TOKEN");
        // ureq::Error::BadUri などはURIをそのまま含むため、tokenが混ざり得る。
        let raw = "bad uri: https://api.telegram.org/bot123456:SECRET-TOKEN/sendMessage";

        let redacted = notifier.redact(raw);

        assert!(
            !redacted.contains("SECRET-TOKEN"),
            "token leaked: {redacted}"
        );
        assert!(redacted.contains("[REDACTED]"));
    }

    #[test]
    fn alerts_only_after_threshold_and_respects_interval() {
        let notifier = AlertNotifier::for_test("token");

        assert_eq!(notifier.record(), None);
        assert_eq!(notifier.record(), None);
        // 閾値到達で件数を返し、カウンタはリセットされる。
        assert_eq!(notifier.record(), Some(AlertThrottle::DEFAULT_THRESHOLD));
        // 直後は送信間隔を満たさないため、閾値に再到達しても送らない。
        for _ in 0..AlertThrottle::DEFAULT_THRESHOLD {
            assert_eq!(notifier.record(), None);
        }
    }
}
