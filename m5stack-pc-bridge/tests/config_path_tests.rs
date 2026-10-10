//! Windows foreground 実行時の `--config` / `M5STACK_PC_BRIDGE_CONFIG` 解決のテスト。
//!
//! 実ファイル・実ネットワークは使わない。`windows_service` 本体は
//! `cfg(windows)` で host ではコンパイルできないため、共有の `Cli` パーサと
//! 既定パス解決をここで固定する(Linux `main` も同じ `Cli` を使う)。

use clap::Parser;
use m5stack_pc_bridge::{default_config_path, Cli};
use std::path::PathBuf;

#[test]
fn parses_space_separated_config_flag() {
    let cli = Cli::try_parse_from(["m5stack-pc-bridge", "--config", "/tmp/a.toml"]).unwrap();
    assert_eq!(cli.config_path(), PathBuf::from("/tmp/a.toml"));
}

#[test]
fn parses_equals_form_config_flag() {
    let cli = Cli::try_parse_from(["m5stack-pc-bridge", "--config=/tmp/b.toml"]).unwrap();
    assert_eq!(cli.config_path(), PathBuf::from("/tmp/b.toml"));
}

#[test]
fn rejects_unknown_arg_and_missing_value() {
    // 手書きパーサは未知の引数や値なし `--config` を無視していたが、
    // clap 化後は Linux `main` と同じくエラー終了になる。
    assert!(Cli::try_parse_from(["m5stack-pc-bridge", "--verbose"]).is_err());
    assert!(Cli::try_parse_from(["m5stack-pc-bridge", "--config"]).is_err());
}

#[test]
fn rejects_duplicate_config_flag() {
    // 手書きパーサの「最後を優先」から、clap既定の重複エラーへ揃えた。
    let result = Cli::try_parse_from([
        "m5stack-pc-bridge",
        "--config",
        "/tmp/first.toml",
        "--config=/tmp/second.toml",
    ]);
    assert!(result.is_err());
}

#[test]
fn env_and_fallback_resolution() {
    // この env var を読み書きするのはこのテストだけなので、set/remove を
    // 同じテスト内へ閉じ込めれば他テストとの並行実行で競合しない。
    const ENV_VAR: &str = "M5STACK_PC_BRIDGE_CONFIG";

    std::env::set_var(ENV_VAR, "/tmp/env.toml");
    let cli = Cli::try_parse_from(["m5stack-pc-bridge"]).unwrap();
    assert_eq!(cli.config_path(), PathBuf::from("/tmp/env.toml"));

    // CLI指定はenvより優先される。
    let cli = Cli::try_parse_from(["m5stack-pc-bridge", "--config", "/tmp/cli.toml"]).unwrap();
    assert_eq!(cli.config_path(), PathBuf::from("/tmp/cli.toml"));

    // 空の値(`--config=`、空の環境変数)はclapがエラーにする(手書きパーサの
    // 「無視して既定パスへフォールバック」から挙動が変わる点)。
    std::env::set_var(ENV_VAR, "");
    assert!(Cli::try_parse_from(["m5stack-pc-bridge"]).is_err());
    assert!(Cli::try_parse_from(["m5stack-pc-bridge", "--config="]).is_err());

    std::env::remove_var(ENV_VAR);
    let cli = Cli::try_parse_from(["m5stack-pc-bridge"]).unwrap();
    assert_eq!(cli.config_path(), default_config_path());
}
