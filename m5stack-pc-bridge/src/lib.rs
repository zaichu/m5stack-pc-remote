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

/// `--config` に対応する環境変数名。Linux側 `main` の clap 定義と同じ文字列を
/// 使う(Windows foreground 経路もこの定数経由で読むため、両OSで名前がずれない)。
pub const CONFIG_ENV_VAR: &str = "M5STACK_PC_BRIDGE_CONFIG";

/// 設定ファイルの既定パス。
pub fn default_config_path() -> std::path::PathBuf {
    exe_dir_file("config.toml")
}

/// 既定パスから設定を読み込む。foreground/service/`main` の各経路で同じ挙動にするため
/// ここへ置く。エラー文に secret は含まれない(`AgentConfig::validate` が汎化して返す)。
pub fn load_default_config() -> anyhow::Result<app_config::AgentConfig> {
    let path = default_config_path();
    app_config::AgentConfig::from_path(&path)
        .map_err(|e| anyhow::anyhow!("failed to load {}: {e}", path.display()))
}

/// foreground 実行時の設定パス解決。優先度は `cli > env > 既定パス`
/// (Linux側 `main` の clap 定義と同じ順序)。
/// SCM経由のservice起動では呼ばないこと(`_arguments` とforeground引数の混同を避けるため、
/// service経路は `load_default_config()` を使う)。
pub fn resolve_config_path(
    cli_config: Option<std::path::PathBuf>,
    env_config: Option<std::path::PathBuf>,
) -> std::path::PathBuf {
    if let Some(path) = cli_config {
        return path;
    }
    if let Some(path) = env_config {
        return path;
    }
    default_config_path()
}

/// 環境変数の値の解釈。空文字は未設定扱いにする。
pub fn env_config_path(value: Option<std::ffi::OsString>) -> Option<std::path::PathBuf> {
    match value {
        Some(value) if !value.is_empty() => Some(std::path::PathBuf::from(value)),
        _ => None,
    }
}

/// foreground 実行時の `std::env::args_os()` から `--config` の値だけを抜き出す
/// (SCM経由のservice起動では呼ばないこと)。
/// `--config <path>` と `--config=<path>` の両形式を受け付け、複数回指定時は
/// 最後を優先する。値なしの `--config` は無視する。
pub fn parse_config_arg(
    args: impl IntoIterator<Item = impl AsRef<std::ffi::OsStr>>,
) -> Option<std::path::PathBuf> {
    let mut result: Option<std::path::PathBuf> = None;
    let mut pending_value = false;
    for arg in args {
        let arg = arg.as_ref();
        if pending_value {
            pending_value = false;
            if !arg.is_empty() {
                result = Some(std::path::PathBuf::from(arg));
            }
            continue;
        }
        let Some(text) = arg.to_str() else {
            continue;
        };
        if text == "--config" {
            pending_value = true;
        } else if let Some(value) = text.strip_prefix("--config=") {
            if !value.is_empty() {
                result = Some(std::path::PathBuf::from(value));
            }
        }
    }
    result
}

/// foreground 実行時の設定読み込み。エラー文の形式は `load_default_config` と同じ。
pub fn load_foreground_config(
    cli_config: Option<std::path::PathBuf>,
    env_config: Option<std::path::PathBuf>,
) -> anyhow::Result<app_config::AgentConfig> {
    let path = resolve_config_path(cli_config, env_config);
    app_config::AgentConfig::from_path(&path)
        .map_err(|e| anyhow::anyhow!("failed to load {}: {e}", path.display()))
}
