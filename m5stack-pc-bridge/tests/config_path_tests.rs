//! Windows foreground 実行時の `--config` / `M5STACK_PC_BRIDGE_CONFIG` 解決のテスト。
//!
//! 実ファイル・実ネットワークは使わない。`windows_service` 本体は
//! `cfg(windows)` で host ではコンパイルできないため、解決ロジックを
//! `lib.rs` の純粋関数へ切り出し、ここで優先度と引数解析を固定する。

use m5stack_pc_bridge::{env_config_path, parse_config_arg, resolve_config_path, CONFIG_ENV_VAR};
use std::path::PathBuf;

#[test]
fn config_env_var_name_matches_clap_definition() {
    assert_eq!(CONFIG_ENV_VAR, "M5STACK_PC_BRIDGE_CONFIG");
}

#[test]
fn cli_takes_precedence_over_env() {
    let resolved = resolve_config_path(
        Some(PathBuf::from("/tmp/cli-config.toml")),
        Some(PathBuf::from("/tmp/env-config.toml")),
    );
    assert_eq!(resolved, PathBuf::from("/tmp/cli-config.toml"));
}

#[test]
fn env_is_used_when_cli_is_absent() {
    let resolved = resolve_config_path(None, Some(PathBuf::from("/tmp/env-config.toml")));
    assert_eq!(resolved, PathBuf::from("/tmp/env-config.toml"));
}

#[test]
fn falls_back_to_default_when_neither_cli_nor_env() {
    let resolved = resolve_config_path(None, None);
    assert_eq!(resolved, m5stack_pc_bridge::default_config_path());
}

#[test]
fn env_value_interpretation_ignores_empty() {
    assert_eq!(env_config_path(None), None);
    assert_eq!(
        env_config_path(Some("".into())),
        None,
        "空文字は未設定扱いにする",
    );
    assert_eq!(
        env_config_path(Some("/tmp/env-config.toml".into())),
        Some(PathBuf::from("/tmp/env-config.toml")),
    );
}

#[test]
fn parses_space_separated_config_flag() {
    let parsed = parse_config_arg(["--config", "/tmp/a.toml"]);
    assert_eq!(parsed, Some(PathBuf::from("/tmp/a.toml")));
}

#[test]
fn parses_equals_form_config_flag() {
    let parsed = parse_config_arg(["--config=/tmp/b.toml"]);
    assert_eq!(parsed, Some(PathBuf::from("/tmp/b.toml")));
}

#[test]
fn ignores_unrelated_args_and_missing_value() {
    // 他の引数に混ざっても `--config` だけを抜き出す。
    let parsed = parse_config_arg(["--verbose", "--config", "/tmp/c.toml", "--other"]);
    assert_eq!(parsed, Some(PathBuf::from("/tmp/c.toml")));

    // 値なしの `--config` は無視する(既定パスへフォールバックさせる)。
    assert_eq!(parse_config_arg(["--config"] as [&str; 1]), None);
    assert_eq!(parse_config_arg([] as [&str; 0]), None);
    assert_eq!(parse_config_arg(["--config="]), None);
}

#[test]
fn last_config_flag_wins() {
    let parsed = parse_config_arg(["--config", "/tmp/first.toml", "--config=/tmp/second.toml"]);
    assert_eq!(parsed, Some(PathBuf::from("/tmp/second.toml")));
}

#[test]
fn foreground_resolution_prefers_cli_over_env_end_to_end() {
    // Windows foreground の組み立て順(cli → env → 既定)そのままを確認する。
    let cli = parse_config_arg(["--config", "/tmp/cli.toml"]);
    let env = env_config_path(Some("/tmp/env.toml".into()));
    assert_eq!(
        resolve_config_path(cli, env),
        PathBuf::from("/tmp/cli.toml"),
    );

    let cli: Option<PathBuf> = parse_config_arg([] as [&str; 0]);
    let env = env_config_path(Some("/tmp/env.toml".into()));
    assert_eq!(
        resolve_config_path(cli, env),
        PathBuf::from("/tmp/env.toml"),
    );
}
