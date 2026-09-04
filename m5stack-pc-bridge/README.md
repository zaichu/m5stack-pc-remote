# m5stack-pc-bridge

Windows PC側で常駐するRust製Windows Serviceです。M5Stack Core2から届く`REBOOT` /
`SHUTDOWN`リクエストをHMAC-SHA256署名、timestamp、nonceで検証してから実行します。
M5Stack側のfirmwareは `firmware/` を参照してください。

## セットアップ

```powershell
cd m5stack-pc-bridge
copy config.example.toml config.toml
cargo build --release
```

`config.toml` の `shared_secret` は32文字以上の長いランダム値へ変更してください。`config.example.toml` のプレースホルダー値や短すぎる値のままでは起動しません。`dry_run = true` の間は実際の電源操作を実行しません。

任意設定として `telegram_bot_token` と `telegram_chat_id` を両方書くと、HTTP認証に失敗したリクエストが3件たまった時点でTelegramへアラートを送ります(最短送信間隔1時間)。省略した場合は通知しないだけで、電源操作の動作には影響しません。このファイルへbot tokenを置くことのリスク評価は `docs/security.md` を参照してください。

## 動作確認(対話実行)

Serviceとしてインストールする前に、コンソールから直接動かして確認できます。
`config.toml` は既定で実行ファイルと同じディレクトリのものを使います(`--config` で上書き可能)。

```powershell
.\target\release\m5stack-pc-bridge.exe --config .\config.toml
```

Windowsバイナリは、Service Control Manager経由の起動ではない場合(=手元で直接実行した場合)は自動的にforeground実行にフォールバックするため、上記コマンドはインストール後の`m5stack-pc-bridge.exe`をそのまま実行しても動きます。

## Windows Serviceとして常駐

管理者PowerShellで実行します。`config.toml` が無い場合は `config.example.toml` から
暗号論的乱数(`RandomNumberGenerator`、64文字)で生成した `shared_secret` を使って
作成します(生成後、`firmware/config.toml` の `bridge_shared_secret` を
同じ値に必ず合わせてください)。

```powershell
.\install.ps1
```

