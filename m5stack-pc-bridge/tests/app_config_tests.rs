use m5stack_pc_bridge::app_config::AgentConfig;

#[test]
fn loads_minimal_toml_config() {
    let input = r#"
bind = "127.0.0.1:18080"
shared_secret = "0123456789abcdef0123456789abcdef"
allowed_skew_seconds = 90
dry_run = true
"#;

    let cfg = AgentConfig::from_toml_str(input).unwrap();

    assert_eq!(cfg.bind, "127.0.0.1:18080");
    assert_eq!(cfg.shared_secret, "0123456789abcdef0123456789abcdef");
    assert_eq!(cfg.allowed_skew_seconds, 90);
    assert!(cfg.dry_run);
}

#[test]
fn rejects_placeholder_shared_secret() {
    let input = r#"
bind = "127.0.0.1:18080"
shared_secret = "replace-with-a-long-random-shared-secret"
"#;

    let err = AgentConfig::from_toml_str(input).unwrap_err();

    assert!(err.to_string().contains("placeholder"));
}

#[test]
fn rejects_short_shared_secret() {
    let input = r#"
bind = "127.0.0.1:18080"
shared_secret = "too-short"
"#;

    let err = AgentConfig::from_toml_str(input).unwrap_err();

    assert!(err.to_string().contains("at least 32"));
}

#[test]
fn rejects_invalid_bind() {
    let input = r#"
bind = "not-an-address"
shared_secret = "0123456789abcdef0123456789abcdef"
"#;

    let err = AgentConfig::from_toml_str(input).unwrap_err();

    assert!(err.to_string().contains("socket address"));
}

#[test]
fn rejects_excessive_skew() {
    let input = r#"
bind = "127.0.0.1:18080"
shared_secret = "0123456789abcdef0123456789abcdef"
allowed_skew_seconds = 3601
"#;

    let err = AgentConfig::from_toml_str(input).unwrap_err();

    assert!(err.to_string().contains("at most"));
}

#[test]
fn rejects_placeholder_telegram_token() {
    let input = r#"
bind = "127.0.0.1:18080"
shared_secret = "0123456789abcdef0123456789abcdef"
telegram_bot_token = "replace-with-your-telegram-bot-token"
"#;

    let err = AgentConfig::from_toml_str(input).unwrap_err();

    assert!(err.to_string().contains("placeholder"));
}

// toml::de::Error をそのまま伝播させると、問題箇所の行の引用やserdeの型不一致
// メッセージ経由で shared_secret の実値がログへ出る。値を出さず行番号だけを
// 返すことをテストで固定する。
#[test]
fn parse_error_reports_line_number_without_leaking_values() {
    let input = r#"
bind = "127.0.0.1:18080"
shared_secret = "0123456789abcdef0123456789abcdef"
allowed_skew_seconds = "not-a-number"
"#;

    let err = AgentConfig::from_toml_str(input).unwrap_err();
    let message = format!("{err:?}");

    assert!(!message.contains("0123456789abcdef"), "message={message}");
    assert!(!message.contains("not-a-number"), "message={message}");
    assert!(message.contains("4行目付近"), "message={message}");
}

#[test]
fn syntax_error_does_not_leak_the_offending_line() {
    let input = r#"
bind = "127.0.0.1:18080"
shared_secret = "0123456789abcdef0123456789abcdef
"#;

    let err = AgentConfig::from_toml_str(input).unwrap_err();
    let message = format!("{err:?}");

    assert!(!message.contains("0123456789abcdef"), "message={message}");
    assert!(message.contains("解析できません"), "message={message}");
}
