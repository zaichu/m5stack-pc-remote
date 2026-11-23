#!/usr/bin/env bash
# SHARED_CRATES(Makefile)へ入れ忘れた shared/* があれば落とす。
#
# 背景: shared の crate を足すたび Makefile の複数箇所・CI・Cargo.toml を
# 触る必要があり、fmt/clippy/キャッシュからの漏れが後追いで見つかっていた
# (例: telegram-cursor の fmt/clippy 漏れ)。一覧は Makefile の SHARED_CRATES
# 1か所が正本で、CI のキャッシュ対象も `make print-workspaces` から読む。
#
# standalone で実行できる: `bash scripts/check-crate-list.sh`
# `REPO_ROOT` を上書きすれば別ツリーに対しても実行できる。
set -euo pipefail

repo_root="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
cd "$repo_root"

listed="$(make -s --no-print-directory print-shared-crates)"
actual="$(for d in shared/*/; do
  name="${d%/}"
  name="${name#shared/}"
  [[ -f "shared/${name}/Cargo.toml" ]] || continue
  echo "$name"
done | LC_ALL=C sort)"

failures=0

while IFS= read -r name; do
  [[ -z "$name" ]] && continue
  if ! printf '%s\n' "$listed" | grep -qxF "$name"; then
    echo "CRATE-LIST FAIL: shared/${name} が Makefile の SHARED_CRATES にありません。" >&2
    failures=$((failures + 1))
  fi
done <<< "$actual"

while IFS= read -r name; do
  [[ -z "$name" ]] && continue
  if [[ ! -f "shared/${name}/Cargo.toml" ]]; then
    echo "CRATE-LIST FAIL: SHARED_CRATES の ${name} に対応する shared/${name}/Cargo.toml がありません。" >&2
    failures=$((failures + 1))
  fi
done <<< "$listed"

if [[ "$failures" -gt 0 ]]; then
  echo "CRATE-LIST FAIL: ${failures} 件の不整合があります。" >&2
  exit 1
fi

echo "CRATE-LIST OK: $(printf '%s\n' "$listed" | grep -c .) crates"
