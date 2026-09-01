use std::{fs, path::Path};

use serde::Deserialize;

const PLACEHOLDER_SHARED_SECRET: &str = "replace-with-a-long-random-shared-secret";
const MIN_SHARED_SECRET_LEN: usize = 32;

#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfig {
    pub bind: String,
    pub shared_secret: String,
    #[serde(default = "default_allowed_skew_seconds")]
    pub allowed_skew_seconds: i64,
    #[serde(default = "default_dry_run")]
    pub dry_run: bool,
}

fn default_allowed_skew_seconds() -> i64 {
    60
}

fn default_dry_run() -> bool {
    true
}

impl AgentConfig {
    pub fn from_toml_str(input: &str) -> anyhow::Result<Self> {
        let config: Self = toml::from_str(input)?;
        config.validate()?;
        Ok(config)
    }

    pub fn from_path(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let input = fs::read_to_string(path)?;
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
        Ok(())
    }
}
