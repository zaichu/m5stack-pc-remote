#pragma once

// Copy this file to config.h and replace all sample values.
// Do not commit config.h.

#define WIFI_SSID "your-wifi-ssid"
#define WIFI_PASSWORD "your-wifi-password"

#define PC_HOSTNAME "desktop"
#define PC_IP_ADDRESS "192.168.1.100"
#define PC_MAC_ADDRESS "AA:BB:CC:DD:EE:FF"
#define WOL_PORT 9

#define STATUS_INTERVAL_MS 10000
#define WIFI_RECONNECT_INTERVAL_MS 15000

#define AGENT_PORT 18080
#define AGENT_SHARED_SECRET "replace-with-the-same-secret-as-windows-agent"
#define AGENT_CLOCK_SKEW_NOTE "Use NTP before reboot/shutdown commands"

// Telegram Bot API (outbound HTTPS long polling). Kept separate from
// AGENT_SHARED_SECRET: this token only talks to Telegram, never to the
// Windows Agent. Leave TELEGRAM_BOT_TOKEN / TELEGRAM_ALLOWED_USER_ID as the
// placeholders below to keep the Telegram client disabled.
#define TELEGRAM_BOT_TOKEN "replace-with-your-telegram-bot-token"
#define TELEGRAM_ALLOWED_USER_ID "replace-with-your-telegram-user-id"
#define TELEGRAM_LONG_POLL_TIMEOUT_SECONDS 20
#define TELEGRAM_CONFIRM_TTL_MS 60000
