#!/usr/bin/env bash
set -euo pipefail

if command -v gitleaks >/dev/null 2>&1; then
  gitleaks detect --no-banner --redact --source .
  exit 0
fi

echo "WARNING: gitleaks が見つからないため、fallback pattern scanを使います。" >&2

# rgが無い環境でも検査を無効化しないため、無ければgrep -Eへfallbackする。
if command -v rg >/dev/null 2>&1; then
  scan() { rg -n -- "$1" "${@:2}"; }
else
  echo "WARNING: rg が見つからないため、grep -Eを使います。" >&2
  scan() { grep -n -E -- "$1" "${@:2}"; }
fi

tracked_files=()
while IFS= read -r path; do
  [[ -f "$path" ]] || continue
  case "${path}" in
    firmware/src/telegram_root_ca.h|firmware-rust-poc/src/telegram_root_ca.rs|scripts/scan-secrets.sh|.claude/skills/verify/SKILL.md)
      continue
      ;;
  esac
  tracked_files+=("${path}")
done < <(git ls-files)

if [[ ${#tracked_files[@]} -eq 0 ]]; then
  exit 0
fi

if scan \
  '-----BEGIN|AIza|ghp_|ghs_|xox[baprs]-|[0-9]{6,}:[A-Za-z0-9_-]{20,}|https://discord\.com/api/webhooks/[0-9]+/[A-Za-z0-9_-]+' \
  "${tracked_files[@]}"; then
  echo "ERROR: fallback secret scanが疑わしい内容を検出しました。" >&2
  exit 1
fi
