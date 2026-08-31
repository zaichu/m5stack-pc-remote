---
name: git-pushing
description: Use when staging, committing, pushing, or opening a PR for m5stack-pc-remote.
---

# Git Pushing

## Rules

- mainへ直接pushしない。
- 1タスク1短期branchを使う。
- `git add .` と `git add -A` は使わない。
- stageは明示パス指定または `git add -p` にする。
- commit message、PR title、PR bodyは日本語にする。

## Workflow

```bash
git status --short --branch
git diff --check
git diff -- <paths>
git add <explicit paths>
git diff --cached --stat
git commit -m "<種別>: <日本語の要約>"
git push -u origin "$(git branch --show-current)"
```

PR作成前に `.claude/skills/verify/SKILL.md` を実行します。
