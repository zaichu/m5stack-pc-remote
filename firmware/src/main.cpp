#include <Arduino.h>
#include <M5Unified.h>
#include <WiFi.h>

#include "power_controller.h"
#include "telegram_client.h"

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

void drawTelegramLine() {
  const char *label;
  uint16_t color;
  switch (TelegramClient::status()) {
  case TelegramClient::Status::Polling:
    label = "Telegram: polling";
    color = TFT_GREEN;
    break;
  case TelegramClient::Status::Error:
    label = "Telegram: error";
    color = TFT_RED;
    break;
  case TelegramClient::Status::Disabled:
  default:
    label = "Telegram: disabled";
    color = TFT_LIGHTGREY;
    break;
  }
  M5.Display.setTextDatum(top_left);
  M5.Display.setTextSize(1);
  M5.Display.setTextColor(color, TFT_BLACK);
  M5.Display.drawString(label, 12, 44);
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

  drawTelegramLine();

  bool pcOnline = PowerController::isPcOnline();
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
  bool okResult = PowerController::postAgentCommand(path);
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
  PowerController::begin();
  TelegramClient::begin();
  configTime(0, 0, "pool.ntp.org", "time.google.com");
  PowerController::updateStatus();
  drawScreen();
}

void loop() {
  M5.update();
  connectWifi();

  // PowerController::begin() already refreshes PC status on its own
  // background task (see power_controller.cpp), so this only needs to
  // redraw periodically to reflect whatever it last found. Redrawing here
  // instead of relying on that task to trigger a redraw keeps all display
  // writes on loopTask.
  unsigned long now = millis();
  if (now - lastStatusAt >= STATUS_INTERVAL_MS) {
    lastStatusAt = now;
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

  bool pcOnline = PowerController::isPcOnline();
  if (contains(wakeButton, touch.x, touch.y)) {
    bool ok = PowerController::sendWakeOnLan();
    showToast(ok ? "Magic Packet sent" : "WOL failed",
              ok ? TFT_GREEN : TFT_RED);
    delay(500);
    PowerController::updateStatus();
    drawScreen();
  } else if (pcOnline && contains(rebootButton, touch.x, touch.y)) {
    pendingAction = PendingAction::Reboot;
    drawConfirm(pendingAction);
  } else if (pcOnline && contains(shutdownButton, touch.x, touch.y)) {
    pendingAction = PendingAction::Shutdown;
    drawConfirm(pendingAction);
  }
}
