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

# placeholderのままではTelegram APIへ投げずに止める。READMEが案内する
# placeholderは config.example.toml と同じ値に統一してあるが、旧READMEの値
# (123456789:your-real-bot-token) が残っている可能性もあるため両方拒否する。
# token形式(<数字>:<英数字等>)に合わない値もここで止める。値は表示しない。
if [[ -z "${token}" || "${token}" == "replace-with-your-telegram-bot-token" || "${token}" == "123456789:your-real-bot-token" ]]; then
  echo "telegram_bot_token is not configured in firmware/config.toml." >&2
  echo "Copy firmware/config.example.toml to firmware/config.toml and set a real bot token." >&2
  exit 1
fi

if ! [[ "${token}" =~ ^[0-9]+:[A-Za-z0-9_-]+$ ]]; then
  echo "telegram_bot_token in firmware/config.toml does not look like a Telegram bot token (<digits>:<secret>)." >&2
  exit 1
fi

# 日常的に使う電源操作と、そこから辿れる入口(/settings)だけを登録する。
#
# 意図的に登録しないもの:
# - /set_ip /set_status_addr /set_wol_port: 値の引数が必須で、一覧からタップしても
#   そのままでは実行できない。/settings の応答が現在値入りの実行例を出すので、
#   そこをコピーして使う導線にする。
# - /lock /unlock: 設定変更と同じく日常操作ではない。/settings が現在のロック状態と
#   切り替えコマンドを表示する。
# - /confirm_reboot /confirm_shutdown /confirm_update /confirm_set: nonce引数が必須の手入力
#   フォールバック。通常はインラインボタンで確定する。
commands='[
  {"command":"status","description":"PC状態を表示"},
  {"command":"wake","description":"PCへWake-on-LANを送信"},
  {"command":"reboot","description":"確認後にPCを再起動"},
  {"command":"shutdown","description":"確認後にPCをシャットダウン"},
  {"command":"update","description":"確認後にfirmwareを更新"},
  {"command":"settings","description":"設定の現在値と変更・ロック操作"}
]'

# curlの失敗時は set -e で黙って終わらせず、tokenをredactした上で理由を出す。
# 失敗の大半は無効なtoken(401)かネットワーク到達性の問題である。
curl_stderr="$(mktemp)"
if ! response="$(
  curl -fsS -X POST "https://api.telegram.org/bot${token}/setMyCommands" \
    -H "Content-Type: application/json" \
    --data "{\"commands\":${commands}}" 2>"${curl_stderr}"
)"; then
  echo "Failed to call Telegram setMyCommands. Check the network connection and that telegram_bot_token in firmware/config.toml is correct." >&2
  perl -pe 's#bot[0-9]+:[A-Za-z0-9_-]+#bot<redacted>#g' "${curl_stderr}" >&2 || true
  rm -f "${curl_stderr}"
  exit 1
fi
rm -f "${curl_stderr}"

ok="$(printf '%s' "${response}" | perl -ne 'print $1 if /"ok"\s*:\s*(true|false)/')"
if [[ "${ok}" != "true" ]]; then
  echo "Failed to set Telegram bot commands." >&2
  printf '%s\n' "${response}" | perl -pe 's#bot[0-9]+:[A-Za-z0-9_-]+#bot<redacted>#g' >&2
  exit 1
fi

echo "Telegram bot commands updated."
