#pragma once

// Root CA used to validate the TLS certificate chain presented by
// https://api.telegram.org. This lets TelegramClient use
// WiFiClientSecure::setCACert() instead of setInsecure(), so the bot token
// (embedded in every Telegram API URL) and command traffic (including
// /reboot and /shutdown confirmations) are protected by real certificate
// validation, not just opportunistic TLS.
//
// api.telegram.org's leaf certificate is currently issued by "Go Daddy
// Secure Certificate Authority - G2", which chains up to the self-signed
// root below. We pin the root rather than the leaf or intermediate because
// the root has a long validity window, while the leaf certificate rotates
// roughly every year.
//
// Captured 2026-09-01 from a live TLS handshake:
//   openssl s_client -connect api.telegram.org:443 \
//     -servername api.telegram.org -showcerts
//
// Root certificate details (for manual verification when rotating this
// file):
//   Subject: C=US, ST=Arizona, L=Scottsdale, O=GoDaddy.com, Inc.,
//            CN=Go Daddy Root Certificate Authority - G2
//   Valid until: 2037-12-31
//   SHA-256 fingerprint:
//     45:14:0B:32:47:EB:9C:C8:C5:B4:F0:D7:B5:30:91:F7:
//     32:92:08:9E:6E:5A:63:E2:74:9D:D3:AC:A9:19:8E:DA
//
// Operational note: if Telegram ever switches to a different certificate
// authority, TLS connections to api.telegram.org will start failing (the
// M5Stack screen will show "Telegram: error") until this file is updated
// with the new root CA and the firmware is reflashed. See
// docs/external-access.md and docs/security.md for the rotation procedure.
static const char *TELEGRAM_ROOT_CA_PEM =
    "-----BEGIN CERTIFICATE-----\n"
    "MIIDxTCCAq2gAwIBAgIBADANBgkqhkiG9w0BAQsFADCBgzELMAkGA1UEBhMCVVMx\n"
    "EDAOBgNVBAgTB0FyaXpvbmExEzARBgNVBAcTClNjb3R0c2RhbGUxGjAYBgNVBAoT\n"
    "EUdvRGFkZHkuY29tLCBJbmMuMTEwLwYDVQQDEyhHbyBEYWRkeSBSb290IENlcnRp\n"
    "ZmljYXRlIEF1dGhvcml0eSAtIEcyMB4XDTA5MDkwMTAwMDAwMFoXDTM3MTIzMTIz\n"
    "NTk1OVowgYMxCzAJBgNVBAYTAlVTMRAwDgYDVQQIEwdBcml6b25hMRMwEQYDVQQH\n"
    "EwpTY290dHNkYWxlMRowGAYDVQQKExFHb0RhZGR5LmNvbSwgSW5jLjExMC8GA1UE\n"
    "AxMoR28gRGFkZHkgUm9vdCBDZXJ0aWZpY2F0ZSBBdXRob3JpdHkgLSBHMjCCASIw\n"
    "DQYJKoZIhvcNAQEBBQADggEPADCCAQoCggEBAL9xYgjx+lk09xvJGKP3gElY6SKD\n"
    "E6bFIEMBO4Tx5oVJnyfq9oQbTqC023CYxzIBsQU+B07u9PpPL1kwIuerGVZr4oAH\n"
    "/PMWdYA5UXvl+TW2dE6pjYIT5LY/qQOD+qK+ihVqf94Lw7YZFAXK6sOoBJQ7Rnwy\n"
    "DfMAZiLIjWltNowRGLfTshxgtDj6AozO091GB94KPutdfMh8+7ArU6SSYmlRJQVh\n"
    "GkSBjCypQ5Yj36w6gZoOKcUcqeldHraenjAKOc7xiID7S13MMuyFYkMlNAJWJwGR\n"
    "tDtwKj9useiciAF9n9T521NtYJ2/LOdYq7hfRvzOxBsDPAnrSTFcaUaz4EcCAwEA\n"
    "AaNCMEAwDwYDVR0TAQH/BAUwAwEB/zAOBgNVHQ8BAf8EBAMCAQYwHQYDVR0OBBYE\n"
    "FDqahQcQZyi27/a9BUFuIMGU2g/eMA0GCSqGSIb3DQEBCwUAA4IBAQCZ21151fmX\n"
    "WWcDYfF+OwYxdS2hII5PZYe096acvNjpL9DbWu7PdIxztDhC2gV7+AJ1uP2lsdeu\n"
    "9tfeE8tTEH6KRtGX+rcuKxGrkLAngPnon1rpN5+r5N9ss4UXnT3ZJE95kTXWXwTr\n"
    "gIOrmgIttRD02JDHBHNA7XIloKmf7J6raBKZV8aPEjoJpL1E/QYVN8Gb5DKj7Tjo\n"
    "2GTzLH4U/ALqn83/B2gX2yKQOC16jdFU8WnjXzPKej17CuPKf1855eJ1usV2GDPO\n"
    "LPAvTK33sefOT6jEm0pUBsV/fdUID+Ic/n4XuKxe9tQWskMJDE32p2u0mYRlynqI\n"
    "4uJEvlz36hz1\n"
    "-----END CERTIFICATE-----\n";
