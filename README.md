# m5stack-pc-remote

M5Stack Core2 for AWS を24時間常時稼働させ、自宅LAN上の Windows 11 Pro デスクトップPCの電源管理専用端末として使うプロジェクトです。

現在のM5Stack firmware本線はRust実装です。C++/Arduino版は安定確認が終わるまでfallbackとして残し、不要になった段階で一括削除します。

## 構成

```text
M5Stack Core2 for AWS
  ├─ Wi-Fi接続
  ├─ Wake-on-LAN Magic Packet送信
  ├─ TCP connect STATUS確認
  ├─ タッチUI
  └─ Telegram Bot API long polling

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

- M5Stack firmware本線: Rust + esp-idf-sys / esp-idf-svc / esp-idf-hal
- M5Stack firmware fallback: PlatformIO + Arduino Framework + M5Unified (`firmware/`)
- STATUS: Rust版は TCP connect probe
- WOL: Rust版は UDP Magic Packet送信
- Windows Agent: Rust
- 認証: HMAC-SHA256 + timestamp + nonce

Rust版はWi-Fi / WOL / STATUS / タッチUI / REBOOT / SHUTDOWN / Telegram経由操作まで実機確認済みです。C++版は運用fallbackとして残しています。

## セットアップ: Rust firmware

```bash
cd firmware-rust-poc
cp config.example.toml config.toml
. ~/export-esp.sh
cargo build --release --target xtensa-esp32-espidf
espflash flash --monitor target/xtensa-esp32-espidf/release/m5remote-rust
```

`config.toml` はGit管理外です。秘密情報をRustソース(`src/`配下)へ直接書かないでください。
起動時はNVSの `m5remote` namespaceを先に読み、値があればそれを使います。NVSが未設定の
場合は `config.toml` からビルド時生成した設定をfallbackとして使います。
tokenやsecretをローテーションした時は、現時点では `config.toml` を更新して再build/flashする
運用を正本にします。NVS provisioningだけで差し替える手順は、実機で安全に確認できてから
運用手順に昇格します。

NVSイメージだけを生成する場合:

```bash
make firmware-rust-nvs-image
```

生成先は `firmware-rust-poc/.nvs-provisioning/` です。secretを含むためGit管理外です。
実機NVSを書き換える場合は、NVS partition offset/sizeを確認した上で次のように明示実行します。

```bash
python3 scripts/provision-firmware-rust-nvs.py --write --yes --port /dev/ttyUSB0
```

現行partition tableではNVSは offset `0x9000`、size `0x6000` です。partition tableを変えた
場合は `--offset` と `--size` を指定してください。

## セットアップ: C++ fallback firmware

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

Telegram経由のスマホ外部操作をC++ fallbackで使う場合は、`TELEGRAM_BOT_TOKEN` と `TELEGRAM_ALLOWED_USER_ID` も設定します(セットアップ手順は次節)。両方ともplaceholderのままか空の場合、Telegram機能は無効化され、既存のタッチUI・WOL・STATUSはそのまま動作します。

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

`config.toml` の `shared_secret` を32文字以上の長いランダム値に変更してください。`config.example.toml` のプレースホルダー値や短すぎる値のままではAgentは起動しません。初期値の `dry_run = true` では実際のshutdown/rebootは実行されません。

M5Stackからの `POST /reboot` / `POST /shutdown` は、HMAC署名に加えてJSON本文の `{"confirm":true}` が必須です。
Windows Agentの `GET /status` はAgentプロセス自体のヘルスチェックであり、PCの電源状態判定にはM5Stack firmware側のICMP ping STATUSを使います。

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

## セットアップ: Telegram Bot (スマホ外部操作)

賃貸無料回線などでルーターVPNを前提にできない場合、M5StackがTelegram Bot APIを外向きHTTPSでlong pollingし、スマホから `/status`、`/wake`、`/reboot`、`/shutdown` を操作できます。設計の詳細は [External Access Design](docs/external-access.md) を参照してください。

### 1. BotFatherでbotを作る

1. TelegramでBotFather (`@BotFather`) とのチャットを開く。
2. `/newbot` を送り、bot名とusernameを設定する。
3. 発行されたbot token (`123456789:AA...` の形式) を控える。**このtokenは秘密情報です。第三者と共有したりGitへコミットしたりしないでください。**

### 2. 自分のTelegram user idを取得する

1. Telegramで `@userinfobot` など、自分のuser idを教えてくれるbotとチャットする、またはBot APIの `getUpdates` を一度手動で呼んで自分の `from.id` を確認する。
2. 数値のuser id (`123456789` のような形式) を控える。

### 3. `config.h` に設定する

`firmware/include/config.h` に以下を追加(または placeholder から変更)します。

```cpp
#define TELEGRAM_BOT_TOKEN "123456789:your-real-bot-token"
#define TELEGRAM_ALLOWED_USER_ID "123456789"
#define TELEGRAM_LONG_POLL_TIMEOUT_SECONDS 20
#define TELEGRAM_CONFIRM_TTL_MS 60000
```

`TELEGRAM_ALLOWED_USER_ID` と一致しない `from.id` からのメッセージはすべて無視され、返信もされません。`TELEGRAM_BOT_TOKEN` はWindows Agent用の `AGENT_SHARED_SECRET` とは別の秘密情報で、Windows Agentへは一切渡りません。

### 4. Telegramのコマンド候補を登録する

Telegramアプリで `/` を入力したときに候補一覧を出すには、Bot APIの `setMyCommands` でbot側へコマンドを登録します。`TELEGRAM_BOT_TOKEN` 設定後に以下を実行してください。

```bash
bash scripts/telegram-set-commands.sh
```

登録される候補:

- `/status`: PC状態を表示
- `/wake`: PCへWake-on-LANを送信
- `/reboot`: 確認後にPCを再起動
- `/shutdown`: 確認後にPCをシャットダウン

### 5. 実行方法

書き込み後、M5Stackの画面に `Telegram: polling` と表示されればTelegram連携が有効です。placeholderのままだと `Telegram: disabled` と表示され、Telegram機能だけが無効化されます(タッチUI・WOL・STATUSは通常どおり動作します)。

Telegramアプリから許可したuser idのアカウントで、bot宛てに以下を送信します。

- `/status`: PCのONLINE/OFFLINE、Wi-Fi RSSI、M5Stack IPを返信します。
- `/wake`: Wake-on-LANを送信し、成功/失敗を返信します。
- `/reboot` / `/shutdown`: 即実行せず、日本語の確認メッセージが返信されます。メッセージには「再起動」または「シャットダウン」ボタンと「キャンセル」ボタン(インラインキーボード)が付いており、タップするだけで確定/キャンセルできます。ボタンを使わない場合は、同じメッセージに記載された `/confirm_reboot <nonce>` または `/confirm_shutdown <nonce>` を手入力しても構いません(後方互換)。nonceは `TELEGRAM_CONFIRM_TTL_MS` の間だけ有効で、ボタンタップ・コマンド入力・キャンセル・期限切れのいずれか1回で消費され、以降は再利用できません。

Telegram APIとのTLS通信は `firmware/src/telegram_root_ca.h` に埋め込んだルートCA証明書でサーバー証明書を検証します。Telegramが将来ルート認証局を切り替えた場合、画面が `Telegram: polling` から `Telegram: error` に変わります。その場合の証明書更新手順は [External Access Design](docs/external-access.md) の「CA証明書のローテーション運用」を参照してください。

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
- `firmware-rust-poc/config.toml`
- `firmware-rust-poc/src/config.rs` (旧方式。ビルドログ漏えい防止のため使用禁止)
- `firmware-rust-poc/src/_config.rs` (旧方式。ビルドログ漏えい防止のため使用禁止)
- `windows-agent/config.toml`
- `.env`
- `*.pem`
- `*.key`
- `*secret*.json`

テンプレートだけをGit管理します。
Rust firmwareでは、秘密情報をRustソースへ直接書かず、Git管理外のTOML設定を使います。
Rust firmware buildはログを一度ローカル一時ファイルへ捕捉し、Telegram token形式や
`config.toml` の秘密値が出ていないか確認してから表示します。漏えいの疑いがある場合は
ログ本文を表示せず停止します。

## ドキュメント

- [Architecture](docs/architecture.md)
- [Phases](docs/phases.md)
- [Security](docs/security.md)
- [External Access Design](docs/external-access.md)
- [Cost Policy](docs/cost.md)
