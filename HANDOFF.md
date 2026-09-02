# HANDOFF

このファイルは、別の Codex/Claude セッションへ引き継ぐための恒久構成メモです。
変わりにくい構造だけを書きます。実装内容・検証結果・判断の経緯は GitHub Issue
と PR に記録します(このファイルには書きません)。

## 恒久構成

- リポジトリ: `m5stack-pc-remote`
- M5Stack側(安定版fallback): `firmware/`
  - PlatformIO
  - Arduino Framework
  - M5Unified
  - Phase 1 は Wi-Fi、Wake-on-LAN、ICMP ping STATUS
- M5Stack側(本線): `firmware-rust-poc/`
  - 主要機能は実機確認済み。ディレクトリ名は当面維持するが、本線として扱う。
  - `firmware/` のC++版は安定確認が終わるまでfallbackとして残し、不要になった段階で一括削除する
  - esp-idf-sys/esp-idf-svc/esp-idf-hal(std)ベースの純Rustスタック
  - `m5unified` crateはESP-IDF I2C driver_ng競合のため不採用
- Windows側: `windows-agent/`
  - Rust
  - HMAC-SHA256認証
  - timestamp + nonceによるリプレイ防止
  - `dry_run = true` を初期値にして、実shutdown/rebootを誤実行しない
- ローカル秘密設定(Git管理外):
  - `firmware/include/config.h`
  - `windows-agent/config.toml`
  - `firmware-rust-poc/config.toml`(secretをRustソースへ直接書かない)
- 外部操作の経路:
  - `Smartphone -> Telegram Bot API (outbound HTTPS long polling) -> M5Stack Core2 -> Windows PC`
  - Windows Agentを直接インターネットへ公開しない
  - Rust本線: `firmware-rust-poc/src/telegram.rs` / `agent.rs` / `net.rs`
  - C++ fallback: `firmware/src/telegram_client.h` / `telegram_client.cpp`、`firmware/src/power_controller.h` / `power_controller.cpp`
  - 設計の正本: `docs/external-access.md`、`docs/cost.md`(コストゼロが絶対条件)

## 次のセッションへの依頼例

```text
AGENTS.md、CLAUDE.md、HANDOFF.mdを読んでから、GitHub Issueの一覧(gh issue list)を確認してください。
続けて git status、make check、必要なら pio run -d firmware を確認してください。
```
