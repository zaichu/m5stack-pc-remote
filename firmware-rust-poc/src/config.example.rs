// Copy this file to config.rs and replace all sample values.
// Do not commit config.rs.

pub const WIFI_SSID: &str = "your-wifi-ssid";
pub const WIFI_PASSWORD: &str = "your-wifi-password";

pub const PC_MAC_ADDRESS: &str = "AA:BB:CC:DD:EE:FF";
pub const WOL_PORT: u16 = 9;

// STATUS check target. The PoC does not have raw ICMP wired up yet (see
// Issue #16), so this is a TCP connect probe instead: a fast connect or a
// refusal both mean "the PC is on and reachable"; a timeout means "off or
// unreachable". Nothing needs to be listening on the port.
pub const PC_STATUS_ADDR: &str = "192.168.1.100:80";
