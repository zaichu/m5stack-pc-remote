use pc_remote_agent::config::AgentConfig;

#[test]
fn loads_minimal_toml_config() {
    let input = r#"
bind = "127.0.0.1:18080"
shared_secret = "local-development-secret"
allowed_skew_seconds = 90
dry_run = true
"#;

    let cfg = AgentConfig::from_toml_str(input).unwrap();

    assert_eq!(cfg.bind, "127.0.0.1:18080");
    assert_eq!(cfg.shared_secret, "local-development-secret");
    assert_eq!(cfg.allowed_skew_seconds, 90);
    assert!(cfg.dry_run);
}
