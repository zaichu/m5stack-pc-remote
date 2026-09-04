pub mod alert;
pub mod app_config;
pub mod audit_log;
pub mod auth;
pub mod firmware;
pub mod power;
pub mod server;

#[cfg(windows)]
pub mod windows_service;

/// 実行ファイルと同じディレクトリにある`name`のパスを返す。
///
/// Windows ServiceはSCMから起動されるとカレントディレクトリが`%SystemRoot%\System32`
/// になるため、CWD相対パスに依存すると設定やログの場所を見失う。`install.ps1` は
/// 実行ファイルと関連ファイルを同じディレクトリへ配置するため、実行ファイルの場所を
/// 基準にすることでService/対話実行のどちらでも同じ挙動にする。
///
/// 実行ファイルの場所が取れない場合はCWD相対へフォールバックする。
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

/// `--config` に対応する環境変数名。Linux側の `main` の clap 定義
/// (`#[arg(long, env = ...)]`) と同じ文字列を使う。Windows foreground 経路も
/// この定数経由で読むため、両OSで名前がずれない。
pub const CONFIG_ENV_VAR: &str = "M5STACK_PC_BRIDGE_CONFIG";

/// 設定ファイルの既定パス。`--config` 省略時に使う。
pub fn default_config_path() -> std::path::PathBuf {
    exe_dir_file("config.toml")
}

/// 既定パスから設定を読み込む。パス解決とエラー整形を一元化する。
///
/// `windows_service` の foreground/service 両経路と、通常の `main` で同じ
/// 挙動にするためここに置く。エラー文に secret は含まれない
///（`AgentConfig::validate` が汎化したメッセージを返す）。
pub fn load_default_config() -> anyhow::Result<app_config::AgentConfig> {
    let path = default_config_path();
    app_config::AgentConfig::from_path(&path)
        .map_err(|e| anyhow::anyhow!("failed to load {}: {e}", path.display()))
}

/// foreground 実行時の設定パス解決。純粋関数なので host でテストできる。
///
/// 優先度は `cli > env > 既定パス` で、Linux 側 `main` の clap 定義
/// (`#[arg(long, env = "M5STACK_PC_BRIDGE_CONFIG")]`) と同じ順序にする。
/// Windows Service(SCM 経由)では呼ばない。SCM が渡す `_arguments` と
/// foreground 実行時の引数を混同しないため、service 経路は従来どおり
/// `load_default_config()` を使う。
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

/// 環境変数 `M5STACK_PC_BRIDGE_CONFIG` の値の解釈。空文字は未設定扱いにする。
/// `std::env::var_os(CONFIG_ENV_VAR)` の結果をそのまま渡す。
pub fn env_config_path(value: Option<std::ffi::OsString>) -> Option<std::path::PathBuf> {
    match value {
        Some(value) if !value.is_empty() => Some(std::path::PathBuf::from(value)),
        _ => None,
    }
}

/// コマンドライン引数から `--config` の値だけを抜き出す。
///
/// SCM 経由の service 起動では呼ばないこと。`service_main` が受け取る引数とは
/// 関係なく、foreground 実行時の `std::env::args_os()` を渡すための関数。
/// `--config <path>` と `--config=<path>` の両形式を受け付け、複数回指定時は
/// 最後を優先する(clap の上書きに近づけるため)。値なしの `--config` は無視する。
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

/// foreground 実行時の設定読み込み。`--config` と環境変数を解決してから読む。
/// エラー文の形式は `load_default_config` と同じにする。
pub fn load_foreground_config(
    cli_config: Option<std::path::PathBuf>,
    env_config: Option<std::path::PathBuf>,
) -> anyhow::Result<app_config::AgentConfig> {
    let path = resolve_config_path(cli_config, env_config);
    app_config::AgentConfig::from_path(&path)
        .map_err(|e| anyhow::anyhow!("failed to load {}: {e}", path.display()))
}
