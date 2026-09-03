// Telegramから実行時に変更できる設定値(pc_ip_address / pc_status_addr / wol_port)。
//
// `AppConfig` は起動時に読んで以後変更しない値の集まりだが、この3値だけは
// Telegram経由で書き換わる。`AppConfig` 全体をMutex化すると、読み取りしかしない
// 他フィールド(bot token等)まで毎回ロックを取ることになるため、この3値だけを
// 独立したMutexで持つ(読み取り多数・書き込みほぼゼロというアクセス形態に合わせる)。
//
// 値の検証は `config-validation` crate(host側でテストできる)側で行う。ここは
// NVSへの永続化と「書き込みに成功した後だけメモリ上の値も更新する」順序を
// 守るだけに徹する。

use std::sync::{Mutex, MutexGuard};

use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use esp_idf_sys::EspError;

use crate::app_config::{AppConfig, NAMESPACE};

/// `app_config.rs::apply_nvs` が読む短縮キーと同じものを使う。ここで書いた値は
/// 次回起動時、`AppConfig::load` がビルド時configより優先して読み直す。
const NVS_KEY_PC_IP: &str = "pc_ip";
const NVS_KEY_STATUS_ADDR: &str = "status_addr";
const NVS_KEY_WOL_PORT: &str = "wol_port";

struct State {
    pc_ip_address: String,
    pc_status_addr: String,
    wol_port: u16,
    /// 書き込み用に読み書きモードで開いたハンドル。`AppConfig::load` が起動時に
    /// 読み取り専用で開くハンドルとは別物で、両者は同時に生存しない
    /// (`load()` はhandleを関数内で使い切って返す前に手放す)。
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
                pc_status_addr: app_config.pc_status_addr.clone(),
                wol_port: app_config.wol_port,
                nvs,
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // 排他が守るのはNVSへの書き込み1回とメモリ上の値の同期だけなので、
        // poisonしても排他を維持したまま使い続ける(telegram::lock_powerと同じ考え方)。
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn pc_ip_address(&self) -> String {
        self.lock().pc_ip_address.clone()
    }

    pub fn pc_status_addr(&self) -> String {
        self.lock().pc_status_addr.clone()
    }

    pub fn wol_port(&self) -> u16 {
        self.lock().wol_port
    }

    /// `/settings` 表示用の一括取得。3回ロックを取るより一貫した値が見える。
    pub fn snapshot(&self) -> (String, String, u16) {
        let state = self.lock();
        (
            state.pc_ip_address.clone(),
            state.pc_status_addr.clone(),
            state.wol_port,
        )
    }

    pub fn set_pc_ip_address(&self, value: String) -> Result<(), EspError> {
        let mut state = self.lock();
        state.nvs.set_str(NVS_KEY_PC_IP, &value)?;
        state.pc_ip_address = value;
        Ok(())
    }

    pub fn set_pc_status_addr(&self, value: String) -> Result<(), EspError> {
        let mut state = self.lock();
        state.nvs.set_str(NVS_KEY_STATUS_ADDR, &value)?;
        state.pc_status_addr = value;
        Ok(())
    }

    pub fn set_wol_port(&self, value: u16) -> Result<(), EspError> {
        let mut state = self.lock();
        state.nvs.set_str(NVS_KEY_WOL_PORT, &value.to_string())?;
        state.wol_port = value;
        Ok(())
    }
}
