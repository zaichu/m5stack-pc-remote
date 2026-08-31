#pragma once

// Wake-on-LAN and Windows Agent HTTP command helpers shared by the touch UI
// (main.cpp) and the Telegram client (telegram_client.cpp). Both callers can
// run from different FreeRTOS tasks/cores, so the implementation serializes
// access to the shared WiFiUDP socket and PC status flag internally.

namespace PowerController {

// Must be called once from setup() before any other function here.
void begin();

// Sends a Wake-on-LAN magic packet to PC_MAC_ADDRESS.
bool sendWakeOnLan();

// Sends an HMAC-signed POST to the Windows Agent at the given path
// (e.g. "/reboot" or "/shutdown").
bool postAgentCommand(const char *path);

// Refreshes the cached PC online/offline state via ICMP ping.
void updateStatus();

// Returns the state last recorded by updateStatus().
bool isPcOnline();

} // namespace PowerController
