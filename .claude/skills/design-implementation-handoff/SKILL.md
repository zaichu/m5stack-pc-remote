---
name: design-implementation-handoff
description: Use when handing a task between the design/review agent and the implementation agent in m5stack-pc-remote, in either direction.
---

# Design / Implementation Handoff

## Goal

このプロジェクトの標準運用は「実装を担当しないエージェントが設計・レビュー・リリース判断を持つ」です。特定のモデル名(Codex/Claude等)に固定しない。使うツールが変わっても、このスキルの手順とチェックリストだけを差し替えれば運用を継続できるようにする。

## 現在の割り当て

- **実装エージェント**: Claude Code
- **設計・レビューエージェント**: Codex CLI

割り当てが変わったら、このセクションだけ書き換える。役割の呼び方(実装エージェント/設計エージェント)は変えない。CLAUDE.md・AGENTS.mdの役割分担ルールも、割り当て先の固有名詞ではなくこの役割名で書いてある。

## 実装エージェントが設計エージェントへ依頼するとき

余計な往復なしに設計エージェントが判断できるだけの文脈を渡す。

現在の割り当てでの起動コマンド(Claude -> Codex):

```bash
codex exec -s read-only --skip-git-repo-check -o <file> "<request>" < /dev/null
```

設計判断そのもの(実装ではない)を依頼する場合は `-s read-only` のままでよい。

### Request Checklist

1. `AGENTS.md` と `CLAUDE.md` を読むように依頼する。
2. ユーザー要求、フェーズ、非ゴールを明示する。
3. 変更対象ファイルと境界を指定する。
4. 実Wi-Fi情報、実MAC、HMAC secret、Windows認証情報を使わないよう明記する。
5. 受入条件と検証コマンドを書く。
6. 日本語で、変更ファイル、検証、残リスクを報告するよう依頼する。

プロンプトに Markdown fence、backtick、`$()` が含まれる場合は `/tmp` に本文を書き、ファイル経由で渡してshell展開を避ける。

## 設計エージェントが実装エージェントへ依頼するとき

現在の割り当てでの起動コマンド(Codex -> Claude)。

初回:

```bash
claude -p "<request>" --permission-mode acceptEdits --allowedTools Bash Edit Write Read Glob Grep
```

同じ作業の継続:

```bash
claude -c -p "<request>" --permission-mode acceptEdits --allowedTools Bash Edit Write Read Glob Grep
```

依頼内容は上と同じチェックリスト(AGENTS.md/CLAUDE.mdを読ませる、要求・フェーズ・非ゴール・対象ファイル・secret方針・受入条件・報告形式)を満たす。

## 設計エージェントが直接コードを書いた場合の逆レビュー

設計エージェントが実質的な差分(コード、テスト、スクリプト、ドキュメント)を直接書いた場合、merge前に実装エージェントへ逆レビューを依頼する。

1. 実diffをレビュー対象にする。
2. 設計意図、既知の懸念、実行済み品質コマンドを渡す。
3. 認証、secret handling、実ネットワーク混入、Windows実電源操作の誤実行、docs更新漏れを重点確認させる。
4. findings first、日本語、可能ならファイル参照付きで返答させる。
5. 実装エージェントが使えない場合は、試行結果と残リスクをPR本文に書く。
