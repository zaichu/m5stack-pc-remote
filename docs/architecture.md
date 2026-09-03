# Architecture

## 現在の構成

```text
M5Stack Core2 for AWS
  ├─ Wi-Fi STA
  ├─ WOL packet sender
  ├─ TCP connect status checker
  ├─ Touch display UI
  └─ Telegram Bot API long polling

Windows 11 Pro Desktop
  ├─ BIOS/UEFI Wake-on-LAN
  ├─ NIC Wake-on-LAN
  └─ Rust m5stack-pc-bridge
```

M5Stack firmwareは `firmware/` のRust実装です。Wi-Fi / WOL / STATUS / UI / REBOOT / SHUTDOWN / Telegram経由操作まで実機確認済みです。

## 将来の外部操作

```text
Smartphone
  ↓ Telegram app
Telegram Bot API
  ↓ outbound HTTPS long polling
M5Stack Core2
  ↓ WOL / authenticated LAN request
Windows PC
```

m5stack-pc-bridgeはLAN内限定です。外部公開が必要な場合でも、m5stack-pc-bridgeの管理ポートは直接公開せず、M5Stackが外向きHTTPSで取得したコマンドだけを実行します。

詳細設計は [External Access Design](external-access.md) を正本にします。コスト方針は [Cost Policy](cost.md) を参照。初期案はTelegram Bot APIのlong polling方式です。

## 用語

- **PC STATUS (TCP probe)**: M5Stack が `pc_status_addr` へ TCP connect probe して判定する PC の電源状態。`firmware/src/net.rs:148` が正本。
- **Bridge health `/status`**: `m5stack-pc-bridge` プロセス自体のヘルスチェック（未認証、電源状態とは無関係）。`m5stack-pc-bridge/src/server.rs:83` が正本。

## 境界

- `firmware/`: M5Stack実機で動くRust firmware
- `m5stack-pc-bridge/`: Windows上で常駐するRust Windows Service
- `docs/`: 設計、フェーズ、セキュリティ
- `.claude/skills/`: 設計/実装エージェント運用の正本(現在の割り当てはCodex/Claude、`design-implementation-handoff` skillを参照)
