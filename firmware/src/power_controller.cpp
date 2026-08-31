#include "power_controller.h"

#include <Arduino.h>
#include <ESP32Ping.h>
#include <HTTPClient.h>
#include <WiFi.h>
#include <WiFiUdp.h>
#include <mbedtls/md.h>
#include <time.h>

#include <freertos/FreeRTOS.h>
#include <freertos/semphr.h>

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
SemaphoreHandle_t powerMutex = nullptr;
volatile bool pcOnline = false;

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
  return String("http://") + PC_IP_ADDRESS + ":" + String(AGENT_PORT) + path;
}

} // namespace

namespace PowerController {

void begin() {
  powerMutex = xSemaphoreCreateMutex();
  udp.begin(WOL_PORT);
}

bool sendWakeOnLan() {
  xSemaphoreTake(powerMutex, portMAX_DELAY);

  uint8_t mac[6];
  if (!parseMac(PC_MAC_ADDRESS, mac)) {
    Serial.println("invalid PC_MAC_ADDRESS");
    xSemaphoreGive(powerMutex);
    return false;
  }

  uint8_t packet[102];
  memset(packet, 0xFF, 6);
  for (int i = 1; i <= 16; ++i) {
    memcpy(packet + i * 6, mac, 6);
  }

  // Use the limited broadcast address instead of a subnet-directed broadcast
  // computed from the local subnet mask, so WOL keeps working regardless of
  // the LAN's actual prefix length (e.g. a /16 network, not /24).
  IPAddress broadcast(255, 255, 255, 255);

  bool began = udp.beginPacket(broadcast, WOL_PORT) != 0;
  size_t written = udp.write(packet, sizeof(packet));
  bool ended = udp.endPacket() == 1;
  Serial.printf("WOL: beginPacket=%d written=%u/%u endPacket=%d\n", began,
                (unsigned)written, (unsigned)sizeof(packet), ended);

  xSemaphoreGive(powerMutex);
  return began && written == sizeof(packet) && ended;
}

bool postAgentCommand(const char *path) {
  xSemaphoreTake(powerMutex, portMAX_DELAY);

  if (WiFi.status() != WL_CONNECTED) {
    xSemaphoreGive(powerMutex);
    return false;
  }

  time_t timestamp = time(nullptr);
  if (timestamp < 1700000000) {
    Serial.println("NTP time is not ready");
    xSemaphoreGive(powerMutex);
    return false;
  }

  String body = "{\"confirm\":true}";
  String requestNonce = nonce();
  String canonical = String("POST\n") + path + "\n" + String(timestamp) +
                     "\n" + requestNonce + "\n" + sha256Hex(body);
  String signature = hmacSha256Hex(canonical);

  HTTPClient http;
  http.begin(agentUrl(path));
  // Keep this short: the call holds powerMutex, which also blocks the
  // touch UI confirm flow and the Telegram task while a request is in
  // flight, so a slow/unreachable Windows Agent should fail fast.
  http.setConnectTimeout(3000);
  http.setTimeout(3000);
  http.addHeader("Content-Type", "application/json");
  http.addHeader("X-Timestamp", String(timestamp));
  http.addHeader("X-Nonce", requestNonce);
  http.addHeader("X-Signature", signature);
  int status = http.POST(body);
  http.end();
  Serial.printf("agent %s -> %d\n", path, status);

  xSemaphoreGive(powerMutex);
  return status >= 200 && status < 300;
}

void updateStatus() {
  xSemaphoreTake(powerMutex, portMAX_DELAY);

  if (WiFi.status() != WL_CONNECTED) {
    pcOnline = false;
    xSemaphoreGive(powerMutex);
    return;
  }
  IPAddress pcIp;
  if (!pcIp.fromString(PC_IP_ADDRESS)) {
    pcOnline = false;
    xSemaphoreGive(powerMutex);
    return;
  }
  pcOnline = Ping.ping(pcIp, 2);

  xSemaphoreGive(powerMutex);
}

bool isPcOnline() { return pcOnline; }

} // namespace PowerController
