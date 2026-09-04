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

// MIN_SHARED_SECRET_LEN(32)の境界を固定する。下限を16へ緩めると31文字が
// 通ってしまい、このテストが落ちる。
#[test]
fn rejects_shared_secret_one_char_below_minimum() {
    let secret = "a".repeat(31);
    assert_eq!(secret.len(), 31);
    let input = format!("bind = \"127.0.0.1:18080\"\nshared_secret = \"{secret}\"\n");

    let err = AgentConfig::from_toml_str(&input).unwrap_err();

    assert!(err.to_string().contains("at least 32"));
}

// 32文字ちょうどは受け入れる。下限を33へ厳格化するとこのテストが落ちる。
#[test]
fn accepts_shared_secret_at_minimum_length() {
    let input = r#"
bind = "127.0.0.1:18080"
shared_secret = "0123456789abcdef0123456789abcdef"
"#;

    let cfg = AgentConfig::from_toml_str(input).unwrap();

    assert_eq!(cfg.shared_secret, "0123456789abcdef0123456789abcdef");
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

// MAX_SKEW_SECONDS(3600)の境界を固定する。上限を3599へ厳格化すると
// 3600ちょうどが通らなくなり、このテストが落ちる。
#[test]
fn accepts_skew_at_maximum() {
    let input = r#"
bind = "127.0.0.1:18080"
shared_secret = "0123456789abcdef0123456789abcdef"
allowed_skew_seconds = 3600
"#;

    let cfg = AgentConfig::from_toml_str(input).unwrap();

    assert_eq!(cfg.allowed_skew_seconds, 3600);
}

// `allowed_skew_seconds <= 0` の拒否を固定する。条件を `< 0` へ緩めると
// 0が通ってしまい、`rejects_zero_allowed_skew` が落ちる。
#[test]
fn rejects_zero_allowed_skew() {
    let input = r#"
bind = "127.0.0.1:18080"
shared_secret = "0123456789abcdef0123456789abcdef"
allowed_skew_seconds = 0
"#;

    let err = AgentConfig::from_toml_str(input).unwrap_err();

    assert!(err.to_string().contains("positive"));
}

#[test]
fn rejects_negative_allowed_skew() {
    let input = r#"
bind = "127.0.0.1:18080"
shared_secret = "0123456789abcdef0123456789abcdef"
allowed_skew_seconds = -60
"#;

    let err = AgentConfig::from_toml_str(input).unwrap_err();

    assert!(err.to_string().contains("positive"));
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
