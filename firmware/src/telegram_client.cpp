#include "telegram_client.h"

#include <Arduino.h>
#include <ArduinoJson.h>
#include <HTTPClient.h>
#include <WiFi.h>
#include <WiFiClientSecure.h>
#include <esp_system.h>
#include <time.h>

#include <atomic>

#include <freertos/FreeRTOS.h>
#include <freertos/task.h>

#include "power_controller.h"
#include "telegram_root_ca.h"

#if __has_include("config.h")
#include "config.h"
#endif
#ifndef WIFI_SSID
// The ESP32 toolchain ships its own unrelated sys-include/config.h, which
// makes __has_include("config.h") true even when this project's config.h is
// absent. Fall back to the example config when our macros never got defined.
#include "config.example.h"
#endif

namespace {

constexpr unsigned long kBackoffMinMs = 5000;
constexpr unsigned long kBackoffMaxMs = 60000;
constexpr time_t kMinValidUnixTime = 1700000000;

std::atomic<TelegramClient::Status> currentStatus{
    TelegramClient::Status::Disabled};
TaskHandle_t taskHandle = nullptr;

int64_t lastUpdateId = 0;
bool initialSyncDone = false;

enum class PendingConfirmAction { None, Reboot, Shutdown };

struct PendingConfirm {
  PendingConfirmAction action = PendingConfirmAction::None;
  String nonceValue;
  unsigned long expiresAtMs = 0;
};
PendingConfirm pendingConfirm;

bool isTelegramConfigured() {
  static const char *placeholderToken = "replace-with-your-telegram-bot-token";
  static const char *placeholderUserId = "replace-with-your-telegram-user-id";
  return TELEGRAM_BOT_TOKEN[0] != '\0' &&
         strcmp(TELEGRAM_BOT_TOKEN, placeholderToken) != 0 &&
         TELEGRAM_ALLOWED_USER_ID[0] != '\0' &&
         strcmp(TELEGRAM_ALLOWED_USER_ID, placeholderUserId) != 0;
}

// Never log this: it embeds TELEGRAM_BOT_TOKEN.
String telegramApiUrl(const char *method) {
  String url = "https://api.telegram.org/bot";
  url += TELEGRAM_BOT_TOKEN;
  url += "/";
  url += method;
  return url;
}

String generateConfirmNonce() {
  static const char charset[] = "23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
  String out;
  out.reserve(6);
  for (int i = 0; i < 6; ++i) {
    out += charset[esp_random() % (sizeof(charset) - 1)];
  }
  return out;
}

void sendReply(int64_t chatId, const String &text) {
  WiFiClientSecure client;
  client.setCACert(TELEGRAM_ROOT_CA_PEM);
  HTTPClient https;
  if (!https.begin(client, telegramApiUrl("sendMessage"))) {
    return;
  }
  https.addHeader("Content-Type", "application/json");

  JsonDocument doc;
  doc["chat_id"] = chatId;
  doc["text"] = text;
  String body;
  serializeJson(doc, body);

  int status = https.POST(body);
  https.end();
  if (status < 200 || status >= 300) {
    Serial.printf("telegram sendMessage failed: %d\n", status);
  }
}

String buildStatusReply() {
  String text = "PC: ";
  text += PowerController::isPcOnline() ? "ONLINE" : "OFFLINE";
  text += "\nWi-Fi RSSI: ";
  text += String(WiFi.RSSI());
  text += " dBm\nM5Stack IP: ";
  text += WiFi.localIP().toString();

  time_t now = time(nullptr);
  if (now >= kMinValidUnixTime) {
    struct tm timeInfo;
    localtime_r(&now, &timeInfo);
    char buf[32];
    strftime(buf, sizeof(buf), "%Y-%m-%d %H:%M:%S", &timeInfo);
    text += "\nLast check: ";
    text += buf;
  }
  return text;
}

void requestConfirmation(int64_t chatId, PendingConfirmAction action,
                         const char *actionLabel,
                         const char *confirmCommand) {
  pendingConfirm.action = action;
  pendingConfirm.nonceValue = generateConfirmNonce();
  pendingConfirm.expiresAtMs = millis() + TELEGRAM_CONFIRM_TTL_MS;

  String reply = "Confirm ";
  reply += actionLabel;
  reply += ": ";
  reply += confirmCommand;
  reply += " ";
  reply += pendingConfirm.nonceValue;
  sendReply(chatId, reply);
}

void handleConfirmation(int64_t chatId, PendingConfirmAction action,
                       const char *actionLabel, const String &suppliedNonce,
                       const char *agentPath) {
  bool valid = pendingConfirm.action == action &&
               pendingConfirm.nonceValue.length() > 0 &&
               suppliedNonce.length() > 0 &&
               pendingConfirm.nonceValue == suppliedNonce &&
               millis() < pendingConfirm.expiresAtMs;

  // Consume the nonce regardless of the outcome so it cannot be replayed or
  // brute-forced across multiple confirm attempts.
  pendingConfirm = PendingConfirm{};

  if (!valid) {
    String reply = "No matching pending ";
    reply += actionLabel;
    reply += " confirmation (expired, already used, or wrong nonce). Send /";
    reply += actionLabel;
    reply += " again.";
    sendReply(chatId, reply);
    return;
  }

  bool ok = PowerController::postAgentCommand(agentPath);
  String reply = actionLabel;
  reply += ok ? " accepted" : " failed";
  sendReply(chatId, reply);
}

void dispatchCommand(int64_t chatId, const String &command,
                     const String &args) {
  if (command == "/status") {
    sendReply(chatId, buildStatusReply());
  } else if (command == "/wake") {
    bool ok = PowerController::sendWakeOnLan();
    sendReply(chatId, ok ? "WOL sent" : "WOL failed");
    PowerController::updateStatus();
  } else if (command == "/reboot") {
    requestConfirmation(chatId, PendingConfirmAction::Reboot, "reboot",
                        "/confirm_reboot");
  } else if (command == "/shutdown") {
    requestConfirmation(chatId, PendingConfirmAction::Shutdown, "shutdown",
                        "/confirm_shutdown");
  } else if (command == "/confirm_reboot") {
    handleConfirmation(chatId, PendingConfirmAction::Reboot, "reboot", args,
                      "/reboot");
  } else if (command == "/confirm_shutdown") {
    handleConfirmation(chatId, PendingConfirmAction::Shutdown, "shutdown",
                      args, "/shutdown");
  }
  // Unrecognized commands are ignored silently.
}

void processUpdates(JsonArray results, bool dispatch) {
  for (JsonObject item : results) {
    int64_t updateId = item["update_id"] | (int64_t)0;
    if (updateId >= lastUpdateId) {
      lastUpdateId = updateId + 1;
    }

    if (!dispatch) {
      // First batch after boot: only advance the offset so commands issued
      // while the device was offline are skipped instead of replayed.
      continue;
    }

    JsonObject message = item["message"];
    if (message.isNull()) {
      continue;
    }

    int64_t fromId = message["from"]["id"] | (int64_t)0;
    char fromIdStr[24];
    snprintf(fromIdStr, sizeof(fromIdStr), "%lld", (long long)fromId);
    if (strcmp(fromIdStr, TELEGRAM_ALLOWED_USER_ID) != 0) {
      // Unauthorized user: ignore without replying.
      continue;
    }

    int64_t chatId = message["chat"]["id"] | (int64_t)0;
    const char *rawText = message["text"] | "";
    String text(rawText);
    text.trim();
    if (text.length() == 0) {
      continue;
    }

    int spaceIdx = text.indexOf(' ');
    String command = spaceIdx < 0 ? text : text.substring(0, spaceIdx);
    String args = spaceIdx < 0 ? String("") : text.substring(spaceIdx + 1);
    args.trim();

    int atIdx = command.indexOf('@');
    if (atIdx >= 0) {
      command = command.substring(0, atIdx);
    }

    dispatchCommand(chatId, command, args);
  }
}

bool waitForNtpSync(unsigned long maxWaitMs) {
  unsigned long start = millis();
  while (time(nullptr) < kMinValidUnixTime) {
    if (millis() - start > maxWaitMs) {
      return false;
    }
    vTaskDelay(pdMS_TO_TICKS(500));
  }
  return true;
}

void pollTask(void *) {
  while (WiFi.status() != WL_CONNECTED) {
    vTaskDelay(pdMS_TO_TICKS(1000));
  }
  // Best-effort: proceed even if NTP never syncs, so Telegram polling never
  // blocks startup indefinitely. postAgentCommand() already refuses to run
  // without a valid clock.
  waitForNtpSync(10000);

  unsigned long backoffMs = kBackoffMinMs;
  currentStatus.store(TelegramClient::Status::Polling);

  for (;;) {
    if (WiFi.status() != WL_CONNECTED) {
      vTaskDelay(pdMS_TO_TICKS(1000));
      continue;
    }

    WiFiClientSecure client;
    client.setCACert(TELEGRAM_ROOT_CA_PEM);
    HTTPClient https;
    String url = telegramApiUrl("getUpdates");
    url += "?timeout=";
    url += String(TELEGRAM_LONG_POLL_TIMEOUT_SECONDS);
    url += "&offset=";
    url += String((long)lastUpdateId);

    bool ok = https.begin(client, url);
    if (ok) {
      https.setTimeout((TELEGRAM_LONG_POLL_TIMEOUT_SECONDS + 10) * 1000);
      int statusCode = https.GET();
      if (statusCode == 200) {
        JsonDocument doc;
        DeserializationError err = deserializeJson(doc, https.getStream());
        https.end();
        if (!err) {
          JsonArray results = doc["result"].as<JsonArray>();
          processUpdates(results, initialSyncDone);
          initialSyncDone = true;
          backoffMs = kBackoffMinMs;
          currentStatus.store(TelegramClient::Status::Polling);
        } else {
          ok = false;
        }
      } else {
        https.end();
        Serial.printf("telegram getUpdates failed: %d\n", statusCode);
        ok = false;
      }
    }

    if (!ok) {
      currentStatus.store(TelegramClient::Status::Error);
      vTaskDelay(pdMS_TO_TICKS(backoffMs));
      backoffMs = backoffMs * 2 < kBackoffMaxMs ? backoffMs * 2 : kBackoffMaxMs;
    } else {
      vTaskDelay(pdMS_TO_TICKS(50));
    }
  }
}

} // namespace

namespace TelegramClient {

void begin() {
  if (!isTelegramConfigured()) {
    currentStatus.store(Status::Disabled);
    return;
  }
  // Pinned to core 0 so the long-poll HTTP call (up to
  // TELEGRAM_LONG_POLL_TIMEOUT_SECONDS) never blocks the touch UI / STATUS
  // loop, which runs as loopTask on core 1.
  xTaskCreatePinnedToCore(pollTask, "telegram_poll", 12288, nullptr, 1,
                          &taskHandle, 0);
}

Status status() { return currentStatus.load(); }

} // namespace TelegramClient
