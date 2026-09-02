pub mod app_config;
pub mod auth;
pub mod power;
pub mod server;

#[cfg(windows)]
pub mod windows_service;

/// 設定ファイルの既定パス。実行ファイルと同じディレクトリの`config.toml`を見る。
///
/// Windows ServiceはSCMから起動されるとカレントディレクトリが`%SystemRoot%\System32`
/// になるため、CWD相対パスに依存すると `--config` を省略したときに設定を見失う。
/// `install.ps1` は実行ファイルと`config.toml`を同じディレクトリへ配置するため、
/// 実行ファイルの場所を基準にすることでService/対話実行のどちらでも同じ挙動にする。
pub fn default_config_path() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("config.toml")))
        .unwrap_or_else(|| std::path::PathBuf::from("config.toml"))
}
