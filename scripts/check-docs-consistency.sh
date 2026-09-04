#!/usr/bin/env bash
# README / docs と実装の食い違い(Issue #131)を守る整合性チェック。
#
# - NVS size/offset が partitions.csv / provision script と一致すること
# - Telegram の token・コマンド・設定手順の記述が script / example と一致すること
# - pr-workflow SKILL の記述が Makefile / CI の実態と一致すること
#
# standalone で実行できる: `bash scripts/check-docs-consistency.sh`
# `REPO_ROOT` を上書きすれば別ツリー(例: 修正前のHEAD展開)に対しても実行できる。
set -euo pipefail

repo_root="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
failures=0

fail() {
  echo "DOC-CONSISTENCY FAIL: $1" >&2
  failures=$((failures + 1))
}

pass() {
  echo "DOC-CONSISTENCY OK: $1"
}

readme="${repo_root}/README.md"
partitions="${repo_root}/firmware/partitions.csv"
provision="${repo_root}/scripts/provision-firmware-nvs.py"
tg_script="${repo_root}/scripts/telegram-set-commands.sh"
fw_example="${repo_root}/firmware/config.example.toml"
skill="${repo_root}/.claude/skills/pr-workflow/SKILL.md"
makefile="${repo_root}/Makefile"
ci_yml="${repo_root}/.github/workflows/ci.yml"
sys_json="${repo_root}/docs/architecture/system.json"
sys_html="${repo_root}/docs/architecture/index.html"

# 1. NVS size/offset (Issue #131 項目1)。0x6000 を書くと otadata へはみ出す。
# バックティックはREADMEのMarkdown記法そのものを検索している。コマンド置換では
# ないためシングルクォートが正しい。
# shellcheck disable=SC2016
if grep -q 'size `0x4000`' "${readme}"; then
  pass "README NVS size is 0x4000"
else
  fail "README must describe NVS size as 0x4000"
fi

if grep -q '0x6000' "${readme}"; then
  fail "README must not contain stale NVS size 0x6000"
else
  pass "README has no stale 0x6000"
fi

if grep -q 'firmware/partitions.csv' "${readme}"; then
  pass "README points at firmware/partitions.csv for partition layout"
else
  fail "README must reference firmware/partitions.csv (not sdkconfig.defaults) for partition layout"
fi

if grep -qE '^nvs, *data, *nvs, *, *0x4000,' "${partitions}"; then
  pass "partitions.csv nvs size is 0x4000"
else
  fail "partitions.csv nvs entry must be size 0x4000"
fi

if grep -q 'DEFAULT_NVS_SIZE = 0x4000' "${provision}" && grep -q 'DEFAULT_NVS_OFFSET = 0x9000' "${provision}"; then
  pass "provision script defaults are offset 0x9000 / size 0x4000"
else
  fail "provision script defaults must be offset 0x9000 / size 0x4000"
fi

# バックティックはREADMEのMarkdown記法そのものを検索している。コマンド置換では
# ないためシングルクォートが正しい。
# shellcheck disable=SC2016
if grep -q 'offset `0x9000`' "${readme}"; then
  pass "README NVS offset is 0x9000"
else
  fail "README must describe NVS offset as 0x9000"
fi

# 2. bot token の扱い (Issue #131 項目2)。security.md の条件付き許容が正本。
if grep -q '一切渡りません' "${readme}"; then
  fail "README must not claim the bot token is never given to the bridge"
else
  pass "README has no absolute no-bridge-token claim"
fi

if grep -q '転送しない' "${readme}" || grep -q '転送しない' "${sys_json}" || grep -q '転送しない' "${sys_html}"; then
  fail "README/architecture diagram must not claim the bot token is never transferred to the bridge"
else
  pass "no absolute no-transfer claim in README/architecture"
fi

if grep -q '認証失敗アラート' "${readme}"; then
  pass "README points at the conditional bridge-token allowance"
else
  fail "README must explain the conditional bridge-token allowance (auth-failure alerts)"
fi

# 3. 環境変数ではなく config.toml が正本 (Issue #131 項目3)。
if grep -q '環境変数に設定' "${readme}"; then
  fail "README must not instruct env-var setup for telegram-set-commands.sh (it only reads firmware/config.toml)"
else
  pass "README does not instruct env-var setup"
fi

if grep -q '123456789:your-real-bot-token' "${readme}"; then
  fail "README must not use the old placeholder 123456789:your-real-bot-token (bypasses the script guard)"
