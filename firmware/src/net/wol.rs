use std::error::Error;
use std::net::UdpSocket;

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
