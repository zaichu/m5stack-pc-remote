// 実行時設定。NVSの `m5remote` namespace に値があれば優先し、無ければ
// build.rs が生成したconfigを使う。
//
// 秘密値を含むため Debug は実装しない。ログへ値を出さないこと。

use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};

use crate::build_config;

const NAMESPACE: &str = "m5remote";
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

    fn apply_nvs(&mut self, nvs: &EspNvs<NvsDefault>) {
        replace_string(nvs, "wifi_ssid", &mut self.wifi_ssid);
        replace_string(nvs, "wifi_pass", &mut self.wifi_password);
        replace_string(nvs, "pc_mac", &mut self.pc_mac_address);
        replace_number(nvs, "wol_port", &mut self.wol_port);
        replace_string(nvs, "status_addr", &mut self.pc_status_addr);
        replace_number_with_fallback(nvs, "bridge_port", "agent_port", &mut self.bridge_port);
        replace_string_with_fallback(
            nvs,
            "bridge_secret",
            "agent_secret",
            &mut self.bridge_shared_secret,
        );
        replace_string(nvs, "pc_ip", &mut self.pc_ip_address);
        replace_string(nvs, "tg_token", &mut self.telegram_bot_token);
        replace_string(nvs, "tg_user_id", &mut self.telegram_allowed_user_id);
        replace_number(
            nvs,
            "tg_poll_secs",
            &mut self.telegram_long_poll_timeout_seconds,
        );
        replace_number(nvs, "tg_ttl_secs", &mut self.telegram_confirm_ttl_secs);
        replace_number(nvs, "report_hour", &mut self.daily_report_hour);
        replace_number(nvs, "tz_offset", &mut self.timezone_offset_hours);
    }
}

fn replace_string(nvs: &EspNvs<NvsDefault>, key: &str, target: &mut String) {
    if let Some(value) = read_string(nvs, key) {
        *target = value;
    }
}

fn replace_string_with_fallback(
    nvs: &EspNvs<NvsDefault>,
    key: &str,
    fallback_key: &str,
    target: &mut String,
) {
    if let Some(value) = read_string(nvs, key).or_else(|| read_string(nvs, fallback_key)) {
        *target = value;
    }
}

fn replace_number<T>(nvs: &EspNvs<NvsDefault>, key: &str, target: &mut T)
where
    T: std::str::FromStr,
{
    if let Some(value) = read_string(nvs, key).and_then(|raw| raw.parse().ok()) {
        *target = value;
    }
}

fn replace_number_with_fallback<T>(
    nvs: &EspNvs<NvsDefault>,
    key: &str,
    fallback_key: &str,
    target: &mut T,
) where
    T: std::str::FromStr,
{
    if let Some(value) = read_string(nvs, key)
        .or_else(|| read_string(nvs, fallback_key))
        .and_then(|raw| raw.parse().ok())
    {
        *target = value;
    }
}

fn read_string(nvs: &EspNvs<NvsDefault>, key: &str) -> Option<String> {
    let len = nvs.str_len(key).ok().flatten()?;
    if len == 0 || len > MAX_STRING_LEN {
        return None;
    }

    let mut buffer = vec![0_u8; len];
    nvs.get_str(key, &mut buffer)
        .ok()
        .flatten()
        .map(str::to_owned)
}
