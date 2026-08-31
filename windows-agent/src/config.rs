use std::{fs, path::Path};

use serde::Deserialize;

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
        Ok(toml::from_str(input)?)
    }

    pub fn from_path(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let input = fs::read_to_string(path)?;
        Self::from_toml_str(&input)
    }
}
