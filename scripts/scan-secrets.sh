#!/usr/bin/env bash
set -euo pipefail

if command -v gitleaks >/dev/null 2>&1; then
  gitleaks detect --no-banner --redact --source .
  exit 0
fi

echo "WARNING: gitleaks not found; using fallback pattern scan." >&2

# rg is not always installed (it is absent from the GitHub Actions runner and
# from a plain Debian/WSL setup). Falling through silently would have made the
# whole gate a no-op, so pick grep -E when rg is missing.
if command -v rg >/dev/null 2>&1; then
  scan() { rg -n -- "$1" "${@:2}"; }
else
  echo "WARNING: rg not found; using grep -E instead." >&2
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
  echo "ERROR: fallback secret scan found suspicious content." >&2
  exit 1
fi
