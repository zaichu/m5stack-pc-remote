// Telegramから実行時に変更できる設定値(pc_ip_address / wol_port / brightness)。
// STATUS確認先のhostは `pc_ip_address` から読み出し時に導くため別に持たず、
// port(`pc_status_port`)だけを読み取り専用で保持する(Issue #176)。
//
// `AppConfig` 全体をMutex化するとbot token等の読み取り専用フィールドまで毎回
// ロックを取ることになるため、この3値(+port)だけを独立したMutexで持つ。
// 値の検証は `config-validation` crate側で行い、ここはNVSへの永続化と
// 「書き込み成功後だけメモリ上の値を更新する」順序を守るだけに徹する。

use std::sync::{Mutex, MutexGuard};

use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use esp_idf_sys::EspError;

use crate::app_config::{AppConfig, NAMESPACE};

// `app_config.rs::apply_nvs` が読む短縮キーと同じものを使う。ここで書いた値は
// 次回起動時 `AppConfig::load` がビルド時configより優先して読み直す。
const NVS_KEY_PC_IP: &str = "pc_ip";
const NVS_KEY_WOL_PORT: &str = "wol_port";
// 既定値100(現状のDCDC3 2800mV相当)は `firmware/build.rs` が正本。NVSにも
// ビルド時configにも無いときはそこへフォールバックする(既存ユーザーの後方互換)。
const NVS_KEY_BRIGHTNESS: &str = "brightness";

struct State {
    pc_ip_address: String,
    /// STATUS確認先のport。読み取り専用で、Telegramからは変更しない。
    pc_status_port: u16,
    wol_port: u16,
    brightness_percent: u8,
    /// 読み書きモードで開いた書き込み用ハンドル。起動時に `AppConfig::load` が
    /// 開く読み取り専用ハンドルとは別物で、両者は同時に生存しない。
    nvs: EspNvs<NvsDefault>,
}

pub struct RuntimeSettings {
    state: Mutex<State>,
}

impl RuntimeSettings {
    pub fn new(app_config: &AppConfig, partition: EspDefaultNvsPartition) -> Result<Self, EspError> {
        let nvs = EspNvs::new(partition, NAMESPACE, true)?;
        Ok(Self {
            state: Mutex::new(State {
                pc_ip_address: app_config.pc_ip_address.clone(),
                pc_status_port: app_config.pc_status_port,
                wol_port: app_config.wol_port,
                brightness_percent: app_config.brightness,
                nvs,
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // poisonしても排他を維持したまま使い続ける(守るのはNVS書き込みと値の
        // 同期だけ。telegram::lock_powerと同じ考え方)。
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn pc_ip_address(&self) -> String {
        self.lock().pc_ip_address.clone()
    }

    /// STATUS確認先を読み出しのたびに組み立てる(キャッシュしないため、IP変更は
    /// 次の呼び出しから効く。Issue #176)。
    pub fn pc_status_addr(&self) -> String {
        let state = self.lock();
        config_validation::compose_status_addr(&state.pc_ip_address, state.pc_status_port)
    }

    pub fn wol_port(&self) -> u16 {
        self.lock().wol_port
    }

    pub fn brightness_percent(&self) -> u8 {
        self.lock().brightness_percent
    }

    /// `/settings` 表示用の一括取得(3回ロックを取るより一貫した値が見える)。
    pub fn snapshot(&self) -> (String, u16, u8) {
        let state = self.lock();
        (
            state.pc_ip_address.clone(),
            state.wol_port,
            state.brightness_percent,
        )
    }

    pub fn set_pc_ip_address(&self, value: String) -> Result<(), EspError> {
        let mut state = self.lock();
        state.nvs.set_str(NVS_KEY_PC_IP, &value)?;
        state.pc_ip_address = value;
        Ok(())
    }

    pub fn set_wol_port(&self, value: u16) -> Result<(), EspError> {
        let mut state = self.lock();
        state.nvs.set_str(NVS_KEY_WOL_PORT, &value.to_string())?;
        state.wol_port = value;
        Ok(())
    }

    pub fn set_brightness_percent(&self, value: u8) -> Result<(), EspError> {
        let mut state = self.lock();
        state.nvs.set_str(NVS_KEY_BRIGHTNESS, &value.to_string())?;
        state.brightness_percent = value;
        Ok(())
    }
}
