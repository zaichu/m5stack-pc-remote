#include <Arduino.h>
#include <ArduinoJson.h>
#include <HTTPClient.h>
#include <ESP32Ping.h>
#include <M5Unified.h>
#include <esp_system.h>
#include <mbedtls/md.h>
#include <time.h>
#include <WiFi.h>
#include <WiFiUdp.h>

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
WiFiUDP udp;
bool pcOnline = false;
bool wifiConnectStarted = false;
unsigned long lastStatusAt = 0;
unsigned long lastWifiAttemptAt = 0;

struct Button {
  int32_t x;
  int32_t y;
  int32_t w;
  int32_t h;
  const char *label;
  uint16_t color;
};

Button wakeButton{10, 165, 95, 55, "WAKE", TFT_DARKGREEN};
Button rebootButton{112, 165, 95, 55, "REBOOT", TFT_ORANGE};
Button shutdownButton{215, 165, 95, 55, "SHUTDOWN", TFT_RED};

enum class PendingAction {
  None,
  Reboot,
  Shutdown,
};

PendingAction pendingAction = PendingAction::None;

bool parseMac(const char *text, uint8_t out[6]) {
  unsigned int values[6];
  if (sscanf(text, "%x:%x:%x:%x:%x:%x", &values[0], &values[1], &values[2],
             &values[3], &values[4], &values[5]) != 6) {
    return false;
  }
  for (int i = 0; i < 6; ++i) {
    if (values[i] > 0xFF) {
      return false;
    }
    out[i] = static_cast<uint8_t>(values[i]);
  }
  return true;
}

void drawButton(const Button &button) {
  M5.Display.fillRoundRect(button.x, button.y, button.w, button.h, 6,
                           button.color);
  M5.Display.setTextColor(TFT_WHITE, button.color);
  M5.Display.setTextDatum(middle_center);
  M5.Display.setTextSize(strlen(button.label) > 6 ? 1 : 2);
  M5.Display.drawString(button.label, button.x + button.w / 2,
                        button.y + button.h / 2);
}

bool contains(const Button &button, int32_t x, int32_t y) {
  return x >= button.x && x <= button.x + button.w && y >= button.y &&
         y <= button.y + button.h;
}

String bytesToHex(const uint8_t *bytes, size_t len) {
  static const char *hex = "0123456789abcdef";
  String out;
  out.reserve(len * 2);
  for (size_t i = 0; i < len; ++i) {
    out += hex[(bytes[i] >> 4) & 0x0F];
    out += hex[bytes[i] & 0x0F];
  }
  return out;
}

String sha256Hex(const String &body) {
  uint8_t digest[32];
  mbedtls_md_context_t ctx;
  mbedtls_md_init(&ctx);
  const mbedtls_md_info_t *info = mbedtls_md_info_from_type(MBEDTLS_MD_SHA256);
  mbedtls_md_setup(&ctx, info, 0);
  mbedtls_md_starts(&ctx);
  mbedtls_md_update(&ctx, reinterpret_cast<const unsigned char *>(body.c_str()),
                    body.length());
  mbedtls_md_finish(&ctx, digest);
  mbedtls_md_free(&ctx);
  return bytesToHex(digest, sizeof(digest));
}

String hmacSha256Hex(const String &message) {
  uint8_t digest[32];
  mbedtls_md_context_t ctx;
  mbedtls_md_init(&ctx);
  const mbedtls_md_info_t *info = mbedtls_md_info_from_type(MBEDTLS_MD_SHA256);
  mbedtls_md_setup(&ctx, info, 1);
  mbedtls_md_hmac_starts(
      &ctx, reinterpret_cast<const unsigned char *>(AGENT_SHARED_SECRET),
      strlen(AGENT_SHARED_SECRET));
  mbedtls_md_hmac_update(
      &ctx, reinterpret_cast<const unsigned char *>(message.c_str()),
      message.length());
  mbedtls_md_hmac_finish(&ctx, digest);
  mbedtls_md_free(&ctx);
  return bytesToHex(digest, sizeof(digest));
}

