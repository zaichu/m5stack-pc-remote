use pc_remote_agent::app_config::AgentConfig;

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
