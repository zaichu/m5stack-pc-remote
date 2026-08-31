---
name: codex-claude-handoff
description: Use when Codex hands implementation, review fixes, or reverse review to Claude in m5stack-pc-remote.
---

# Codex Claude Handoff

## Goal

Claudeが余計な往復なしに実装できるだけの文脈を渡し、Codexが設計・レビュー・リリース判断を持つ。

## Invocation

初回:

```bash
claude -p "<request>" --permission-mode acceptEdits --allowedTools Bash Edit Write Read Glob Grep
```

同じ作業の継続:

```bash
claude -c -p "<request>" --permission-mode acceptEdits --allowedTools Bash Edit Write Read Glob Grep
```

プロンプトに Markdown fence、backtick、`$()` が含まれる場合は `/tmp` に本文を書き、`--body-file` 相当の扱いにしてshell展開を避けます。

## Request Checklist

1. `AGENTS.md` と `CLAUDE.md` を読むように依頼する。
2. ユーザー要求、フェーズ、非ゴールを明示する。
3. 変更対象ファイルと境界を指定する。
4. 実Wi-Fi情報、実MAC、HMAC secret、Windows認証情報を使わないよう明記する。
5. 受入条件と検証コマンドを書く。
6. 日本語で、変更ファイル、検証、残リスクを報告するよう依頼する。

## Codex-authored Diff Review

Codexが直接差分を書いた場合:

1. 実diffをレビュー対象にする。
2. 設計意図、既知の懸念、実行済み品質コマンドを渡す。
3. 認証、secret handling、実ネットワーク混入、Windows実電源操作の誤実行、docs更新漏れを重点確認させる。
4. findings first、日本語、可能ならファイル参照付きで返答させる。