String nonce() {
  return String(static_cast<uint32_t>(esp_random()), HEX) + String("-") +
         String(millis(), HEX);
}

String agentUrl(const char *path) {
  return String("http://") + AGENT_HOST + ":" + String(AGENT_PORT) + path;
}

bool postAgentCommand(const char *path) {
  if (WiFi.status() != WL_CONNECTED) {
    return false;
  }

  time_t timestamp = time(nullptr);
  if (timestamp < 1700000000) {
    Serial.println("NTP time is not ready");
    return false;
  }

  String body = "{\"confirm\":true}";
  String requestNonce = nonce();
  String canonical = String("POST\n") + path + "\n" + String(timestamp) +
                     "\n" + requestNonce + "\n" + sha256Hex(body);
  String signature = hmacSha256Hex(canonical);

  HTTPClient http;
  http.begin(agentUrl(path));
  http.addHeader("Content-Type", "application/json");
  http.addHeader("X-Timestamp", String(timestamp));
  http.addHeader("X-Nonce", requestNonce);
  http.addHeader("X-Signature", signature);
  int status = http.POST(body);
  http.end();
  Serial.printf("agent %s -> %d\n", path, status);
  return status >= 200 && status < 300;
}

void connectWifi() {
  if (WiFi.status() == WL_CONNECTED) {
    return;
  }
  unsigned long now = millis();
  if (wifiConnectStarted && now - lastWifiAttemptAt < WIFI_RECONNECT_INTERVAL_MS) {
    return;
  }
  wifiConnectStarted = true;
  lastWifiAttemptAt = now;
  WiFi.mode(WIFI_STA);
  WiFi.begin(WIFI_SSID, WIFI_PASSWORD);
}

bool sendWakeOnLan() {
  uint8_t mac[6];
  if (!parseMac(PC_MAC_ADDRESS, mac)) {
    Serial.println("invalid PC_MAC_ADDRESS");
    return false;
  }

  uint8_t packet[102];
  memset(packet, 0xFF, 6);
  for (int i = 1; i <= 16; ++i) {
    memcpy(packet + i * 6, mac, 6);
  }

  IPAddress broadcast;
  if (!broadcast.fromString(WOL_BROADCAST_ADDRESS)) {
    Serial.println("invalid WOL_BROADCAST_ADDRESS");
    return false;
  }

  udp.beginPacket(broadcast, WOL_PORT);
  udp.write(packet, sizeof(packet));
  return udp.endPacket() == 1;
}

void updateStatus() {
  if (WiFi.status() != WL_CONNECTED) {
    pcOnline = false;
    return;
  }
  IPAddress pcIp;
  if (!pcIp.fromString(PC_IP_ADDRESS)) {
    pcOnline = false;
    return;
  }
  pcOnline = Ping.ping(pcIp, 2);
}

void drawScreen() {
  M5.Display.fillScreen(TFT_BLACK);
  M5.Display.setTextDatum(top_left);
  M5.Display.setTextSize(1);
  M5.Display.setTextColor(TFT_LIGHTGREY, TFT_BLACK);
  M5.Display.drawString("m5stack-pc-remote", 12, 10);

  M5.Display.setTextColor(WiFi.status() == WL_CONNECTED ? TFT_GREEN : TFT_RED,
                          TFT_BLACK);
  String wifiLine = WiFi.status() == WL_CONNECTED
                        ? "Wi-Fi: " + WiFi.localIP().toString() + " RSSI " +
                              String(WiFi.RSSI()) + "dBm"
                        : "Wi-Fi: disconnected";
  M5.Display.drawString(wifiLine, 12, 30);

  M5.Display.setTextDatum(middle_center);
  M5.Display.setTextSize(4);
  M5.Display.setTextColor(pcOnline ? TFT_GREEN : TFT_RED, TFT_BLACK);
  M5.Display.drawString(pcOnline ? "ONLINE" : "OFFLINE", 160, 92);

  M5.Display.setTextSize(1);
  M5.Display.setTextColor(TFT_LIGHTGREY, TFT_BLACK);
  M5.Display.drawString(PC_HOSTNAME, 160, 132);

  drawButton(wakeButton);
  if (pcOnline) {
    drawButton(rebootButton);
    drawButton(shutdownButton);
  }
}

