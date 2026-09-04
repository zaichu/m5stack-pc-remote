---
name: design-implementation-handoff
description: Use when handing a task between agents in m5stack-pc-remote, in either direction, or when reviewing what another agent produced.
---

# Design / Implementation Handoff

## Goal

このプロジェクトの標準運用は「実装を担当しないエージェントが設計・レビュー・リリース
判断を持つ」です。特定の製品名には固定しません。

**誰がどの役割かは `docs/agent-roles.md` が唯一の正本です。**
起動コマンドと依頼チェックリストもそこにあります。担当が変わったときに
書き換えるのはあのファイルだけで、このスキルは書き換えません。

このスキルには、担当が変わっても変わらない手順だけを置きます。

## 依頼するとき

`docs/agent-roles.md` の「起動コマンド」と「依頼するときのチェックリスト」に従います。

特に外しやすいのは次の2つです。

- **触ってよいファイルの明示。** 複数エージェントへ同時に依頼するとき、共有ファイル
  (`Makefile`、CI設定、共通crate)を両者の範囲に残すと同じものが二重に作られます。
- **実機由来の制約を渡すこと、かつその制約が正しいこと。** 渡さなければ動かない
  コードが書かれ、間違ったものを渡せば間違った前提がコメントとして固定されます。

## 受け取ったものをレビューするとき

`docs/agent-roles.md` の「受け取った成果物の扱い」に従います。要点は
**報告ではなく実 diff を読む**ことです。

順序:

1. `git show <commit> --stat` で変更範囲を把握する。報告と一致するか確認する。
2. セキュリティに関わる差分(認証、署名、secret handling、外部通信、電源操作)から読む。
3. テストが追加されていれば、**修正を外してそのテストが落ちることを確認する**。
   落ちなければ、そのテストは何も守っていません。
4. `make check` を自分で回す。報告の「全部通りました」を根拠にしない。
5. 見つけた問題は、実装した本人へ差し戻すか、自分で直して理由をPR本文に書く。

## 逆レビュー

設計エージェントや統合エージェントが実質的な差分(コード、テスト、スクリプト、
ドキュメント)を直接書いた場合も、merge前に**実装した本人以外**のレビューを通します。
それができない場合は、試行結果と残リスクをPR本文に書きます。

## worktree を使う

エージェントごとに専用の worktree を用意します。共有 working directory で
並行作業すると、`git switch` / `git stash` が絡み合って未commitの作業が消えます
(実害の記録は `parallel-agent-coordination` skill 参照)。

```bash
git worktree add -b {type}/{issue-number}-{slug} /tmp/<repo>-<topic> origin/main
```
