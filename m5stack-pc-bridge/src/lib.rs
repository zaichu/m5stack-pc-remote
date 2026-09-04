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
