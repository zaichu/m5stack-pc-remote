// Wi-Fi, Wake-on-LAN and the STATUS reachability check.

use std::error::Error;
use std::io;
use std::net::{TcpStream, ToSocketAddrs, UdpSocket};
use std::time::Duration;

use esp_idf_hal::modem::Modem;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi};

/// Wi-Fi station handle. Must be kept alive for the connection to persist:
/// dropping it tears the connection down.
pub struct Wifi {
    inner: BlockingWifi<EspWifi<'static>>,
}

impl Wifi {
    /// Configures and starts the station interface, then blocks until the
    /// interface is up.
    pub fn connect(modem: Modem, ssid: &str, password: &str) -> Result<Self, Box<dyn Error>> {
        let sys_loop = EspSystemEventLoop::take()?;
        let nvs = EspDefaultNvsPartition::take()?;

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

    /// Re-associates after a drop-out. Mirrors the reconnect behaviour of the
    /// C++ firmware's `connectWifi()`; callers rate-limit the retries.
    pub fn reconnect(&mut self) -> Result<(), Box<dyn Error>> {
        // `connect()` fails if the driver still considers itself connected.
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

/// Sends a Wake-on-LAN magic packet to the limited broadcast address
/// (255.255.255.255), matching firmware/src/power_controller.cpp: this keeps
/// working regardless of the LAN's actual subnet prefix length.
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

/// STATUS-equivalent check. A fast connect or a connection refusal both mean
/// the PC answered; a timeout means it is off or unreachable.
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