else
  pass "README has no guard-bypassing placeholder"
fi

readme_token="$(grep -E '^telegram_bot_token *=' "${readme}" | head -n1 | sed -E 's/.*"(.*)".*/\1/')"
example_token="$(grep -E '^telegram_bot_token *=' "${fw_example}" | head -n1 | sed -E 's/.*"(.*)".*/\1/')"
if [[ -n "${readme_token}" && "${readme_token}" == "${example_token}" ]]; then
  pass "README token placeholder matches config.example.toml"
else
  fail "README token placeholder (${readme_token:-missing}) must match config.example.toml (${example_token:-missing})"
fi

readme_user="$(grep -E '^telegram_allowed_user_id *=' "${readme}" | head -n1 | sed -E 's/.*"(.*)".*/\1/')"
example_user="$(grep -E '^telegram_allowed_user_id *=' "${fw_example}" | head -n1 | sed -E 's/.*"(.*)".*/\1/')"
if [[ -n "${readme_user}" && "${readme_user}" == "${example_user}" ]]; then
  pass "README user-id placeholder matches config.example.toml"
else
  fail "README user-id placeholder (${readme_user:-missing}) must match config.example.toml (${example_user:-missing})"
fi

# スクリプト側の guard: 旧・新どちらの placeholder も拒否し、形式も見る。
if grep -q 'replace-with-your-telegram-bot-token' "${tg_script}" && grep -q '123456789:your-real-bot-token' "${tg_script}"; then
  pass "telegram script rejects both old and current placeholders"
else
  fail "telegram script must reject both telegram token placeholders"
fi

if grep -q 'Failed to call Telegram setMyCommands' "${tg_script}"; then
  pass "telegram script reports curl failures instead of exiting silently"
else
  fail "telegram script must print a message when the Telegram API call fails"
fi

# 4. 登録されるコマンド候補 (Issue #131 項目4)。lock/unlock は有効だが一覧には無い。
# 候補リストの行(`- `/xxx``)だけを見て、直後の説明文中の言及は数えない。
# バッククォートは awk 変数経由で渡す(bash のダブルクォート内に書かない)。
bt='`'
candidates="$(awk -v bt="${bt}" '/登録される候補/{flag=1; next} /^### /{flag=0} flag && index($0, "- " bt "/") == 1' "${readme}")"
if echo "${candidates}" | grep -q '/settings'; then
  pass "README lists /settings as a registered candidate"
else
  fail "README registered candidates must include /settings"
fi

if echo "${candidates}" | grep -qE '/lock|/unlock'; then
  fail "README must not list /lock//unlock as registered candidates (script registers status/wake/reboot/shutdown/settings)"
else
  pass "README does not list /lock//unlock as registered"
fi

for cmd in status wake reboot shutdown settings; do
  if grep -q "\"command\":\"${cmd}\"" "${tg_script}"; then
    pass "script registers /${cmd}"
  else
    fail "script must register /${cmd} in setMyCommands"
  fi
done

# 5. pr-issue-link-check の実行場所 (Issue #131 項目5)。SKILL は実態どおりに書く。
check_line="$(grep -E '^check:' "${makefile}")"
if echo "${check_line}" | grep -q 'pr-issue-link-check'; then
  fail "Makefile check must not claim pr-issue-link-check (SKILL describes pre-push only)"
else
  pass "Makefile check does not include pr-issue-link-check"
fi

pre_push_line="$(grep -E '^git-pre-push:' "${makefile}")"
if echo "${pre_push_line}" | grep -q 'pr-issue-link-check'; then
  pass "Makefile git-pre-push includes pr-issue-link-check"
else
  fail "Makefile git-pre-push must include pr-issue-link-check"
fi

if grep -q 'make check' "${ci_yml}"; then
  pass "CI runs make check"
else
  fail "CI workflow must run make check"
fi

if grep -qE 'CI で実行|CIで実行' "${skill}"; then
  fail "pr-workflow SKILL must not claim CI execution (CI runs make check only)"
else
  pass "pr-workflow SKILL does not claim CI execution"
fi

if grep -q 'pre-push' "${skill}"; then
  pass "pr-workflow SKILL describes pre-push execution"
else
  fail "pr-workflow SKILL must describe pre-push execution"
fi

if [[ "${failures}" -gt 0 ]]; then
  echo "DOC-CONSISTENCY: ${failures} failure(s)" >&2
  exit 1
fi
echo "DOC-CONSISTENCY: all checks passed"
