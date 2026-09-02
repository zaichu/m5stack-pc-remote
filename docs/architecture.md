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

詳細設計は [External Access Design](external-access.md) を正本にします。コストゼロを絶対条件にし、ルーターVPNを前提にできないため、初期案はTelegram Bot APIのlong polling方式です。

## 境界

- `firmware/`: M5Stack実機で動くRust firmware
- `m5stack-pc-bridge/`: Windows上で常駐するRust Windows Service
- `docs/`: 設計、フェーズ、セキュリティ
- `.claude/skills/`: Codex/Claude運用の正本
