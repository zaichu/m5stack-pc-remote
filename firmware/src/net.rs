// Wi-Fi、Wake-on-LAN、STATUS相当の疎通確認。

use std::error::Error;
use std::io;
use std::net::{TcpStream, ToSocketAddrs, UdpSocket};
use std::time::Duration;

use esp_idf_hal::modem::Modem;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi};

/// これより古いUNIX時刻はNTP未同期とみなす(2023-11-14相当)。
/// 署名のtimestampと定期レポートの時刻判定で共通に使う。
pub const MIN_VALID_UNIX_TIME: u64 = 1_700_000_000;

/// Wi-Fi stationハンドル。接続維持のためプログラム中で保持し続ける。
pub struct Wifi {
    inner: BlockingWifi<EspWifi<'static>>,
}

impl Wifi {
    /// station interfaceを設定して起動し、ネットワークが上がるまで待つ。
    /// 初回接続専用。失敗時はModemが破棄されるため、再試行は `connect_retry()` を使う。
    pub fn connect(
        modem: Modem<'static>,
        nvs: EspDefaultNvsPartition,
        ssid: &str,
        password: &str,
    ) -> Result<Self, Box<dyn Error>> {
        Self::connect_with_modem(modem, nvs, ssid, password)
    }

    /// 前回の接続失敗でModemが破棄されたあと、最初から接続をやり直す。
    ///
    /// # Safety
    /// 生きている `Wifi` / `Modem` が他にない状態でだけ呼ぶ。初回 `connect()` 失敗後や、
    /// 前回の `connect_retry()` 失敗後が該当する。
    pub fn connect_retry(
        nvs: EspDefaultNvsPartition,
        ssid: &str,
        password: &str,
    ) -> Result<Self, Box<dyn Error>> {
        let modem = unsafe { Modem::steal() };
        Self::connect_with_modem(modem, nvs, ssid, password)
    }

    fn connect_with_modem(
        modem: Modem<'static>,
        nvs: EspDefaultNvsPartition,
        ssid: &str,
        password: &str,
    ) -> Result<Self, Box<dyn Error>> {
        let sys_loop = EspSystemEventLoop::take()?;

        let mut inner =
            BlockingWifi::wrap(EspWifi::new(modem, sys_loop.clone(), Some(nvs))?, sys_loop)?;

        inner.set_configuration(&Configuration::Client(ClientConfiguration {
            ssid: ssid.try_into().map_err(|_| "WIFI_SSID too long")?,
            password: password.try_into().map_err(|_| "WIFI_PASSWORD too long")?,
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

fn parse_mac(text: &str) -> Option<[u8; 6]> {
    let mut mac = [0u8; 6];
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() != 6 {
        return None;
    }
    for (i, part) in parts.iter().enumerate() {
        mac[i] = u8::from_str_radix(part, 16).ok()?;
    }
    Some(mac)
}

/// Wake-on-LAN magic packetをlimited broadcast(255.255.255.255)へ送る。
/// LANの実subnet prefixに依存しない。
pub fn send_wake_on_lan(mac_text: &str, port: u16) -> Result<(), Box<dyn Error>> {
    let mac = parse_mac(mac_text).ok_or("invalid PC_MAC_ADDRESS")?;

    let mut packet = vec![0xFFu8; 6];
    for _ in 0..16 {
        packet.extend_from_slice(&mac);
    }

    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_broadcast(true)?;
    let sent = socket.send_to(&packet, ("255.255.255.255", port))?;
    if sent != packet.len() {
        return Err(format!("short WOL write: {sent}/{}", packet.len()).into());
    }
    Ok(())
}

/// STATUS相当の疎通確認。接続成功または即時refusedならPCは応答あり、
/// timeoutなら電源OFFまたは到達不能として扱う。
pub fn check_pc_online(addr_text: &str, timeout: Duration) -> bool {
    let Ok(mut addrs) = addr_text.to_socket_addrs() else {
        return false;
    };
    let Some(addr) = addrs.next() else {
        return false;
    };
    match TcpStream::connect_timeout(&addr, timeout) {
        Ok(_) => true,
        Err(e) => e.kind() == io::ErrorKind::ConnectionRefused,
    }
}

/// SNTPを開始してシステム時刻を同期する。m5stack-pc-bridgeはtimestampを検証するため、
/// 署名付きREBOOT/SHUTDOWNは時刻同期後だけ成功する。返したhandleは保持する。
pub fn start_sntp() -> Result<esp_idf_svc::sntp::EspSntp<'static>, Box<dyn Error>> {
    Ok(esp_idf_svc::sntp::EspSntp::new_default()?)
}

/// NTP同期済みに見えるまで待つ。timeoutした場合も呼び出し側は処理を続ける。
pub fn wait_for_time_sync(timeout: Duration) -> bool {
    use std::time::{SystemTime, UNIX_EPOCH};

    let start = std::time::Instant::now();
    loop {
        let synced = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() >= MIN_VALID_UNIX_TIME)
            .unwrap_or(false);
        if synced {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}
