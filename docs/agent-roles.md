# エージェント役割の割り当て

**このファイルが、どのAIエージェントがどの役割を持つかの唯一の正本です。**

割り当ては頻繁に変わります。変わったら「現在の割り当て」の表と「起動コマンド」だけを
書き換えてください。他のドキュメントやスキルは役割名だけで書いてあるので、
書き換える必要はありません。

`scripts/check-agent-roles.sh` が、このファイル以外に割り当てが書かれていないかを
検査します(`make check` に含まれます)。他所へ割り当てを書くと CI が落ちます。
過去に CLAUDE.md・AGENTS.md・docs/architecture.md の3か所へ割り当てが複製され、
片方だけ古くなって実態とずれたため、この検査を入れました。

## 役割

役割名は固定です。担当が変わっても呼び方は変えません。

| 役割 | 責務 |
|---|---|
| 実装エージェント | 実装、テスト、ドキュメント更新、明示パスでの staging、作業branchへのpush、PR作成 |
| 設計エージェント | 技術選定、認証方式、外部操作経路、Windowsサービス化方針の決定とレビュー |
| 統合エージェント | 実装のレビュー、PRのmerge判断とmerge実行、Issueの優先順位付け、エージェント間の作業分割 |

設計と統合は同じエージェントが兼ねてよいですが、**実装エージェントは自分の実装を
自分で承認しません**。実装した本人以外がレビューと merge 判断を持ちます。

## 現在の割り当て

| 役割 | 担当 |
|---|---|
| 実装エージェント | OpenCode (`opencode/muse-spark-1.3-contributor-free`)、Codex CLI |
| 設計エージェント | Claude Code |
| 統合エージェント | Claude Code |

実装エージェントは2人で、**提供元が別なのでレート制限を共有しません**。片方が使えないときは
もう片方へ振り、作業ディレクトリ(worktree)とファイルの担当を分ければ並行もできます。

Codex CLI のsandbox(`workspace-write`)は既定でネットワークが使えないため、Codex は
**実装とテストまで**を担当し、`git push`・PR作成・マージは統合エージェントが行います。

## 起動コマンド

他のエージェントへ作業を渡すときに使います。担当が変わったらここも書き換えます。

OpenCode:

```bash
bash scripts/run-agent.sh \
  --model opencode/muse-spark-1.3-contributor-free \
  --dir <worktree> \
  --prompt-file <file>
```

モデルは必ず `--model` で明示します。省略すると既定モデルで動き、結果が期待と違ったとき
**どのモデルの出力なのか後から特定できません**。

`opencode run` を直接叩かず、必ずこのラッパーを通してください。素の `opencode run` は
タイムアウトを持たず、上流のレート制限で再試行が固まると**無限に待ちます**。
`run-agent.sh` はログが伸びなくなったことを検知して数分で打ち切り、原因のエラー行を
表示します。終了コード 2 が「停止検知による打ち切り」です。

### 大きな作業は1回のプロンプトへ詰め込まない

`--continue` でターンを分け、**対話的に進めてください。**

```bash
# 1ターン目: 調査と方針だけ
bash scripts/run-agent.sh --model <model> --dir <worktree> --prompt-file step1.md
# 内容を確認してから
# 2ターン目以降: 実装 → テスト → 検証、と刻む
bash scripts/run-agent.sh --model <model> --dir <worktree> --prompt-file step2.md --continue
```

一括で渡すと、エージェント内部のループが長くなり(実測で `step=60` まで到達)、
リクエストが肥大して上流のレート制限を踏みやすくなります。

刻む利点は他にもあります。

- **途中経過が見える。** 一括だと完了まで出力が一切返らない(実際に5時間ゼロだった)
- **途中で軌道修正できる。** 方針が違ったときに、終わってから300行を読み直さずに済む
- **ハングしても1ターン分しか失わない**

### 並列で走らせすぎない

**同時に走らせるのは1本まで**にしてください。2026-09-05 に3本を並列で回したところ、
プロバイダのレート制限(`rate_limit_exceeded`、ログに185回)を誘発し、
2本が5時間ハングして成果0で終わりました。速く終わらせるつもりが逆効果になります。

