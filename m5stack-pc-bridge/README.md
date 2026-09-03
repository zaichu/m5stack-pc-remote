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

本文は `{"confirm":true}` が必須です。

署名対象は以下です。

```text
METHOD + "\n" + PATH + "\n" + TIMESTAMP + "\n" + NONCE + "\n" + SHA256(BODY)
```
