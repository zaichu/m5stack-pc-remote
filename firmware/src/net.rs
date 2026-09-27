// Wi-Fi、Wake-on-LAN、STATUS相当の疎通確認。

use std::error::Error;
use std::io;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::time::Duration;

use esp_idf_hal::modem::Modem;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi};

/// これより古いUNIX時刻(2023-11-14より前)はNTP未同期とみなす。
/// 直接比較せず `is_ntp_synced` を使うこと。
const MIN_VALID_UNIX_TIME: u64 = 1_700_000_000;

/// NTP同期済みとみなせる時刻か。判定式を1関数へ寄せる(分散すると閾値変更の
/// 更新漏れや `<`/`>=`・符号の取り違えが起きる)。
pub fn is_ntp_synced(unix_seconds: i64) -> bool {
    unix_seconds >= MIN_VALID_UNIX_TIME as i64
}

/// PC死活確認のTCP接続タイムアウト。長くすると画面更新とTelegram応答が遅くなる。
pub const STATUS_PROBE_TIMEOUT: Duration = Duration::from_millis(800);

/// PCの死活状態の日本語表記。用語は `docs/glossary.md` が正本
/// (状態は「オン/オフ」と書き、「オンライン/オフライン」は使わない)。
pub fn pc_online_label_ja(online: bool) -> &'static str {
    if online {
        "オン"
    } else {
        "オフ"
    }
}

/// PCの状態変化の通知文。出来事(起動/停止)で書く(状態表示は `pc_online_label_ja`)。
pub fn pc_state_notification_ja(online: bool) -> &'static str {
    if online {
        "PCが起動しました。"
    } else {
        "PCが停止しました。"
    }
}

/// 画面表示用のASCII表記(`mono_font::ascii` は非ASCIIを `?` へ落とすため)。
pub fn pc_online_label_ascii(online: bool) -> &'static str {
    if online {
        "ONLINE"
    } else {
        "OFFLINE"
    }
}

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

/// STATUS相当の疎通確認。接続成功または即時refusedならオン、timeoutならオフ/到達不能。
pub fn check_pc_online(addr_text: &str, timeout: Duration) -> bool {
    // IPリテラルならDNSを引かない。ホスト名の名前解決はブロックし、UIループから
    // 呼ばれるここで画面もタッチも止まる(Issue #130-3)。
    // `to_socket_addrs()` は制限前にNVSへ書かれたホスト名が残っている場合の互換。
    if let Ok(addr) = addr_text.parse::<SocketAddr>() {
        return probe(addr, timeout);
    }

    let Ok(addrs) = addr_text.to_socket_addrs() else {
        return false;
    };
    // A/AAAA両方を持つホスト名では先頭1件だけだと取りこぼして誤判定するため順に試す。
    for addr in addrs {
        if probe(addr, timeout) {
            return true;
        }
    }
    false
}

/// `ConnectionRefused` は「相手は居るがそのportで待っていない」ためオンとみなす。
fn probe(addr: SocketAddr, timeout: Duration) -> bool {
    match TcpStream::connect_timeout(&addr, timeout) {
        Ok(_) => true,
        Err(e) => e.kind() == io::ErrorKind::ConnectionRefused,
    }
}

/// SNTPでシステム時刻を同期する。bridgeはtimestampを検証するため、署名付き
/// REBOOT/SHUTDOWNは時刻同期後だけ成功する。返したhandleは保持する。
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
            .map(|d| is_ntp_synced(d.as_secs() as i64))
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
