---
name: pr-workflow
description: Use when preparing, reviewing, merging, or cleaning up an m5stack-pc-remote task branch/PR.
---

# PR Workflow

## Branch Rules

- mainへ直接pushしない。
- 作業branchは短命にする。
- unrelated changesを混ぜない。
- ブランチ名は `{type}/{issue-number}-{slug}` とし、Issue 番号を必ず含める（例: `refactor/91-bridge-config-validation`）。`scripts/check-pr-issue-link.sh` が検証する。
- PR本文は日本語で、概要、変更内容、検証、残リスクを書く。

## Issue Close Keywords

- `Fixes #N` は、そのPRのmergeだけでIssueの受入条件がすべて満たされる場合だけ使う。
- 実機確認、Windowsサービス登録、外部操作のライブ確認が残る場合は `Refs #N` を使う。
- PR本文またはコミットメッセージに `Fixes #N` / `Refs #N` が含まれることを `scripts/check-pr-issue-link.sh` が検証する。`make check` や pre-push、CI で実行される。

## Issue の明示的マーキング（競合防止）

- 着手時に `gh issue edit N --add-assignee @me` と `gh issue comment N --body "着手します PR #xx"` を必ず実行し、誰がどの Issue を担当しているかを可視化する。
- ブランチと Issue は 1:1 で対応させ、別 Issue の差分を混ぜない。並行する Claude Code との競合は、ブランチ名と Assignee で検出する。

## 設計エージェントが書いたDiff

設計エージェント(現在の割り当ては `.claude/skills/design-implementation-handoff/SKILL.md` を参照)が実質的な差分を直接書いた場合、merge前に同スキルの逆レビューを行います。実装エージェントが使えない場合は、PR本文に試行結果と残リスクを書きます。
