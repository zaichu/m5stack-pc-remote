#!/usr/bin/env bash
set -euo pipefail

if command -v gitleaks >/dev/null 2>&1; then
  gitleaks detect --no-banner --redact --source .
  exit 0
fi

echo "WARNING: gitleaks not found; using fallback rg-based secret scan." >&2

tracked_files=()
while IFS= read -r path; do
  case "${path}" in
    firmware/src/telegram_root_ca.h|scripts/scan-secrets.sh|.claude/skills/verify/SKILL.md)
      continue
      ;;
  esac
  tracked_files+=("${path}")
done < <(git ls-files)

if [[ ${#tracked_files[@]} -eq 0 ]]; then
  exit 0
fi

if rg -n -- \
  '-----BEGIN|AIza|ghp_|ghs_|xox[baprs]-|[0-9]{6,}:[A-Za-z0-9_-]{20,}|https://discord\.com/api/webhooks/[0-9]+/[A-Za-z0-9_-]+' \
  "${tracked_files[@]}"; then
  echo "ERROR: fallback secret scan found suspicious content." >&2
  exit 1
fi