`%ProgramData%\m5stack-pc-bridge\` へバイナリと設定を配置し、`M5StackPcBridge` という
名前でWindows Serviceを登録します(スタートアップ種類: 自動、異常終了時は自動再起動)。
旧版(Task Scheduler常駐)が入っていれば、ポート競合を避けるため自動的に削除します。
同時に、`config.toml` の `bind` ポートに対してプライベートネットワークプロファイル限定の
Windows Firewall受信許可ルールも作成します(Windows Firewallは既定でこの種の
未知のアプリへの受信を静かにブロックするため、実機確認で必須と分かりました)。

インストール先ディレクトリのACLは `%ProgramData%` からの既定の継承のままです。
`config.toml` はREBOOT/SHUTDOWNの認証に使うshared_secretを平文で含むため、
このPCを他のローカルアカウントと共有している場合は注意してください(詳細は
`docs/security.md` を参照)。

インストール直後にServiceを起動する場合は `-Start` を付けます。

```powershell
.\install.ps1 -Start
```

Serviceの状態確認・手動での起動/停止:

```powershell
Get-Service M5StackPcBridge
Start-Service M5StackPcBridge
Stop-Service M5StackPcBridge
```

アンインストール(ServiceとFirewallルールを削除、`-RemoveFiles` でインストール先も削除):

```powershell
.\uninstall.ps1 -RemoveFiles
```

本番運用で `dry_run = false` にする前に、`%ProgramData%\m5stack-pc-bridge\config.toml`
の内容を再確認してください。

## 監査ログ

認証成功かつ `confirm: true` の `POST /reboot` / `POST /shutdown` は、実行ファイルと
同じディレクトリの `audit.log` へ追記します。記録するのは時刻、操作種別、`dry_run`、
結果のみで、`shared_secret`、署名、nonce、リクエスト本文、Telegram tokenは書きません。

監査ログを書けない場合、m5stack-pc-bridgeは電源操作を実行せず `500` を返します。
`audit.log` が約1MBを超えると、書き込み前に `audit.log.1` へ1世代だけローテーションします。

## API

- `GET /status`
- `POST /reboot`
- `POST /shutdown`
- `GET /firmware/manifest` (OTA Phase 2: 要HMAC認証)
- `GET /firmware` (OTA Phase 2: 要HMAC認証)

`GET /status` はm5stack-pc-bridgeプロセス自体のヘルスチェックです。PCの電源状態を判定するエンドポイントではありません。

レスポンス例:

```json
{
  "agent_online": true,
  "agent": "m5stack-pc-bridge",
  "status": "ok"
}
```

`POST` は以下のヘッダーが必須です。

- `X-Timestamp`
- `X-Nonce`
- `X-Signature`

本文は `{"confirm":true}` が必須です。本文は128byte以下で、`confirm` 以外のfieldは拒否します。
認証失敗時のHTTP response bodyは固定の `unauthorized` です。

署名対象等の wire protocol は `shared/pc-remote-signing/src/lib.rs` を正本とします。

## firmware配信(OTA Phase 2)

実行ファイルと同じディレクトリに `firmware.bin` を置くと、`GET /firmware` で
バイナリ本体、`GET /firmware/manifest` でメタ情報(`version`、`size`、`sha256`、
`created_at`、HMAC-SHA256署名 `signature`)を配信します。`firmware.bin` が無い
ときは `404` を返します。

- `version` は同じディレクトリの `firmware.version`(1行のテキスト)から読みます。
  無い・空の場合は `"unknown"` になります。内容の同一性は `sha256`(実バイナリ
  から計算)で担保されるため、版ファイルが無くても配信は壊れません。
- `created_at` は `firmware.bin` の更新時刻(RFC3339、UTC)です。
- `signature` は `shared_secret` によるHMAC-SHA256で、署名対象の組み立ては
  `shared/pc-remote-signing` の `sign_manifest` が正本です。bridgeは配布場所で
  あって信頼の根ではないため、M5Stack側は公開値だけを信じず署名を検証します
  (検証側はPhase 3で実装)。
- 読み取り専用ですが電源操作と同じHMACリクエスト認証(`X-Timestamp`、
  `X-Nonce`、`X-Signature`、本文は空)を要求します。LAN内だからという理由で
  無認証APIを増やさない方針(AGENTS.md)のためです。電源操作の権限は渡しません。
### `firmware.bin` の作り方

`make firmware-build` が作るのはELFと `bootloader.bin` / `partition-table.bin` だけで、
ここで配信するアプリイメージは含まれません。次で生成します。

```bash
make firmware-build
make firmware-package
```

`firmware/dist/firmware.bin` と `firmware/dist/firmware.version` ができ、`sha256` が
表示されます。この値は bridge が返す manifest の `sha256` と一致します。

`scripts/package-firmware.sh` は `espflash save-image`(`--merge` なし = アプリイメージ
単体)を使い、次を正本から取ります。値を固定で書かないのは、片方だけ変えたときに
静かに食い違うためです。

- flash size: `firmware/sdkconfig.defaults` の `CONFIG_ESPTOOLPY_FLASHSIZE_*MB`。
  省くと espflash は4MB想定でヘッダを書き、実機(16MB)と食い違うイメージになります。
- パーティション: `firmware/partitions.csv`。渡さないと `ota_0`(2M)に収まるかを
  実サイズで検査できません。
- version: `firmware/Cargo.toml` の `version`。

生成後に先頭バイトが `0xE9`(ESP32アプリイメージのmagic)であることも検査します。
ELFやマージ済みイメージを誤って配信すると、実機は書き込んだ後の起動で初めて失敗
するためです。

### 配置

`%ProgramData%\m5stack-pc-bridge\` へ `m5stack-pc-bridge.exe` と一緒に置きます。

```powershell
Copy-Item .\firmware.bin "$env:ProgramData\m5stack-pc-bridge\firmware.bin"
Copy-Item .\firmware.version "$env:ProgramData\m5stack-pc-bridge\firmware.version"
```
