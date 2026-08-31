---
name: pr-workflow
description: Use when preparing, reviewing, merging, or cleaning up an m5stack-pc-remote task branch/PR.
---

# PR Workflow

## Branch Rules

- mainへ直接pushしない。
- 作業branchは短命にする。
- unrelated changesを混ぜない。
- PR本文は日本語で、概要、変更内容、検証、残リスクを書く。

## Issue Close Keywords

- `Fixes #N` は、そのPRのmergeだけでIssueの受入条件がすべて満たされる場合だけ使う。
- 実機確認、Windowsサービス登録、外部操作のライブ確認が残る場合は `Refs #N` を使う。

## Codex-authored Diff

Codexが実質的な差分を直接書いた場合、merge前に `.claude/skills/codex-claude-handoff/SKILL.md` の逆レビューを行います。Claudeが使えない場合は、PR本文に試行結果と残リスクを書きます。
