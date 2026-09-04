// 実行時設定。NVSの `m5remote` namespace に値があれば優先し、無ければ
// build.rs が生成したconfigを使う。
//
// 秘密値を含むため Debug は実装しない。ログへ値を出さないこと。

use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};

use crate::build_config;

/// `settings` moduleも同じNVS namespaceへ書き込むため、ここで公開する。
pub(crate) const NAMESPACE: &str = "m5remote";
const MAX_STRING_LEN: usize = 512;

#[derive(Clone)]
pub struct AppConfig {
    pub wifi_ssid: String,
    pub wifi_password: String,
    pub pc_mac_address: String,
    pub wol_port: u16,
    pub pc_status_addr: String,
    pub bridge_port: u16,
    pub bridge_shared_secret: String,
    pub pc_ip_address: String,
    pub telegram_bot_token: String,
    pub telegram_allowed_user_id: String,
    pub telegram_long_poll_timeout_seconds: u32,
    pub telegram_confirm_ttl_secs: u64,
    /// 定期レポートを送るローカル時刻(0-23)。範囲外なら送らない。
    pub daily_report_hour: i64,
    /// UTCからのローカル時刻のずれ(時間)。JSTなら9。
    pub timezone_offset_hours: i64,
}

impl AppConfig {
    pub fn load(partition: EspDefaultNvsPartition) -> Self {
        let mut app_config = Self::from_build_config();

        match EspNvs::new(partition, NAMESPACE, false) {
            Ok(nvs) => {
                app_config.apply_nvs(&nvs);
                println!("NVS設定を読み込みました");
            }
            Err(e) => {
                println!("NVS設定なし。ビルド時configを使用します: {e}");
            }
        }

        app_config.clamp_ranges();
        app_config
    }

    fn from_build_config() -> Self {
        Self {
            wifi_ssid: build_config::WIFI_SSID.to_string(),
            wifi_password: build_config::WIFI_PASSWORD.to_string(),
            pc_mac_address: build_config::PC_MAC_ADDRESS.to_string(),
            wol_port: build_config::WOL_PORT,
            pc_status_addr: build_config::PC_STATUS_ADDR.to_string(),
            bridge_port: build_config::BRIDGE_PORT,
            bridge_shared_secret: build_config::BRIDGE_SHARED_SECRET.to_string(),
            pc_ip_address: build_config::PC_IP_ADDRESS.to_string(),
            telegram_bot_token: build_config::TELEGRAM_BOT_TOKEN.to_string(),
            telegram_allowed_user_id: build_config::TELEGRAM_ALLOWED_USER_ID.to_string(),
            telegram_long_poll_timeout_seconds: build_config::TELEGRAM_LONG_POLL_TIMEOUT_SECONDS,
            telegram_confirm_ttl_secs: build_config::TELEGRAM_CONFIRM_TTL_SECS,
            daily_report_hour: build_config::DAILY_REPORT_HOUR,
            timezone_offset_hours: build_config::TIMEZONE_OFFSET_HOURS,
        }
    }

    /// NVSに値があれば上書きする。keyを複数渡した場合は先に見つかったものを使う
    /// (2つ目以降は旧key)。NVSのkeyは15文字までなので短縮名になっている。
    /// 範囲を持つ値が明らかに不正なら既定値へ戻す。
    ///
    /// NVSはビルド時configを迂回して値を差し込めるため、build.rs側の検証だけでは
    /// 素通りする。`timezone_offset_hours` に極端な値が入ると
    /// `telegram.rs` の `unix + offset * 3600` で日付が大きくずれ、定期レポートが
    /// 意図しない時刻に出る。
    fn clamp_ranges(&mut self) {
        // 実在するUTCオフセットの範囲(UTC-12〜UTC+14)。
        if !(-12..=14).contains(&self.timezone_offset_hours) {
            println!(
                "timezone_offset_hours={} は範囲外(-12..=14)です。0として扱います",
                self.timezone_offset_hours
            );
            self.timezone_offset_hours = 0;
        }
        // 0-23が有効時刻。範囲外は「無効(送らない)」を意味する -1 に寄せる。
        if !(0..=23).contains(&self.daily_report_hour) && self.daily_report_hour != -1 {
            println!(
                "daily_report_hour={} は範囲外です。無効(-1)として扱います",
                self.daily_report_hour
            );
            self.daily_report_hour = -1;
        }
    }

    fn apply_nvs(&mut self, nvs: &EspNvs<NvsDefault>) {
        replace(nvs, &["wifi_ssid"], &mut self.wifi_ssid);
        replace(nvs, &["wifi_pass"], &mut self.wifi_password);
        replace(nvs, &["pc_mac"], &mut self.pc_mac_address);
        replace(nvs, &["wol_port"], &mut self.wol_port);
        replace(nvs, &["status_addr"], &mut self.pc_status_addr);
        replace(nvs, &["bridge_port", "agent_port"], &mut self.bridge_port);
        replace(
            nvs,
            &["bridge_secret", "agent_secret"],
            &mut self.bridge_shared_secret,
        );
        replace(nvs, &["pc_ip"], &mut self.pc_ip_address);
        replace(nvs, &["tg_token"], &mut self.telegram_bot_token);
        replace(nvs, &["tg_user_id"], &mut self.telegram_allowed_user_id);
        replace(
            nvs,
            &["tg_poll_secs"],
            &mut self.telegram_long_poll_timeout_seconds,
        );
        replace(nvs, &["tg_ttl_secs"], &mut self.telegram_confirm_ttl_secs);
        replace(nvs, &["report_hour"], &mut self.daily_report_hour);
        replace(nvs, &["tz_offset"], &mut self.timezone_offset_hours);
    }
}

/// NVSから読めて、かつ目的の型へparseできたときだけ上書きする。
/// `String` も `FromStr` を実装しているため、文字列と数値を同じ関数で扱える。
/// 壊れた値が入っていてもビルド時configへフォールバックする。
///
/// ログにはkey名だけを出し、値は出さない。ここを通る値にはWi-Fiパスワードや
/// bot tokenが含まれる。
fn replace<T>(nvs: &EspNvs<NvsDefault>, keys: &[&str], target: &mut T)
where
    T: std::str::FromStr,
{
    let Some((key, raw)) = keys
        .iter()
        .find_map(|key| read_string(nvs, key).map(|raw| (*key, raw)))
    else {
        return;
    };
    match raw.parse() {
        Ok(value) => *target = value,
        // 無言でfallbackすると、NVSへ壊れた値(" 80" や "abc")が入っていても
        // 気づけない。設定したはずの値が効かない理由が分かるようにする。
        Err(_) => println!(
            "NVS `{key}` の値を解釈できませんでした。ビルド時configを使います"
        ),
    }
}

fn read_string(nvs: &EspNvs<NvsDefault>, key: &str) -> Option<String> {
    let len = nvs.str_len(key).ok().flatten()?;
    if len == 0 {
        println!("NVS `{key}` が空です。ビルド時configを使います");
        return None;
    }
    if len > MAX_STRING_LEN {
        println!(
            "NVS `{key}` が長すぎます({len} > {MAX_STRING_LEN})。ビルド時configを使います"
        );
        return None;
    }

    let mut buffer = vec![0_u8; len];
    nvs.get_str(key, &mut buffer)
        .ok()
        .flatten()
        .map(str::to_owned)
}
