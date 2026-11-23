use std::error::Error;

use esp_idf_hal::modem::Modem;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi};

/// Wi-Fi stationハンドル。接続維持のためプログラム中で保持し続ける。
pub struct Wifi {
    inner: BlockingWifi<EspWifi<'static>>,
}

impl Wifi {
    /// 初回接続専用。失敗時はModemが破棄されるため、再試行は `connect_retry()` を使う。
    pub fn connect(
        modem: Modem,
        nvs: EspDefaultNvsPartition,
        ssid: &str,
        password: &str,
    ) -> Result<Self, Box<dyn Error>> {
        Self::connect_with_modem(modem, nvs, ssid, password)
    }

    /// 前回の接続失敗後に最初から接続をやり直す。
    ///
    /// # Safety
    /// 生きている `Wifi` / `Modem` が他にない状態でだけ呼ぶこと
    /// (初回 `connect()` 失敗後や前回の `connect_retry()` 失敗後)。
    pub fn connect_retry(
        nvs: EspDefaultNvsPartition,
        ssid: &str,
        password: &str,
    ) -> Result<Self, Box<dyn Error>> {
        let modem = unsafe { Modem::new() };
        Self::connect_with_modem(modem, nvs, ssid, password)
    }

    fn connect_with_modem(
        modem: Modem,
        nvs: EspDefaultNvsPartition,
        ssid: &str,
        password: &str,
    ) -> Result<Self, Box<dyn Error>> {
        let sys_loop = EspSystemEventLoop::take()?;

        let mut inner =
            BlockingWifi::wrap(EspWifi::new(modem, sys_loop.clone(), Some(nvs))?, sys_loop)?;

        inner.set_configuration(&Configuration::Client(ClientConfiguration {
            ssid: ssid.try_into().map_err(|()| "WIFI_SSID too long")?,
            password: password.try_into().map_err(|()| "WIFI_PASSWORD too long")?,
            auth_method: AuthMethod::WPA2Personal,
            ..Default::default()
        }))?;

        inner.start()?;

        let mut wifi = Self { inner };
        wifi.associate()?;
        Ok(wifi)
    }

    fn associate(&mut self) -> Result<(), Box<dyn Error>> {
        self.inner.connect()?;
        self.inner.wait_netif_up()?;
        Ok(())
    }

    pub fn is_up(&self) -> bool {
        self.inner.is_up().unwrap_or(false)
    }

    /// 切断後に再接続する。呼び出し側で再試行間隔を制御する。
    pub fn reconnect(&mut self) -> Result<(), Box<dyn Error>> {
        // driverが接続中と認識している場合に備えて、先に切断してから接続する。
        let _ = self.inner.disconnect();
        self.associate()
    }
}
