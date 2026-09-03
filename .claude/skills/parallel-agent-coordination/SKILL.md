---
name: parallel-agent-coordination
description: Use when multiple AI coding agents (Claude Code, Codex CLI, OpenCode, etc.) may work on m5stack-pc-remote at the same time, to claim work without duplicating it and to isolate git state so agents don't corrupt each other's uncommitted changes.
---

# Parallel Agent Coordination

## Goal

このリポジトリは複数のAIコーディングエージェントが同時に作業し得る。誰がどのエージェントを使うか、どの組み合わせで動くかは頻繁に変わるため、特定のエージェント名に依存しないルールにする。目的は2つ:

1. 同じIssue/作業を複数エージェントが重複して進めない。
2. あるエージェントのgit操作(branch切り替え、stash等)が、別エージェントの未commit作業を壊さない。

## 現在アクティブなエージェント

このリポジトリで動き得るエージェントの一覧。増減したらここだけ書き換える(個々のルールにエージェント名を書かない)。

- Claude Code
- Codex CLI
- OpenCode

設計/実装の役割分担(誰が設計しレビューし、誰が実装するか)は `.claude/skills/design-implementation-handoff/SKILL.md` の「現在の割り当て」節を正本にする。このスキルは「役割」ではなく「同時実行時に踏まないための手続き」を扱う。

## 実害の記録

2026-09-04、Claude CodeとOpenCodeが同じ working directory(同じ `.git`)を共有した状態で並行作業し、双方の `git switch`/`git stash` が絡み合って一方の未commit作業(OTA関連の2ファイル)が消えた。`git fsck --unreachable --dangling` から dangling commit を見つけて復旧できたが、これは運が良かっただけで保証された手段ではない。この事故を踏まえてこのスキルを作った。

## 作業開始前: Issueを確認して宣言する

1. `gh issue list --state open` と `gh pr list --json number,title,headRefName,author` で、対象Issueに既に着手しているエージェント(assignee、または関連PRの存在)がいないか確認する。
2. 既に他のエージェントのPRが存在するIssueには着手しない。別のIssueを選ぶか、ユーザーに確認する。
3. 着手する場合は次の両方を行い、他のエージェントから見えるようにする。
   - `gh issue edit <N> --add-assignee <自分のGitHubユーザー>`
   - `gh issue comment <N> --body "着手中(<エージェント名>): <作業内容の要約>"`
4. 作業を終えた(PRを出した/中断した)ら、コメントで状況を更新する。着手を取り下げる場合は `--remove-assignee` する。

これは「予約」であって強制ロックではない。数分〜数十分の作業ならこの往復のオーバーヘッドが割に合わないこともあるが、複数ファイルにまたがる実装や、他Issueとの依存がある作業では必ず行う。

## 作業中: git状態を分離する

**同時に他のエージェントが動いている可能性がある間は、共有の主working directory(このリポジトリを最初にcloneした場所)で `git switch` / `git checkout <branch>` / `git stash` を実行しない。** これらはworking directory全体に影響し、別エージェントが同じ場所で作業していると、branch切り替えのタイミングで相手の未commit変更を巻き込んだり、意図せず消したりする。

代わりに、タスクごとに専用の `git worktree` を作る(`pr-workflow` skillの「1タスク1ブランチ+1worktree」方針と同じ)。

```bash
git fetch -q origin
git worktree add /tmp/<repo>-<topic> -b <branch-name> origin/main
cd /tmp/<repo>-<topic>
```

この専用ディレクトリの中でだけ `git add` / `git commit` / `git push` / `cargo build` 等を行う。共有チェックアウト側では読み取り専用の操作(`git log`、`git status`、`gh issue/pr list`)だけにとどめる。

作業が終わったらcleanup する。

```bash
cd <repo root>
git worktree remove /tmp/<repo>-<topic>
```

## 共有チェックアウトで自分が作っていない変更を見つけたら

`git status` に自分が編集した覚えのないファイルが出た場合、それは別のエージェントの作業中の変更である可能性が高い。

- **勝手に `git checkout --`、`git restore`、`git reset --hard`、`git clean` で消さない。**
- 内容を確認し(`git diff <file>`)、自分の作業と無関係なら触らずそのままにする。
- 自分の変更を安全に確保する必要がある場合は、その変更だけを対象にした `git worktree` へ切り出すか、対象ファイルを明示指定した `git stash push -- <file...>` を使う(`git stash` を無引数で使うと他エージェントの変更まで一緒にstashしてしまう)。

## 事故が起きた場合の復旧

未commitの変更が消えたように見えても、直前に `git add`/`stash` を経由していれば、objectとしては残っている可能性がある。

```bash
git fsck --unreachable --dangling | grep "dangling commit"
git show <commit>:<path>   # 中身を確認してから復旧する
```

見つけたら、まず安全な場所(専用worktreeなど)へ復元し、その場でcommitして確定させる。commit前に再度git操作を重ねると回収の機会を失う。

## PR/Issueのひもづけ

PRを対象Issueへ確実にひもづける仕組みは `scripts/check-pr-issue-link.sh`(`make git-pre-push` 経由)が正本(Issue #96)。ブランチ名に `{type}/{issue-number}-{slug}` の形でIssue番号を含め、PR本文またはコミットに `Fixes #N` / `Refs #N` を書く。詳細は `.claude/skills/pr-workflow/SKILL.md` を参照。

このスキルからは以下だけ補足する。

- 対応するIssueが無いPR(ユーザーからの直接依頼など)は、その旨と理由をPR本文に明記する。`no-issue` labelがリポジトリに用意されている。
- 自分が出していないPRをmergeしたり、他エージェントの作業branchを書き換えたりしない。
- 複数PRが同じファイルを触っていて、片方が先にmergeされた場合、後発PRのrebase対応は後発PR側の責任にする。特別な調整ルールは設けず、通常のgit conflict解消として扱う。
