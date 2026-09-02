#!/usr/bin/env bash
set -euo pipefail

secret_path_patterns=(
  '^\.env$'
  '^\.env\.[^/]+$'
  '^windows-agent/config\.toml$'
  '(^|/)[^/]*secret[^/]*\.json$'
  '(^|/)[^/]*credential[^/]*\.json$'
  '(^|/)[^/]*service-account[^/]*\.json$'
  '(^|/)[^/]*\.pem$'
  '(^|/)[^/]*\.key$'
)

allowed_templates=(
  ".env.example"
  ".env.local.example"
  "windows-agent/config.example.toml"
)

is_allowed_template() {
  local path="$1"
  local allowed
  for allowed in "${allowed_templates[@]}"; do
    if [[ "$path" == "$allowed" ]]; then
      return 0
    fi
  done
  return 1
}

blocked=()
while IFS= read -r path; do
  [[ -z "$path" ]] && continue
  if is_allowed_template "$path"; then
    continue
  fi
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
