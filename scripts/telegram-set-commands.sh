#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
config_file="${repo_root}/firmware/config.toml"

if [[ ! -f "${config_file}" ]]; then
  echo "firmware/config.toml not found. Copy config.example.toml and set telegram_bot_token first." >&2
  exit 1
fi

token="$(
  perl -ne 'if (/^\s*telegram_bot_token\s*=\s*"([^"]*)"/) { print $1; exit }' \
    "${config_file}"
)"

if [[ -z "${token}" || "${token}" == "replace-with-your-telegram-bot-token" ]]; then
  echo "telegram_bot_token is not configured in firmware/config.toml." >&2
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
