use std::env;
use std::fs;
use std::path::Path;

fn main() {
    if env::var("TARGET").is_ok_and(|target| target.contains("espidf")) {
        embuild::espidf::sysenv::output();
    }

    generate_config();
}

/// Git管理外の `config.toml` を読み、`$OUT_DIR/generated_config.rs` を生成する。
/// `src/main.rs` はこれを `config` moduleとして取り込む。secretを `src/` 配下の
/// Rustソースに置かないことで、コンパイラ警告に実値が出る事故を避ける。
/// 生成する定数には `#[allow(dead_code)]` を付け、未使用警告自体も出さない。
fn generate_config() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let config_path = Path::new(&manifest_dir).join("config.toml");
    println!("cargo:rerun-if-changed={}", config_path.display());

    if !config_path.exists() {
        panic!(
            "firmware-rust-poc/config.toml が見つかりません。config.example.toml を同じ \
             ディレクトリの config.toml へコピーし、実際の値を設定してください。詳細は \
             firmware-rust-poc/README.md を参照してください。"
        );
    }

    let raw = fs::read_to_string(&config_path).expect("config.toml の読み込みに失敗しました");
    let table: toml::Table = raw.parse().expect(
        "config.toml をTOMLとして解析できませんでした。secretを含む可能性があるため内容は表示しません",
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
        panic!("config.toml に必須key `{key}` がありません。config.example.toml を確認してください")
    })
}

fn push_str_const(out: &mut String, table: &toml::Table, key: &str, const_name: &str) {
    let value = require(table, key)
        .as_str()
        .unwrap_or_else(|| panic!("config.toml: `{key}` は文字列で指定してください"));
    out.push_str("#[allow(dead_code)]\n");
    out.push_str(&format!("pub const {const_name}: &str = {value:?};\n\n"));
}

fn push_u16_const(out: &mut String, table: &toml::Table, key: &str, const_name: &str) {
    let value = require(table, key)
        .as_integer()
        .unwrap_or_else(|| panic!("config.toml: `{key}` は整数で指定してください"));
    out.push_str("#[allow(dead_code)]\n");
    out.push_str(&format!("pub const {const_name}: u16 = {value};\n\n"));
}

fn push_u32_const(out: &mut String, table: &toml::Table, key: &str, const_name: &str) {
    let value = require(table, key)
        .as_integer()
        .unwrap_or_else(|| panic!("config.toml: `{key}` は整数で指定してください"));
    out.push_str("#[allow(dead_code)]\n");
    out.push_str(&format!("pub const {const_name}: u32 = {value};\n\n"));
}

fn push_u64_const(out: &mut String, table: &toml::Table, key: &str, const_name: &str) {
    let value = require(table, key)
        .as_integer()
        .unwrap_or_else(|| panic!("config.toml: `{key}` は整数で指定してください"));
    out.push_str("#[allow(dead_code)]\n");
    out.push_str(&format!("pub const {const_name}: u64 = {value};\n\n"));
}
