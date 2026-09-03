use std::env;
use std::fs;
use std::path::Path;

fn main() {
    if env::var("TARGET").is_ok_and(|target| target.contains("espidf")) {
        embuild::espidf::sysenv::output();
    }

    generate_config();
}

/// `daily_report_hour` がこの値(0-23の範囲外)なら定期レポートを送らない。
const DAILY_REPORT_DISABLED: i64 = -1;

/// config.tomlのkeyと、生成するRust定数の対応。新しい設定はここへ1行足す。
/// 並びはそのまま生成順になる。
const KEYS: &[Key] = &[
    Key::text("wifi_ssid", "WIFI_SSID"),
    Key::text("wifi_password", "WIFI_PASSWORD"),
    Key::text("pc_mac_address", "PC_MAC_ADDRESS"),
    Key::int("wol_port", "WOL_PORT", "u16"),
    Key::text("pc_status_addr", "PC_STATUS_ADDR"),
    Key::int("bridge_port", "BRIDGE_PORT", "u16").alias("agent_port"),
    Key::text("bridge_shared_secret", "BRIDGE_SHARED_SECRET").alias("agent_shared_secret"),
    Key::text("pc_ip_address", "PC_IP_ADDRESS"),
    Key::text("telegram_bot_token", "TELEGRAM_BOT_TOKEN"),
    Key::text("telegram_allowed_user_id", "TELEGRAM_ALLOWED_USER_ID"),
    Key::int(
        "telegram_long_poll_timeout_seconds",
        "TELEGRAM_LONG_POLL_TIMEOUT_SECONDS",
        "u32",
    ),
    Key::int(
        "telegram_confirm_ttl_secs",
        "TELEGRAM_CONFIRM_TTL_SECS",
        "u64",
    ),
    // 定期レポート関連は後から追加した任意keyなので、既存のconfig.tomlでも
    // ビルドが通るよう既定値を持たせる(必須にするとkey追加まで壊れる)。
    Key::int("daily_report_hour", "DAILY_REPORT_HOUR", "i64").default(DAILY_REPORT_DISABLED),
    Key::int("timezone_offset_hours", "TIMEZONE_OFFSET_HOURS", "i64").default(0),
];

enum Kind {
    Text,
    /// 生成する定数のRust型名。値はTOMLの整数から変換する。
    Int(&'static str),
}

struct Key {
    key: &'static str,
    /// 旧key。`key` が無ければこちらを見る。
    alias: Option<&'static str>,
    const_name: &'static str,
    kind: Kind,
    /// `Some` なら任意key。無いときはこの値を使う。整数keyのみ。
    default: Option<i64>,
}

impl Key {
    const fn text(key: &'static str, const_name: &'static str) -> Self {
        Self {
            key,
            alias: None,
            const_name,
            kind: Kind::Text,
            default: None,
        }
    }

    const fn int(key: &'static str, const_name: &'static str, ty: &'static str) -> Self {
        Self {
            key,
            alias: None,
            const_name,
            kind: Kind::Int(ty),
            default: None,
        }
    }

    const fn alias(mut self, alias: &'static str) -> Self {
        self.alias = Some(alias);
        self
    }

    const fn default(mut self, default: i64) -> Self {
        self.default = Some(default);
        self
    }
}

/// Git管理外の `config.toml` を読み、`$OUT_DIR/generated_config.rs` を生成する。
/// `src/main.rs` はこれを `build_config` moduleとして取り込む。secretを `src/` 配下の
/// Rustソースに置かないことで、コンパイラ警告に実値が出る事故を避ける。
/// 生成する定数には `#[allow(dead_code)]` を付け、未使用警告自体も出さない。
fn generate_config() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let config_path = Path::new(&manifest_dir).join("config.toml");
    println!("cargo:rerun-if-changed={}", config_path.display());

    if !config_path.exists() {
        panic!(
            "firmware/config.toml が見つかりません。config.example.toml を同じ \
             ディレクトリの config.toml へコピーし、実際の値を設定してください。詳細は \
             firmware/README.md を参照してください。"
        );
    }

    let raw = fs::read_to_string(&config_path).expect("config.toml の読み込みに失敗しました");
    // parse失敗時のエラーはDebug出力に config.toml の全文(`input`)を含むため、
    // エラーそのものをpanicメッセージへ載せない。行番号すら出さない代わりに、
    // secretがビルドログへ流れないことを優先する。
    let Ok(table) = raw.parse::<toml::Table>() else {
        panic!(
            "config.toml をTOMLとして解析できませんでした。secretを含む可能性があるため \
             内容とエラー詳細は表示しません。config.example.toml と見比べてください"
        );
    };

    let mut out = String::new();
    for spec in KEYS {
        let (ty, literal) = match spec.kind {
            Kind::Text => ("&str", text_literal(&table, spec)),
            Kind::Int(ty) => (ty, int_value(&table, spec).to_string()),
        };
        out.push_str("#[allow(dead_code)]\n");
        out.push_str(&format!(
            "pub const {}: {ty} = {literal};\n\n",
            spec.const_name
        ));
    }

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
    let dest = Path::new(&out_dir).join("generated_config.rs");
    fs::write(&dest, out).expect("failed to write generated_config.rs");
}

/// `key`、無ければ `alias` を引く。
fn lookup<'a>(table: &'a toml::Table, spec: &Key) -> Option<&'a toml::Value> {
    table
        .get(spec.key)
        .or_else(|| spec.alias.and_then(|alias| table.get(alias)))
}

fn require<'a>(table: &'a toml::Table, spec: &Key) -> &'a toml::Value {
    lookup(table, spec).unwrap_or_else(|| match spec.alias {
        Some(alias) => panic!(
            "config.toml に必須key `{}` がありません。旧key `{alias}` も未設定です。config.example.toml を確認してください",
            spec.key
        ),
        None => panic!(
            "config.toml に必須key `{}` がありません。config.example.toml を確認してください",
            spec.key
        ),
    })
}

/// Rustソースへ埋め込む文字列リテラル。panicメッセージへ値そのものは出さない。
/// secretが含まれ得るため。
fn text_literal(table: &toml::Table, spec: &Key) -> String {
    let value = require(table, spec)
        .as_str()
        .unwrap_or_else(|| panic!("config.toml: `{}` は文字列で指定してください", spec.key));
    format!("{value:?}")
}

fn int_value(table: &toml::Table, spec: &Key) -> i64 {
    match (lookup(table, spec), spec.default) {
        (None, Some(default)) => default,
        _ => require(table, spec)
            .as_integer()
            .unwrap_or_else(|| panic!("config.toml: `{}` は整数で指定してください", spec.key)),
    }
}