### 進捗の確認

「プロセスが生きている」と「進んでいる」は別です。上の事故では `ps` 上は正常に見え、
CPUも少し使っていました。確認するのは次の3つです。

- `~/.local/share/opencode/log/opencode.log` の更新時刻
- worktree のファイル変更 (`git status --short`)
- CPU時間 (`ps -o etime,time`) — 経過時間に対して極端に小さければ待機している

Codex CLI:

```bash
codex exec -s read-only --skip-git-repo-check -o <file> "<request>" < /dev/null
```

Codex CLI(実装を任せるとき。作業はworktreeで行い、`-C` で指定する):

```bash
codex exec -s workspace-write --skip-git-repo-check -C <worktree> \
  -o <最終報告の出力先> "$(cat <prompt-file>)" < /dev/null
```

疎通確認は `-s read-only` で1行だけ投げて、返るかを見る(OpenCodeと同じ)。

Claude Code:

```bash
claude -p "<request>" --permission-mode acceptEdits --allowedTools Bash Edit Write Read Glob Grep
```

## Issueの振り分けの補助(Jev)

`scripts/triage-issue.sh <issue番号>` が、Issueの規模・リスク・担当候補・実機確認の要否を
Jev(TypeSafe AIのSystem One Model)で判定する。**結果は提案で、決定ではない。**

```bash
export TYPESAFE_API_KEY=<TypeSafe AIのAPIキー>   # ~/.bashrc などに置く。Gitへ入れない
bash scripts/triage-issue.sh 196
```

- 確信度が0.5未満の項目があるときは警告が出る。**モデルが迷っている合図**なので人間が判断する。
- `TYPESAFE_API_KEY` 未設定・API障害・想定外の応答では終了コード2で終わり、**他の作業には影響しない**。
- 送るのはIssueのタイトルと本文だけ。secretや家庭内の情報は送らない。
- **firmware / m5stack-pc-bridge には組み込まない**(Issue #196)。ESP32はTLSを同時1本しか張れず
  (Issue #127)、ハンドシェイクだけで3.1秒かかる(Issue #163)。電源操作を外部クラウドに依存させない。
- 外部依存のため `make check` には入れない。

## 依頼するときのチェックリスト

役割によらず共通です。

1. `AGENTS.md` と `CLAUDE.md` を読むように依頼する。
2. ユーザー要求、フェーズ、非ゴールを明示する。
3. **触ってよいファイルと触ってはいけないファイルを明示する。**
   複数のエージェントへ同時に依頼する場合は特に重要です。
   `Makefile` のような共有ファイルを両者の範囲に残すと、同じ目的のターゲットが
   二重に作られます(実例: 2026-09-04、`bash-n-check` と `shell-syntax-check` が
   別々に作られ、統合が必要になった)。
4. 実Wi-Fi情報、実MAC、HMAC secret、Windows認証情報を使わないよう明記する。
5. **そのエージェントが知り得ない実機由来の制約を渡す。**
   例: ESP32のmbedTLSヒープでは実質同時に1本のTLS接続しか張れない。
   渡さないと、動かないコードを正しいと信じて書きます。
   逆に、**渡す制約が正しいことを確認してから渡してください**
   (実例: 2026-09-04、bridge接続がplain HTTPなのにTLS制約だと伝え、
   誤った説明がそのままコード内コメントへ転写された)。
6. 受入条件と検証コマンドを書く。
7. 日本語で、変更ファイル、検証、残リスクを報告するよう依頼する。

プロンプトに Markdown fence、backtick、`$()` が含まれる場合はファイルへ書き、
ファイル経由で渡して shell 展開を避けます。

## 受け取った成果物の扱い

**報告を信用せず、実際の diff を読んでください。** 報告に書かれた変更が
入っていなかった実例、報告されていない副作用があった実例の両方があります。

- 実 diff をレビュー対象にする
- 認証、secret handling、実ネットワーク混入、実電源操作の誤実行、docs更新漏れを重点確認する
- テストが追加されている場合、**修正を外すとそのテストが本当に落ちることを確認する**
- 品質ゲート(`make check`)は報告を鵜呑みにせず自分で回す
