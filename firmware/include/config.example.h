#pragma once

// Copy this file to config.h and replace all sample values.
// Do not commit config.h.

#define WIFI_SSID "your-wifi-ssid"
#define WIFI_PASSWORD "your-wifi-password"

#define PC_HOSTNAME "desktop"
#define PC_IP_ADDRESS "192.168.1.100"
#define PC_MAC_ADDRESS "AA:BB:CC:DD:EE:FF"
#define WOL_BROADCAST_ADDRESS "192.168.1.255"
#define WOL_PORT 9

#define STATUS_INTERVAL_MS 10000
#define WIFI_RECONNECT_INTERVAL_MS 15000

#define AGENT_HOST "192.168.1.100"
#define AGENT_PORT 18080
#define AGENT_SHARED_SECRET "replace-with-the-same-secret-as-windows-agent"
#define AGENT_CLOCK_SKEW_NOTE "Use NTP before reboot/shutdown commands"
