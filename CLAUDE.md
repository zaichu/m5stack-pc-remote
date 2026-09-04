# CLAUDE.md

Claude Code がこのリポジトリで作業する際のルールです。詳細な制約は [AGENTS.md](AGENTS.md) を正本とします。

## 役割分担

**誰がどの役割かは [docs/agent-roles.md](docs/agent-roles.md) が唯一の正本です。**
割り当ては頻繁に変わるため、ここには書きません。作業開始前に必ず読んでください。

役割によらず守るルール:

- 大きな技術選定、認証方式、外部操作経路、Windowsサービス化方針は設計エージェントのレビューを前提にする。
- 実装エージェントは main へ直接pushしない。自分の実装を自分でmergeしない。Windows PCの実 shutdown/reboot や外部公開設定変更を勝手に実行しない。
- 統合エージェントはPRのmerge判断とmerge実行を担当する。ただし Issue に実機確認などのmerge前提条件が書かれている場合はそれに従う。
- 設計エージェントが直接書いた実質的な差分は、実装した本人以外による逆レビュー対象にする。

## 複数エージェントの同時作業

複数のAIエージェントが同時にこのリポジトリで作業することがあります。Issueの重複着手や、git状態の衝突(あるエージェントのbranch切り替え/stashが別エージェントの未commit変更を壊す)を防ぐ手順は `.claude/skills/parallel-agent-coordination/SKILL.md` を正本にします。作業開始前に必ず参照してください。

## 品質ゲート

正本は `.claude/skills/verify/SKILL.md` と `Makefile` です。

通常は以下を実行します。

```bash
make check
```

`bash -n scripts/*.sh` は `make check` に含まれます。gitleaks / shellcheck / mingw-w64 がない環境では該当検査を警告してスキップしますが、CI（`CI=1`）ではエラーになります。ローカルで通ったものが CI で初めて落ちるのを避けるためです。

esp Rust toolchain やローカル `firmware/config.toml` がない環境では `make firmware-build` は警告だけ出してスキップします。実機作業前には esp toolchain を入れた環境で `cargo +esp build --release --target xtensa-esp32-espidf` を必ず実行してください。

## 秘密情報

以下をコード、テスト、ログ、コミットメッセージ、PR本文へ入れないでください。

- 実Wi-Fi SSID/Password
- 実PC MACアドレス
- 実LANグローバルIPや家庭内ネットワーク詳細
- HMAC shared secret
- Windows認証情報

テンプレート値は `replace-with-*` や `AA:BB:CC:DD:EE:FF` のような明確なダミーだけを使います。
