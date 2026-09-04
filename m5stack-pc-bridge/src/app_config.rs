use std::{fs, net::SocketAddr, path::Path};

use serde::Deserialize;

const PLACEHOLDER_SHARED_SECRET: &str = "replace-with-a-long-random-shared-secret";
const PLACEHOLDER_TELEGRAM_TOKEN: &str = "replace-with-your-telegram-bot-token";
const MIN_SHARED_SECRET_LEN: usize = 32;
const MAX_SKEW_SECONDS: i64 = 3600;

// `shared_secret` と `telegram_bot_token` を含むため `Debug` は手書きし、値を出力しない。
// ログや panic メッセージへ `{:?}` で secret が漏れる事故を防ぐ。
#[derive(Deserialize)]
pub struct AgentConfig {
    pub bind: String,
    pub shared_secret: String,
    #[serde(default = "default_allowed_skew_seconds")]
    pub allowed_skew_seconds: i64,
    #[serde(default = "default_dry_run")]
    pub dry_run: bool,
    /// 認証失敗アラートの送信先。両方揃ったときだけ通知を有効にする。
    /// 未設定なら通知しないだけで、電源操作の動作には影響しない。
    #[serde(default)]
    pub telegram_bot_token: Option<String>,
    #[serde(default)]
    pub telegram_chat_id: Option<i64>,
}

impl std::fmt::Debug for AgentConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentConfig")
            .field("bind", &self.bind)
            .field("shared_secret", &"[REDACTED]")
            .field("allowed_skew_seconds", &self.allowed_skew_seconds)
            .field("dry_run", &self.dry_run)
            .field(
                "telegram_bot_token",
                &self.telegram_bot_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("telegram_chat_id", &self.telegram_chat_id)
            .finish()
    }
}

fn default_allowed_skew_seconds() -> i64 {
    60
}

fn default_dry_run() -> bool {
    true
}

impl AgentConfig {
    pub fn from_toml_str(input: &str) -> anyhow::Result<Self> {
        let config: Self =
            toml::from_str(input).map_err(|_| anyhow::anyhow!("config.tomlを解析できません"))?;
        config.validate()?;
        Ok(config)
    }

    pub fn from_path(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let input = fs::read_to_string(path.as_ref())?;
        Self::from_toml_str(&input)
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.shared_secret == PLACEHOLDER_SHARED_SECRET {
            anyhow::bail!("shared_secret must not be the placeholder value");
        }
        if self.shared_secret.len() < MIN_SHARED_SECRET_LEN {
            anyhow::bail!("shared_secret must be at least 32 characters");
        }
        if self.allowed_skew_seconds <= 0 {
            anyhow::bail!("allowed_skew_seconds must be positive");
        }
        if self.allowed_skew_seconds > MAX_SKEW_SECONDS {
            anyhow::bail!("allowed_skew_seconds must be at most {MAX_SKEW_SECONDS} seconds");
        }
        self.bind
            .parse::<SocketAddr>()
            .map_err(|e| anyhow::anyhow!("bind is not a valid socket address: {e}"))?;
        if self
            .telegram_bot_token
            .as_deref()
            .is_some_and(|t| t == PLACEHOLDER_TELEGRAM_TOKEN)
        {
            anyhow::bail!("telegram_bot_token must not be the placeholder value");
        }
        Ok(())
    }
}
