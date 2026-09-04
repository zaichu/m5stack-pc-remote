#!/usr/bin/env bash
# PR と Issue の紐付けを簡易チェックする。
# - ブランチ名に issue 番号（例: refactor/91-bridge...）が含まれること
# - PR本文またはコミットメッセージに Fixes #N / Refs #N が含まれること
# ローカル（pre-push）と CI の両方で使う。network が無い環境ではスキップする。
#
# 対応するIssueが無いPR(ユーザーからの直接依頼等)向けの逃げ道:
# - ブランチ名を `no-issue/<slug>` にする(番号チェックの対象外)
# - PRに `no-issue` labelを付ける(PR作成後、CI側のgh呼び出しでのみ検出できる。
#   pre-pushの1回目はPRがまだ無いため、まずブランチ名で通すしかない)
set -euo pipefail

# 比較基準。ローカルの main は古いことがあり、その場合 `main..HEAD` へ
# マージ済みコミットが混入して、Issue参照を持たないブランチが他人のコミットの
# `Refs #N` で誤って通ってしまう。origin/main があればそちらを使う。
if git rev-parse --verify --quiet origin/main >/dev/null; then
  base="origin/main"
else
  base="main"
fi

branch="$(git branch --show-current 2>/dev/null || true)"
if [[ "$branch" == "main" ]]; then
  exit 0
fi

if [[ "$branch" == no-issue/* ]]; then
  exit 0
fi

if command -v gh >/dev/null 2>&1; then
  if labels_json="$(gh pr view --json labels --jq '.labels[].name' 2>/dev/null)"; then
    if echo "$labels_json" | grep -qx "no-issue"; then
      exit 0
    fi
  fi
fi

# ブランチ名から issue 番号を抽出
issue="$(echo "$branch" | grep -oE '[0-9]+' | head -1 || true)"
if [[ -z "$issue" ]]; then
  # ブランチ名に番号が無くても、PR本文やコミットで実際にIssueへ紐づいていれば
  # 目的は満たされている。ブランチ命名は手段であって要件ではない。
  #
  # 既にPRが開いているブランチは後からリネームできない(GitHubのbranch rename
  # APIはhead branchを付け替えず、PRをcloseしてしまう。実際にPR #82で踏んだ)。
  # 命名だけを理由に、正しく紐づいているPRの更新を止めない。
  if git log --format=%B "$base..HEAD" 2>/dev/null | grep -qE "(Fixes|Refs) #[0-9]+"; then
    exit 0
  fi
  if command -v gh >/dev/null 2>&1; then
    if body="$(gh pr view --json body --jq .body 2>/dev/null)"; then
      if echo "$body" | grep -qE "(Fixes|Refs) #[0-9]+"; then
        exit 0
      fi
    fi
  fi
  echo "ERROR: ブランチ名 '$branch' に issue 番号が無く、PR本文/コミットにも Issue参照がありません。" >&2
  echo "  ブランチ名を '{type}/{issue-number}-{slug}' にするか、PR本文へ 'Fixes #N' / 'Refs #N' を書いてください。" >&2
  echo "  対応するIssueが無い場合は、ブランチ名を 'no-issue/<slug>' にするか、PRに 'no-issue' labelを付けてください。" >&2
  exit 1
fi

# PR本文またはコミットメッセージに Fixes/Refs が含まれるか
# コミットがまだ無い（ブランチ作成直後）は、PR本文がまだ無いのが正常なので
# ブランチ名のチェックだけで通す。
if ! git log --oneline "$base..HEAD" 2>/dev/null | grep -q .; then
  exit 0
fi

# 1) ローカルのコミットメッセージを優先して見る
if git log --format=%B "$base..HEAD" 2>/dev/null | grep -qE "(Fixes|Refs) #$issue"; then
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
