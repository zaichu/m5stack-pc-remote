# HANDOFF

このファイルは、別の Codex/Claude セッションへ引き継ぐための恒久構成メモです。
変わりにくい構造だけを書きます。実装内容・検証結果・判断の経緯は GitHub Issue
と PR に記録します(このファイルには書きません)。

## 恒久構成

- リポジトリ: `m5stack-pc-remote`
- M5Stack側(本線): `firmware/`
  - PlatformIO
  - Arduino Framework
  - M5Unified
  - Phase 1 は Wi-Fi、Wake-on-LAN、ICMP ping STATUS
- M5Stack側(Rust化PoC、Issue #16): `firmware-rust-poc/`
  - `firmware/` の置き換えではない。PoCが成功するまで `firmware/` は変更しない
  - esp-idf-sys(std)ベース。`m5unified` crateでM5Unified(C++)をラップする方針を検証中
- Windows側: `windows-agent/`
  - Rust
  - HMAC-SHA256認証
  - timestamp + nonceによるリプレイ防止
  - `dry_run = true` を初期値にして、実shutdown/rebootを誤実行しない
- ローカル秘密設定(Git管理外):
  - `firmware/include/config.h`
  - `windows-agent/config.toml`
- 外部操作の経路:
  - `Smartphone -> Telegram Bot API (outbound HTTPS long polling) -> M5Stack Core2 -> Windows PC`
  - Windows Agentを直接インターネットへ公開しない
  - `firmware/src/telegram_client.h` / `telegram_client.cpp` (Telegram long polling、core 0の専用FreeRTOSタスク)
  - `firmware/src/power_controller.h` / `power_controller.cpp` (WOL送信、Windows AgentへのHMAC署名付きPOST、PC ping状態。タッチUIとTelegramタスクの共有ロジック)
  - 設計の正本: `docs/external-access.md`、`docs/cost.md`(コストゼロが絶対条件)

## 次のセッションへの依頼例

```text
AGENTS.md、CLAUDE.md、HANDOFF.mdを読んでから、GitHub Issueの一覧(gh issue list)を確認してください。
続けて git status、make check、必要なら pio run -d firmware を確認してください。
```
