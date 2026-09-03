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
    Key::int("wol_port", "WOL_PORT", IntTy::U16),
    Key::text("pc_status_addr", "PC_STATUS_ADDR"),
    Key::int("bridge_port", "BRIDGE_PORT", IntTy::U16).alias("agent_port"),
    Key::text("bridge_shared_secret", "BRIDGE_SHARED_SECRET").alias("agent_shared_secret"),
    Key::text("pc_ip_address", "PC_IP_ADDRESS"),
    Key::text("telegram_bot_token", "TELEGRAM_BOT_TOKEN"),
    Key::text("telegram_allowed_user_id", "TELEGRAM_ALLOWED_USER_ID"),
    Key::int(
        "telegram_long_poll_timeout_seconds",
        "TELEGRAM_LONG_POLL_TIMEOUT_SECONDS",
        IntTy::U32,
    ),
    Key::int(
        "telegram_confirm_ttl_secs",
        "TELEGRAM_CONFIRM_TTL_SECS",
        IntTy::U64,
    ),
    // 定期レポート関連は後から追加した任意keyなので、既存のconfig.tomlでも
    // ビルドが通るよう既定値を持たせる(必須にするとkey追加まで壊れる)。
    Key::int("daily_report_hour", "DAILY_REPORT_HOUR", IntTy::I64).default(DAILY_REPORT_DISABLED),
    Key::int("timezone_offset_hours", "TIMEZONE_OFFSET_HOURS", IntTy::I64).default(0),
];

enum Kind {
    Text,
    Int {
        ty: IntTy,
        /// `Some` なら任意key。key が無いときはこの値を使う。
        /// default は整数keyだけの意味なので、Int の内側に持たせる。
        default: Option<i64>,
    },
}

/// 生成する定数のRust型。文字列の型名だと範囲チェックができず、
/// `wol_port = 70000` のような値が生成側の "literal out of range" コンパイルエラーに
/// 化けて config.toml のどのkeyが悪いか分からなくなる。ビルド時に型名と範囲の
/// 両方で検証し、キー名入りのエラーにするため enum で持つ。
#[derive(Clone, Copy)]
enum IntTy {
    U16,
    U32,
    U64,
    I64,
}

impl IntTy {
    const fn rust_name(self) -> &'static str {
        match self {
            IntTy::U16 => "u16",
            IntTy::U32 => "u32",
            IntTy::U64 => "u64",
            IntTy::I64 => "i64",
        }
    }

    const fn min_value(self) -> i64 {
        match self {
            IntTy::U16 | IntTy::U32 | IntTy::U64 => 0,
            IntTy::I64 => i64::MIN,
        }
    }

    /// TOMLの整数として表現できる範囲で判定する。`u64` の `i64::MAX` 超は
    /// TOML整数として入ってこない(i64超のリテラルはTOMLパース時点で失敗する)ため、
    /// `i64::MAX` を上限として問題ない。
    const fn max_value(self) -> i64 {
        match self {
            IntTy::U16 => u16::MAX as i64,
            IntTy::U32 => u32::MAX as i64,
            IntTy::U64 | IntTy::I64 => i64::MAX,
        }
    }
}

struct Key {
    key: &'static str,
    /// 旧key。`key` が無ければこちらを見る。
    alias: Option<&'static str>,
    const_name: &'static str,
    kind: Kind,
}

impl Key {
    const fn text(key: &'static str, const_name: &'static str) -> Self {
        Self {
            key,
            alias: None,
            const_name,
            kind: Kind::Text,
        }
    }

    const fn int(key: &'static str, const_name: &'static str, ty: IntTy) -> Self {
        Self {
            key,
            alias: None,
            const_name,
            kind: Kind::Int { ty, default: None },
        }
    }

    const fn alias(mut self, alias: &'static str) -> Self {
        self.alias = Some(alias);
        self
    }

    /// 任意keyの既定値。整数key専用。Text key に付けると const 評価時の
    /// panic(= build.rs のコンパイルエラー)になり、黙って無視されない。
    const fn default(self, default: i64) -> Self {
        let Self {
            key,
            alias,
            const_name,
            kind,
        } = self;
        let kind = match kind {
            Kind::Int { ty, .. } => Kind::Int {
                ty,
                default: Some(default),
            },
            Kind::Text => panic!("Key::default() は整数keyにのみ使えます"),
        };
        Self {
            key,
            alias,
            const_name,
            kind,
        }
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
            Kind::Int { ty, default } => (
                ty.rust_name(),
                int_value(&table, spec, ty, default).to_string(),
            ),
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

/// 整数keyの値をTOMLから引き、生成先の型範囲を検証した i64 を返す。
/// `default` が `Some` で key も alias も無いときは既定値を使う。
fn int_value(table: &toml::Table, spec: &Key, ty: IntTy, default: Option<i64>) -> i64 {
    let value = match (lookup(table, spec), default) {
        (None, Some(default)) => default,
        _ => require(table, spec)
            .as_integer()
            .unwrap_or_else(|| panic!("config.toml: `{}` は整数で指定してください", spec.key)),
    };
    // 範囲外をここで止めないと、生成された `pub const ...: u16 = 70000;` が
    // "literal out of range" として落ち、config.toml のどのkeyの値か分からなくなる。
    if !(ty.min_value()..=ty.max_value()).contains(&value) {
        panic!(
            "config.toml: `{}` は {} の範囲({}〜{})で指定してください。指定値: {value}",
            spec.key,
            ty.rust_name(),
            ty.min_value(),
            ty.max_value()
        );
    }
    value
}
