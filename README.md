# m5stack-pc-remote

M5Stack Core2 for AWS を24時間常時稼働させ、自宅LAN上の Windows 11 Pro デスクトップPCの電源管理専用端末として使うプロジェクトです。

現在のM5Stack firmwareはRust実装です。

## 構成

```text
M5Stack Core2 for AWS
  ├─ Wi-Fi接続
  ├─ Wake-on-LAN Magic Packet送信
  ├─ TCP connect STATUS確認
  ├─ タッチUI
  └─ Telegram Bot API long polling

Windows 11 Pro PC
  └─ Rust m5stack-pc-bridge
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

m5stack-pc-bridge のポートをインターネットへ直接公開しません。賃貸無料回線などでルーターVPNを前提にできない場合は、M5StackがTelegram Bot APIを外向きHTTPSでlong pollingしてコマンドを受け取ります。
外部操作経路はコストゼロを絶対条件にします（詳細は [Cost Policy](docs/cost.md)）。

## 技術選定

- M5Stack firmware: Rust + esp-idf-sys / esp-idf-svc / esp-idf-hal
- STATUS: Rust版は TCP connect probe
- WOL: Rust版は UDP Magic Packet送信
- m5stack-pc-bridge: Rust
- 認証: HMAC-SHA256 + timestamp + nonce

Rust版はWi-Fi / WOL / STATUS / タッチUI / REBOOT / SHUTDOWN / Telegram経由操作（Phase 5D: `/status` `/wake` `/reboot` `/shutdown` のコマンド実行まで確認済み、Phase 5E インラインボタンは実装済み・実機確認待ち）まで実機確認済みです。

## セットアップ: Rust firmware

事前に `cargo install espup ldproxy espflash && espup install --targets esp32` が必要です（詳細は `firmware/README.md` 参照）。

```bash
cd firmware
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
make firmware-nvs-image
```

生成先は `firmware/.nvs-provisioning/` です。secretを含むためGit管理外です。
実機NVSを書き換える場合は、NVS partition offset/sizeを確認した上で次のように明示実行します。

```bash
python3 scripts/provision-firmware-nvs.py --write --yes --port /dev/ttyUSB0
```

現行partition tableではNVSは offset `0x9000`、size `0x6000` です（`firmware/sdkconfig.defaults` の partition 定義を参照）。`--size` は `4096` の倍数で指定してください。partition tableを変えた場合は `--offset` と `--size` を指定してください。

## セットアップ: m5stack-pc-bridge (Windows側)

```powershell
cd m5stack-pc-bridge
copy config.example.toml config.toml
cargo build --release
```

`config.toml` の `shared_secret` を32文字以上の長いランダム値に変更してください。`config.example.toml` のプレースホルダー値や短すぎる値のままでは起動しません。初期値の `dry_run = true` では実際のshutdown/rebootは実行されません。

M5Stackからの `POST /reboot` / `POST /shutdown` は、HMAC署名に加えてJSON本文の `{"confirm":true}` が必須です。
`GET /status` はm5stack-pc-bridgeプロセス自体のヘルスチェックであり、PCの電源状態判定にはM5Stack firmware側のTCP connect STATUSを使います。

対話実行での動作確認:

```powershell
.\target\release\m5stack-pc-bridge.exe --config .\config.toml
```

Windows Serviceとして常駐させるには、管理者PowerShellで以下を実行します。
バイナリ配置・設定ファイル生成・Windows Firewall受信許可ルール作成・Windows Service登録
(スタートアップ種類: 自動、異常終了時は自動再起動)をまとめて行います
(詳細は `m5stack-pc-bridge/README.md` 参照)。

```powershell
.\install.ps1
```

## セットアップ: Telegram Bot (スマホ外部操作)

賃貸無料回線などでルーターVPNを前提にできない場合、M5StackがTelegram Bot APIを外向きHTTPSでlong pollingし、スマホから `/status`、`/wake`、`/reboot`、`/shutdown`、`/update` を操作できます。設計の詳細は [External Access Design](docs/external-access.md) を参照してください。

### 1. BotFatherでbotを作る

1. TelegramでBotFather (`@BotFather`) とのチャットを開く。
2. `/newbot` を送り、bot名とusernameを設定する。
3. 発行されたbot token (`123456789:AA...` の形式) を控える。**このtokenは秘密情報です。第三者と共有したりGitへコミットしたりしないでください。**

### 2. 自分のTelegram user idを取得する

1. Telegramで `@userinfobot` など、自分のuser idを教えてくれるbotとチャットする、またはBot APIの `getUpdates` を一度手動で呼んで自分の `from.id` を確認する。

   ```bash
   curl https://api.telegram.org/bot<token>/getUpdates | jq .result[0].message.from.id
   ```

   `TELEGRAM_BOT_TOKEN` を環境変数に設定している場合は `scripts/telegram-set-commands.sh` が `setMyCommands` を登録します。
2. 数値のuser id (`123456789` のような形式) を控える。

### 3. Rust firmwareの `config.toml` に設定する

`firmware/config.toml` の以下を placeholder から変更します。

```toml
telegram_bot_token = "123456789:your-real-bot-token"
telegram_allowed_user_id = "123456789"
telegram_long_poll_timeout_seconds = 20
telegram_confirm_ttl_secs = 60
```

`TELEGRAM_ALLOWED_USER_ID` と一致しない `from.id` からのメッセージはすべて無視され、返信もされません。`TELEGRAM_BOT_TOKEN` はm5stack-pc-bridge用の `BRIDGE_SHARED_SECRET` とは別の秘密情報で、m5stack-pc-bridgeへは一切渡りません。

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
- `/update`: 確認後にfirmwareを更新
- `/lock`: 電源操作を一時的に禁止
- `/unlock`: `/lock` を解除

### 5. 実行方法

書き込み後、M5Stackの画面に `Telegram: polling` と表示されればTelegram連携が有効です。placeholderのままだと `Telegram: disabled` と表示され、Telegram機能だけが無効化されます(タッチUI・WOL・STATUSは通常どおり動作します)。

Telegramアプリから許可したuser idのアカウントで、bot宛てに以下を送信します。

- `/status`: PCのONLINE/OFFLINE、Wi-Fi RSSI、M5Stack IPを返信します。
- `/wake`: Wake-on-LANを送信し、成功/失敗を返信します。
 - `/reboot` / `/shutdown`: 即実行せず、日本語の確認メッセージが返信されます。メッセージには「再起動」または「シャットダウン」ボタンと「キャンセル」ボタン(インラインキーボード)が付いており、タップするだけで確定/キャンセルできます。ボタンを使わない場合は、同じメッセージに記載された `/confirm_reboot <nonce>` または `/confirm_shutdown <nonce>` を手入力しても構いません(後方互換)。nonceは `TELEGRAM_CONFIRM_TTL_SECS`（秒）の間だけ有効（既定 60秒）で、ボタンタップ・コマンド入力・キャンセル・期限切れのいずれか1回で消費され、以降は再利用できません。
 - `/update`: manifestのversionとsizeを提示してから確認を求め、確定後にfirmwareを更新して自動で再起動します。新しいfirmwareは起動自己診断を通るまでvalidにならず、通らないまま再起動すると旧版へ戻ります。
- `/lock` / `/unlock`: 旅行中などに誤操作・不正操作を防ぐため、電源操作を一時的に禁止します。ロック中は `/wake` `/reboot` `/shutdown` `/update` と確認ボタンをすべて拒否し、**M5Stack本体のタッチ操作も同様に拒否**します(画面に `LOCKED` と表示されます)。`/lock` `/unlock` `/status` はロック中でも受け付けます。ロック状態はメモリ上だけで保持するため、M5Stackを再起動すると解除されます。

なお、次の出来事はこちらから操作しなくてもTelegramへ通知されます。

- PCのオンライン/オフラインが切り替わったとき(瞬断での連投を防ぐため、20秒継続した変化だけを通知)
- 1日1回の定期レポート(`firmware/config.toml` の `daily_report_hour` に 0-23 のローカル時刻を設定したときだけ。`timezone_offset_hours` はUTCからのずれで、JSTなら9。既定では無効)
- M5Stack本体のタッチパネルから電源操作を実行したとき(`[本体パネル操作]` のprefixが付きます)
- 未許可ユーザーからのアクセスを検知したとき(3回たまった時点で通知。最短送信間隔1時間)

Telegram APIとのTLS通信は `firmware/src/telegram_root_ca.rs` に埋め込んだルートCA証明書でサーバー証明書を検証します。Telegramが将来ルート認証局を切り替えた場合、画面が `Telegram: polling` から `Telegram: error` に変わります。その場合の証明書更新手順は [External Access Design](docs/external-access.md) の「CA証明書のローテーション運用」を参照してください。

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

- `firmware/config.toml`
- `firmware/src/config.rs` (旧方式。ビルドログ漏えい防止のため使用禁止)
- `firmware/src/_config.rs` (旧方式。ビルドログ漏えい防止のため使用禁止)
- `m5stack-pc-bridge/config.toml`
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
