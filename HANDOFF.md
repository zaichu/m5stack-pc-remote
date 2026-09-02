# HANDOFF

このファイルは、別の Codex/Claude セッションへ引き継ぐための恒久構成メモです。
変わりにくい構造だけを書きます。実装内容・検証結果・判断の経緯は GitHub Issue
と PR に記録します(このファイルには書きません)。

## 恒久構成

- リポジトリ: `m5stack-pc-remote`
- M5Stack側: `firmware/`
  - 主要機能(Wi-Fi / WOL / STATUS / UI / REBOOT / SHUTDOWN / Telegram)は実機確認済み
  - esp-idf-sys/esp-idf-svc/esp-idf-hal(std)ベースの純Rustスタック
  - `m5unified` crateはESP-IDF I2C driver_ng競合のため不採用
  - 旧C++/Arduino/M5Unified版は実運用検証を経て削除済み(#24)
- Windows側: `m5stack-pc-bridge/`
  - Rust
  - Windows Service(SCM管理、スタートアップ自動、異常終了時は自動再起動)として常駐。`install.ps1`/`uninstall.ps1`で管理する
  - HMAC-SHA256認証
  - timestamp + nonceによるリプレイ防止
  - `dry_run = true` を初期値にして、実shutdown/rebootを誤実行しない
- ローカル秘密設定(Git管理外):
  - `m5stack-pc-bridge/config.toml`
  - `firmware/config.toml`(secretをRustソースへ直接書かない)
- 外部操作の経路:
  - `Smartphone -> Telegram Bot API (outbound HTTPS long polling) -> M5Stack Core2 -> Windows PC`
  - m5stack-pc-bridgeを直接インターネットへ公開しない
  - `firmware/src/telegram.rs` / `agent.rs` / `net.rs`
  - 設計の正本: `docs/external-access.md`、`docs/cost.md`(コストゼロが絶対条件)

## 次のセッションへの依頼例

```text
AGENTS.md、CLAUDE.md、HANDOFF.mdを読んでから、GitHub Issueの一覧(gh issue list)を確認してください。
続けて git status、make check を確認してください。
```
