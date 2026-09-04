#!/usr/bin/env bash
set -euo pipefail

if command -v gitleaks >/dev/null 2>&1; then
  # 設定は .gitleaks.toml を自動で読む。--redact で検出値そのものは出力しない。
  gitleaks detect --no-banner --redact --source .
  exit 0
fi

# fallback は既知パターンしか見ないため、gitleaks の代わりにはならない。
# CIでは必ず gitleaks を通す。skipを成功扱いにすると、ローカルで通ったものが
# 誰にも検査されないまま main へ入る。
if [[ -n "${CI:-}" ]]; then
  echo "ERROR: CIでは gitleaks が必須です。" >&2
  exit 1
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
    firmware/src/telegram_root_ca.rs|scripts/scan-secrets.sh|.claude/skills/verify/SKILL.md)
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
