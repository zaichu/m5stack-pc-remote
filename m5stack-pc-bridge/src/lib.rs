pub mod alert;
pub mod app_config;
pub mod audit_log;
pub mod auth;
pub mod firmware;
pub mod power;
pub mod server;
pub mod suspend;

#[cfg(windows)]
pub mod windows_service;

/// 実行ファイルと同じディレクトリにある`name`のパスを返す。
/// Windows ServiceはSCMから起動されるとCWDがSystem32になるため、CWD相対だと
/// 設定やログの場所を見失う。
pub fn exe_dir_file(name: &str) -> std::path::PathBuf {
    if let Some(path) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
    {
        path
    } else {
        tracing::warn!(
            "current_exe() が取得できず CWD 相対パスへフォールバックします: {}",
            name
        );
        std::path::PathBuf::from(name)
    }
}

/// `--config` / `M5STACK_PC_BRIDGE_CONFIG` を解釈するCLI引数。Linux `main` と
/// Windows foreground の両経路でこのパーサを共有し、挙動を揃える。
/// SCM経由のservice起動では使わない(service登録引数とは別物で、設定は既定パス固定)。
#[derive(Debug, clap::Parser)]
pub struct Cli {
    /// 設定ファイルのパス。
    #[arg(long, env = "M5STACK_PC_BRIDGE_CONFIG")]
    pub config: Option<std::path::PathBuf>,
}

impl Cli {
    /// `cli > env` の優先度は clap の `env` 属性が解決済み。未指定なら既定パスを返す。
    pub fn config_path(self) -> std::path::PathBuf {
        self.config.unwrap_or_else(default_config_path)
    }
}

/// 設定ファイルの既定パス。
pub fn default_config_path() -> std::path::PathBuf {
    exe_dir_file("config.toml")
}

/// 指定パスから設定を読み込む。foreground/service/`main` の各経路で同じエラー形式に
/// するためここへ置く。エラー文に secret は含まれない(`AgentConfig::validate` が汎化して返す)。
pub fn load_config(path: impl AsRef<std::path::Path>) -> anyhow::Result<app_config::AgentConfig> {
    let path = path.as_ref();
    app_config::AgentConfig::from_path(path)
        .map_err(|e| anyhow::anyhow!("failed to load {}: {e}", path.display()))
}
