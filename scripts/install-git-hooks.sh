#!/usr/bin/env bash
set -euo pipefail

if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "ERROR: not inside a Git repository." >&2
  exit 1
fi

repo_root="$(git rev-parse --show-toplevel)"
hooks_dir="$repo_root/.githooks"

if [[ ! -d "$hooks_dir" ]]; then
  echo "ERROR: $hooks_dir not found." >&2
  exit 1
fi

not_executable=()
for hook in "$hooks_dir"/*; do
  [[ -f "$hook" ]] || continue
  if [[ ! -x "$hook" ]]; then
    not_executable+=("$hook")
  fi
done

if [[ ${#not_executable[@]} -gt 0 ]]; then
  echo "ERROR: hook file(s) are not executable:" >&2
  for hook in "${not_executable[@]}"; do
    echo "  $hook" >&2
  done
  echo "Run: chmod +x ${hooks_dir}/*" >&2
  exit 1
fi

git config core.hooksPath .githooks
echo "git hooks installed: core.hooksPath=.githooks"
