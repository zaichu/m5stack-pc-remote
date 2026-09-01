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

// Windows Agent (LAN, plain HTTP + HMAC-SHA256 signed requests).
pub const AGENT_PORT: u16 = 18080;
pub const AGENT_SHARED_SECRET: &str = "replace-with-the-same-secret-as-windows-agent";

// The Windows Agent lives on the same PC as the STATUS target.
pub const PC_IP_ADDRESS: &str = "192.168.1.100";

// Telegram Bot API (outbound HTTPS long polling). Leave these as the
// placeholders below to keep the Telegram client disabled.
pub const TELEGRAM_BOT_TOKEN: &str = "replace-with-your-telegram-bot-token";
pub const TELEGRAM_ALLOWED_USER_ID: &str = "replace-with-your-telegram-user-id";
pub const TELEGRAM_LONG_POLL_TIMEOUT_SECONDS: u32 = 20;
pub const TELEGRAM_CONFIRM_TTL_SECS: u64 = 60;
