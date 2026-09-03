# CLAUDE.md

Claude Code がこのリポジトリで作業する際のルールです。詳細な制約は [AGENTS.md](AGENTS.md) を正本とします。

## 役割分担

このプロジェクトの標準運用は「実装を担当しないエージェントが設計・レビュー・リリース判断を持つ」です。特定のモデル名には固定しません。現在の割り当ては `.claude/skills/design-implementation-handoff/SKILL.md` の「現在の割り当て」節が正本で、今は Claude Code が実装エージェント、Codex CLI が設計・レビューエージェントです。割り当てが変わったらそのスキルの節だけ書き換えます。

- 大きな技術選定、認証方式、外部操作経路、Windowsサービス化方針は設計エージェントのレビューを前提にする。
- 実装エージェント(現在: Claude)は実装、テスト、ドキュメント更新、明示パスでの staging、作業branchへのpush、PR作成を担当する。
- 実装エージェントは main へ直接pushしない。PRをmergeしない。Windows PCの実 shutdown/reboot や外部公開設定変更を勝手に実行しない。
- 設計エージェントが直接書いた実質的な差分は、`.claude/skills/design-implementation-handoff/SKILL.md` の逆レビュー対象にする。

## 複数エージェントの同時作業

Claude Code、Codex CLI、OpenCode等、複数のAIエージェントが同時にこのリポジトリで作業することがあります。Issueの重複着手や、git状態の衝突(あるエージェントのbranch切り替え/stashが別エージェントの未commit変更を壊す)を防ぐ手順は `.claude/skills/parallel-agent-coordination/SKILL.md` を正本にします。作業開始前に必ず参照してください。

## 品質ゲート

正本は `.claude/skills/verify/SKILL.md` と `Makefile` です。

通常は以下を実行します。

```bash
make check
bash -n scripts/*.sh
```

esp Rust toolchain やローカル `firmware/config.toml` がない環境では `make firmware-build` は警告だけ出してスキップします。実機作業前には esp toolchain を入れた環境で `cargo +esp build --release --target xtensa-esp32-espidf` を必ず実行してください。

## 秘密情報

以下をコード、テスト、ログ、コミットメッセージ、PR本文へ入れないでください。

- 実Wi-Fi SSID/Password
- 実PC MACアドレス
- 実LANグローバルIPや家庭内ネットワーク詳細
- HMAC shared secret
- Windows認証情報

テンプレート値は `replace-with-*` や `AA:BB:CC:DD:EE:FF` のような明確なダミーだけを使います。
