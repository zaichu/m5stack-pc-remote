# Architecture

## 現在の構成

```text
M5Stack Core2 for AWS
  ├─ Wi-Fi STA
  ├─ WOL packet sender
  ├─ ICMP ping status checker
  └─ Touch display UI

Windows 11 Pro Desktop
  ├─ BIOS/UEFI Wake-on-LAN
  ├─ NIC Wake-on-LAN
  └─ Rust Agent
```

Phase 1では Windows Agent を使わず、M5Stack から Magic Packet を送信し、PCの固定IPへ ping してONLINE/OFFLINEを判定します。

## 将来の外部操作

```text
Smartphone
  ↓ HTTPS
Cloudflare Worker等
  ↓ authenticated relay
M5Stack Core2
  ↓ WOL / authenticated LAN request
Windows PC
```

Windows AgentはLAN内限定です。外部公開が必要な場合でも、公開するのはWorker等の中継層に限定します。

## 境界

- `firmware/`: M5Stack実機で動くコード
- `windows-agent/`: Windows上で常駐するRust Agent
- `docs/`: 設計、フェーズ、セキュリティ
- `.claude/skills/`: Codex/Claude運用の正本
