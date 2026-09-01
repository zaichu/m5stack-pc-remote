use std::env;
use std::fs;
use std::path::Path;

fn main() {
    if env::var("TARGET").is_ok_and(|target| target.contains("espidf")) {
        embuild::espidf::sysenv::output();
    }

    generate_config();
}

/// Reads the git-ignored `config.toml` (see `config.example.toml`) and emits
/// `$OUT_DIR/generated_config.rs`, which `src/main.rs` pulls in via
/// `include!()` as the `config` module. Secrets never live in `src/` as Rust
/// source, so a compiler warning (e.g. unused import) can no longer print a
/// source line containing a real Wi-Fi password or Telegram bot token into
/// the build log (Issue #21). Each generated const also carries
/// `#[allow(dead_code)]` so an *unused* one still can't trigger that class of
/// warning in the first place.
fn generate_config() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let config_path = Path::new(&manifest_dir).join("config.toml");
    println!("cargo:rerun-if-changed={}", config_path.display());

    if !config_path.exists() {
        panic!(
            "firmware-rust-poc/config.toml not found. Copy config.example.toml to \
             config.toml (same directory) and fill in real values; see \
             firmware-rust-poc/README.md."
        );
    }

    let raw = fs::read_to_string(&config_path).expect("failed to read config.toml");
    let table: toml::Table = raw.parse().expect(
        "failed to parse config.toml as TOML (not printing its contents: it may hold secrets)",
    );

    let mut out = String::new();
    push_str_const(&mut out, &table, "wifi_ssid", "WIFI_SSID");
    push_str_const(&mut out, &table, "wifi_password", "WIFI_PASSWORD");
    push_str_const(&mut out, &table, "pc_mac_address", "PC_MAC_ADDRESS");
    push_u16_const(&mut out, &table, "wol_port", "WOL_PORT");
    push_str_const(&mut out, &table, "pc_status_addr", "PC_STATUS_ADDR");
    push_u16_const(&mut out, &table, "agent_port", "AGENT_PORT");
    push_str_const(
        &mut out,
        &table,
        "agent_shared_secret",
        "AGENT_SHARED_SECRET",
    );
    push_str_const(&mut out, &table, "pc_ip_address", "PC_IP_ADDRESS");
    push_str_const(&mut out, &table, "telegram_bot_token", "TELEGRAM_BOT_TOKEN");
    push_str_const(
        &mut out,
        &table,
        "telegram_allowed_user_id",
        "TELEGRAM_ALLOWED_USER_ID",
    );
    push_u32_const(
        &mut out,
        &table,
        "telegram_long_poll_timeout_seconds",
        "TELEGRAM_LONG_POLL_TIMEOUT_SECONDS",
    );
    push_u64_const(
        &mut out,
        &table,
        "telegram_confirm_ttl_secs",
        "TELEGRAM_CONFIRM_TTL_SECS",
    );

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
    let dest = Path::new(&out_dir).join("generated_config.rs");
    fs::write(&dest, out).expect("failed to write generated_config.rs");
}

fn require<'a>(table: &'a toml::Table, key: &str) -> &'a toml::Value {
    table.get(key).unwrap_or_else(|| {
        panic!("config.toml is missing required key `{key}` (see config.example.toml)")
    })
}

fn push_str_const(out: &mut String, table: &toml::Table, key: &str, const_name: &str) {
    let value = require(table, key)
        .as_str()
        .unwrap_or_else(|| panic!("config.toml: `{key}` must be a string"));
    out.push_str("#[allow(dead_code)]\n");
    out.push_str(&format!("pub const {const_name}: &str = {value:?};\n\n"));
}

fn push_u16_const(out: &mut String, table: &toml::Table, key: &str, const_name: &str) {
    let value = require(table, key)
        .as_integer()
        .unwrap_or_else(|| panic!("config.toml: `{key}` must be an integer"));
    out.push_str("#[allow(dead_code)]\n");
    out.push_str(&format!("pub const {const_name}: u16 = {value};\n\n"));
}

fn push_u32_const(out: &mut String, table: &toml::Table, key: &str, const_name: &str) {
    let value = require(table, key)
        .as_integer()
        .unwrap_or_else(|| panic!("config.toml: `{key}` must be an integer"));
    out.push_str("#[allow(dead_code)]\n");
    out.push_str(&format!("pub const {const_name}: u32 = {value};\n\n"));
}

fn push_u64_const(out: &mut String, table: &toml::Table, key: &str, const_name: &str) {
    let value = require(table, key)
        .as_integer()
        .unwrap_or_else(|| panic!("config.toml: `{key}` must be an integer"));
    out.push_str("#[allow(dead_code)]\n");
    out.push_str(&format!("pub const {const_name}: u64 = {value};\n\n"));
}