void showToast(const char *message, uint16_t color) {
  M5.Display.fillRect(0, 225, 320, 15, TFT_BLACK);
  M5.Display.setTextDatum(middle_center);
  M5.Display.setTextSize(1);
  M5.Display.setTextColor(color, TFT_BLACK);
  M5.Display.drawString(message, 160, 232);
}

void drawConfirm(PendingAction action) {
  M5.Display.fillScreen(TFT_BLACK);
  M5.Display.setTextDatum(middle_center);
  M5.Display.setTextSize(2);
  M5.Display.setTextColor(TFT_WHITE, TFT_BLACK);
  M5.Display.drawString(action == PendingAction::Reboot ? "Confirm REBOOT"
                                                        : "Confirm SHUTDOWN",
                        160, 55);
  Button cancel{20, 145, 130, 60, "CANCEL", TFT_DARKGREY};
  Button ok{170, 145, 130, 60, "OK", TFT_RED};
  drawButton(cancel);
  drawButton(ok);
}

void handleConfirmTouch(int32_t x, int32_t y) {
  Button cancel{20, 145, 130, 60, "CANCEL", TFT_DARKGREY};
  Button ok{170, 145, 130, 60, "OK", TFT_RED};
  if (contains(cancel, x, y)) {
    pendingAction = PendingAction::None;
    drawScreen();
    return;
  }
  if (!contains(ok, x, y)) {
    return;
  }

  const char *path = pendingAction == PendingAction::Reboot ? "/reboot"
                                                            : "/shutdown";
  bool okResult = postAgentCommand(path);
  pendingAction = PendingAction::None;
  drawScreen();
  showToast(okResult ? "Command accepted" : "Command failed",
            okResult ? TFT_GREEN : TFT_RED);
}
} // namespace

void setup() {
  auto cfg = M5.config();
  M5.begin(cfg);
  Serial.begin(115200);
  // WiFi.mode() must run before any UDP/lwIP call, or the TCP/IP task's
  // mbox is not ready yet and udp.begin() crashes with "Invalid mbox".
  connectWifi();
  udp.begin(WOL_PORT);
  configTime(0, 0, "pool.ntp.org", "time.google.com");
  updateStatus();
  drawScreen();
}

void loop() {
  M5.update();
  connectWifi();

  unsigned long now = millis();
  if (now - lastStatusAt >= STATUS_INTERVAL_MS) {
    lastStatusAt = now;
    updateStatus();
    drawScreen();
  }

  auto touch = M5.Touch.getDetail();
  if (!touch.wasClicked()) {
    return;
  }

  if (pendingAction != PendingAction::None) {
    handleConfirmTouch(touch.x, touch.y);
    return;
  }

  if (contains(wakeButton, touch.x, touch.y)) {
    bool ok = sendWakeOnLan();
    showToast(ok ? "Magic Packet sent" : "WOL failed",
              ok ? TFT_GREEN : TFT_RED);
    delay(500);
    updateStatus();
    drawScreen();
  } else if (pcOnline && contains(rebootButton, touch.x, touch.y)) {
    pendingAction = PendingAction::Reboot;
    drawConfirm(pendingAction);
  } else if (pcOnline && contains(shutdownButton, touch.x, touch.y)) {
    pendingAction = PendingAction::Shutdown;
    drawConfirm(pendingAction);
  }
}
