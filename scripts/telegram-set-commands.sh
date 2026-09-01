#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
config_file="${repo_root}/firmware/include/config.h"

if [[ ! -f "${config_file}" ]]; then
  echo "firmware/include/config.h not found. Copy config.example.h and set TELEGRAM_BOT_TOKEN first." >&2
  exit 1
fi

token="$(
  perl -ne 'if (/^#define\s+TELEGRAM_BOT_TOKEN\s+"([^"]*)"/) { print $1; exit }' \
    "${config_file}"
)"

if [[ -z "${token}" || "${token}" == "replace-with-your-telegram-bot-token" ]]; then
  echo "TELEGRAM_BOT_TOKEN is not configured in firmware/include/config.h." >&2
  exit 1
fi

commands='[
  {"command":"status","description":"PC状態を表示"},
  {"command":"wake","description":"PCへWake-on-LANを送信"},
  {"command":"reboot","description":"確認後にPCを再起動"},
  {"command":"shutdown","description":"確認後にPCをシャットダウン"}
]'

response="$(
  curl -fsS -X POST "https://api.telegram.org/bot${token}/setMyCommands" \
    -H "Content-Type: application/json" \
    --data "{\"commands\":${commands}}"
)"

ok="$(printf '%s' "${response}" | perl -ne 'print $1 if /"ok"\s*:\s*(true|false)/')"
if [[ "${ok}" != "true" ]]; then
  echo "Failed to set Telegram bot commands." >&2
  printf '%s\n' "${response}" | perl -pe 's#bot[0-9]+:[A-Za-z0-9_-]+#bot<redacted>#g' >&2
  exit 1
fi

echo "Telegram bot commands updated."
