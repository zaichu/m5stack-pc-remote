#!/usr/bin/env bash
set -euo pipefail

secret_path_patterns=(
  '^\.env$'
  '^\.env\.[^/]+$'
  '^firmware/config\.toml$'
  '^m5stack-pc-bridge/config\.toml$'
  '(^|/)[^/]*secret[^/]*\.json$'
  '(^|/)[^/]*credential[^/]*\.json$'
  '(^|/)[^/]*service-account[^/]*\.json$'
  '(^|/)[^/]*\.pem$'
  '(^|/)[^/]*\.key$'
  '(^|/)[^/]*\.bin$'
)

blocked=()
while IFS= read -r path; do
  [[ -z "$path" ]] && continue
  for pattern in "${secret_path_patterns[@]}"; do
    if [[ "$path" =~ $pattern ]]; then
      blocked+=("$path")
      break
    fi
  done
done < <(git diff --cached --name-only --diff-filter=ACMR)

if [[ ${#blocked[@]} -gt 0 ]]; then
  echo "ERROR: commit blocked -- staged path looks like a secret file:" >&2
  for path in "${blocked[@]}"; do
    echo "  $path" >&2
  done
  exit 1
fi
