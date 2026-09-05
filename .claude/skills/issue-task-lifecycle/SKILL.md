---
name: issue-task-lifecycle
description: |
  Issue とプロジェクト内の一時 task file を使い分ける。Use when: 新しい作業を起こす時、task file を作るか迷う時、task file が残っている/消えた/古い時、Issue に残すべき判断・原因・残件がある時、PR マージ前後の記録整理を行う時。
---

# Issue / Task Lifecycle

## 基本方針

- Issue は永続記録の正本とする。
- task file は作業中だけのローカル一時メモとする。
- PR は実装差分、検証結果、レビュー対応の記録とする。
- 後から読み返す価値がある原因、判断、仕様、残件は task file だけに置かない。

## Issue を作る条件

次のいずれかに当てはまる場合は Issue を作る、または既存 Issue に追記する。

- 本番障害、CI/CD 失敗、デプロイ失敗、ユーザー影響がある不具合。
- 原因、判断、再発防止を後から追う必要がある。
- 作業が複数 PR、複数日、複数 agent にまたがる。
- 「別タスクで検討する」と判断した。
- task file を削除すると経緯が消える。

小さい typo、単一ファイルの自明な修正、作業中のチェックリストだけで足りるものは Issue 不要。

### 検討・分析・RFC はファイル化せず Issue 本文に書く

実装を伴わない検討・分析・提案・調査結果（アーキテクチャ検討、CI/CD 改善案など）は、
`docs/**` に新規ファイルを作って PR で merge しない。Issue 本文（`gh issue create/edit
--body-file`）に全文を書く。

理由: ファイル化すると PR レビュー・merge のコストがかかる上、内容が実装により陳腐化
しても誰も更新しない「化石ドキュメント」になる。かつ同じ内容が PR と Issue の二箇所に
分散し、どちらが正本か曖昧になる。

ファイル + PR が妥当なのは、コード・設定自体が成果物になる場合のみ（「記録の置き場所」参照）。

### 複数の作業単位にまたがる場合は sub-issue に分割する

優先順位付きタスク（P1/P2/P3...）や複数の改善項目を 1 つの Issue にチェックリストで
まとめない。全体像を示す親 Issue を 1 つ作り、実際に着手・完了・クローズできる単位ごとに
子 Issue を作って GitHub の sub-issue（`gh api graphql` の `addSubIssue` mutation）で
紐付ける。

```bash
PARENT_ID=$(gh api graphql -f query='query{repository(owner:"OWNER",name:"REPO"){issue(number:PARENT_NUM){id}}}' --jq '.data.repository.issue.id')
CHILD_ID=$(gh api graphql -f query='query{repository(owner:"OWNER",name:"REPO"){issue(number:CHILD_NUM){id}}}' --jq '.data.repository.issue.id')
gh api graphql -f query="mutation{addSubIssue(input:{issueId:\"$PARENT_ID\",subIssueId:\"$CHILD_ID\"}){issue{number}}}"
```

親 Issue の本文は「結論 + sub-issue 一覧 + 未確認事項」に留め、各子 Issue が単体で
受け入れ条件・優先度を持つようにする。`gh issue view <parent> --json subIssuesSummary`
で進捗（完了数/全体数）を確認できる。

## task file を作る条件

task file はプロジェクトルールが指定する場所に作る。指定がなければ `docs/tasks/<branch-name>.md` を使う。

- Claude に実装を委譲する。
- 専用 worktree で中規模以上の作業を進める。
- 受け入れ条件、非対象、確認コマンドを固定したい。
- レビュー指摘を Claude に再依頼する必要がある。

task file には Issue/PR 番号がある場合は必ず書く。完了または中止時に削除し、コミットしない。

## 記録の置き場所

- 仕様、原因、判断、残件: Issue
- 実装内容、検証結果、レビュー対応: PR
- Claude 依頼文、受け入れ条件、作業中チェック: task file
- 一時ログ、コメント下書き: `/tmp`。完了時に削除する。

## 終了時チェック

作業を閉じる前に確認する。

- Issue が必要な内容は Issue/PR に移した。
- task file は削除した。
- PR コメントは実改行で投稿した。`\n` を文字列として表示させない。
- マージ済み worktree、local branch、remote branch を削除した。
- `main` を `git pull --ff-only origin main` で更新した。
