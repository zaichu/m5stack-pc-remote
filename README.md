# m5stack-pc-remote

M5Stack Core2 for AWS を24時間常時稼働させ、自宅LAN上の Windows 11 Pro デスクトップPCの電源管理専用端末として使うプロジェクトです。

初期実装は **Wi-Fi接続 -> Wake-on-LAN -> STATUS** に絞っています。REBOOT / SHUTDOWN は Rust製 Windows Agent による認証付きAPIとして段階的に追加します。

## 構成

```text
M5Stack Core2 for AWS
  ├─ Wi-Fi接続
  ├─ Wake-on-LAN Magic Packet送信
  └─ ICMP ping STATUS確認

Windows 11 Pro PC
  └─ Rust Windows Agent
       ├─ GET /status
       ├─ POST /reboot
       └─ POST /shutdown
```

将来的な外部操作は以下の経路を想定します。

```text
Smartphone
  ↓
Telegram Bot API
  ↓
M5Stack Core2
  ↓
Windows PC
```

Windows Agent のポートをインターネットへ直接公開しません。賃貸無料回線などでルーターVPNを前提にできない場合は、M5StackがTelegram Bot APIを外向きHTTPSでlong pollingしてコマンドを受け取ります。
外部操作経路はコストゼロを絶対条件にし、月額課金・従量課金・無料枠超過リスクのある構成を運用必須経路にしません。

## 技術選定

- M5Stack firmware: PlatformIO + Arduino Framework + M5Unified
- STATUS: 初期は ICMP ping
- WOL: ESP32 Arduino標準の `WiFiUDP`
- Windows Agent: Rust
- 認証: HMAC-SHA256 + timestamp + nonce

Core2の画面・タッチ・Wi-Fiまわりは Arduino + M5Unified が安定しており、PlatformIOで再現性を確保しやすいためこの構成を採用しています。RustはWindows Agent側で採用し、単一バイナリ配布と堅牢な認証処理を優先します。

## セットアップ: firmware

```bash
cd firmware
cp include/config.example.h include/config.h
```

`include/config.h` を編集します。

```cpp
#define WIFI_SSID "your-wifi-ssid"
#define WIFI_PASSWORD "your-wifi-password"
#define PC_HOSTNAME "desktop"
#define PC_IP_ADDRESS "192.168.1.100"
#define PC_MAC_ADDRESS "AA:BB:CC:DD:EE:FF"
#define AGENT_PORT 18080
#define AGENT_SHARED_SECRET "replace-with-the-same-secret-as-windows-agent"
```

ビルド:

```bash
pio run -d firmware
```

書き込み:

```bash
pio run -d firmware -t upload
```

Serial monitor:

```bash
pio device monitor -d firmware
```

## セットアップ: Windows Agent

```powershell
cd windows-agent
copy config.example.toml config.toml
cargo build --release
```

`config.toml` の `shared_secret` を長いランダム値に変更してください。初期値の `dry_run = true` では実際のshutdown/rebootは実行されません。

M5Stackからの `POST /reboot` / `POST /shutdown` は、HMAC署名に加えてJSON本文の `{"confirm":true}` が必須です。

起動:

```powershell
.\target\release\pc-remote-agent.exe --config .\config.toml
```

Windows起動時に自動起動するには、管理者PowerShellで以下を実行します。
バイナリ配置・設定ファイル生成・Windows Firewall受信許可ルール作成・Scheduled Task登録を
まとめて行います(詳細は `windows-agent/README.md` 参照)。

```powershell
.\install.ps1
```

## ローカル品質チェック

```bash
make check
```

初回またはworktree作成後にGit hooksを有効化します。

```bash
make install-hooks
```

## 秘密情報

Gitへ入れないファイル:

- `firmware/include/config.h`
- `windows-agent/config.toml`
- `.env`
- `*.pem`
- `*.key`
- `*secret*.json`

テンプレートだけをGit管理します。

## ドキュメント

- [Architecture](docs/architecture.md)
- [Phases](docs/phases.md)
- [Security](docs/security.md)
- [External Access Design](docs/external-access.md)
- [Cost Policy](docs/cost.md)
