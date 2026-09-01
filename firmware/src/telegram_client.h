#pragma once

// Outbound HTTPS long-polling client for the Telegram Bot API. Lets a
// smartphone drive /status, /wake, /reboot and /shutdown from outside the
// LAN without opening any inbound port. See docs/external-access.md for the
// full design.

namespace TelegramClient {

enum class Status { Disabled, Polling, Error };

// Starts the background polling task if TELEGRAM_BOT_TOKEN /
// TELEGRAM_ALLOWED_USER_ID are configured (not left as placeholders). Safe to
// call even without Wi-Fi yet; the task waits for a connection internally.
// Must be called once from setup(), after PowerController::begin().
void begin();

// Current state for the small on-screen indicator.
Status status();

} // namespace TelegramClient
