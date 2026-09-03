#!/usr/bin/env bash
# PR と Issue の紐付けを簡易チェックする。
# - ブランチ名に issue 番号（例: refactor/91-bridge...）が含まれること
# - PR本文またはコミットメッセージに Fixes #N / Refs #N が含まれること
# ローカル（pre-push）と CI の両方で使う。network が無い環境ではスキップする。
set -euo pipefail

branch="$(git branch --show-current 2>/dev/null || true)"
if [[ "$branch" == "main" ]]; then
  exit 0
fi

# ブランチ名から issue 番号を抽出
issue="$(echo "$branch" | grep -oE '[0-9]+' | head -1 || true)"
if [[ -z "$issue" ]]; then
  echo "ERROR: ブランチ名 '$branch' に issue 番号が含まれていません。例: refactor/91-bridge-config-validation" >&2
  exit 1
fi

# PR本文またはコミットメッセージに Fixes/Refs が含まれるか
# コミットがまだ無い（ブランチ作成直後）は、PR本文がまだ無いのが正常なので
# ブランチ名のチェックだけで通す。
if ! git log --oneline "main..HEAD" 2>/dev/null | grep -q .; then
  exit 0
fi

# 1) ローカルのコミットメッセージを優先して見る
if git log --format=%B "main..HEAD" 2>/dev/null | grep -qE "(Fixes|Refs) #$issue"; then
  exit 0
fi

# 2) PRが既に存在すれば本文を見る（gh が無い/cached 失敗ならスキップ）
if command -v gh >/dev/null 2>&1; then
  if body="$(gh pr view --json body --jq .body 2>/dev/null)"; then
    if echo "$body" | grep -qE "(Fixes|Refs) #$issue"; then
      exit 0
    fi
  fi
fi

echo "ERROR: PR本文またはコミットに 'Fixes #$issue' / 'Refs #$issue' が見つかりません。" >&2
echo "  ブランチ: $branch" >&2
echo "  期待: Fixes #$issue または Refs #$issue を PR本文末尾に記載してください。" >&2
echo "  参考: .claude/skills/pr-workflow/SKILL.md の Issue Close Keywords を参照" >&2
exit 1
