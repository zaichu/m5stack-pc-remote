# Windows Agent

Rust製のLAN内常駐Agentです。M5Stackからの `REBOOT` / `SHUTDOWN` はHMAC-SHA256署名、timestamp、nonceで検証します。

## セットアップ

```powershell
cd windows-agent
copy config.example.toml config.toml
cargo build --release
```

`config.toml` の `shared_secret` は長いランダム値へ変更してください。`dry_run = true` の間は実際の電源操作を実行しません。

## 起動

```powershell
.\target\release\pc-remote-agent.exe --config .\config.toml
```

## インストール(Windows起動時の自動起動)

管理者PowerShellで実行します。`config.toml` が無い場合は `config.example.toml` から
暗号論的乱数(`RandomNumberGenerator`、64文字)で生成した `shared_secret` を使って
作成します(生成後、`firmware/include/config.h` の `AGENT_SHARED_SECRET` を
同じ値に必ず合わせてください)。

```powershell
.\install.ps1
```

`%ProgramData%\m5stack-pc-remote-agent\` へバイナリと設定を配置し、Windows起動時に
`SYSTEM` 権限でエージェントを起動するScheduled Taskを登録します。同時に、
`config.toml` の `bind` ポートに対してプライベートネットワークプロファイル限定の
Windows Firewall受信許可ルールも作成します(Windows Firewallは既定でこの種の
未知のアプリへの受信を静かにブロックするため、実機確認で必須と分かりました)。

インストール先ディレクトリのACLは、`icacls` でAdministratorsとSYSTEMのみに
読み取り・書き込みを制限します。`config.toml` はREBOOT/SHUTDOWNの認証に使う
shared_secretを平文で含むため、ローカルの一般ユーザーから読めないようにする
ためです。

インストール直後にエージェントを起動する場合は `-Start` を付けます。

```powershell
.\install.ps1 -Start
```

アンインストール(Scheduled TaskとFirewallルールを削除、`-RemoveFiles` でインストール先も削除):

```powershell
.\uninstall.ps1 -RemoveFiles
```

本番運用で `dry_run = false` にする前に、`%ProgramData%\m5stack-pc-remote-agent\config.toml`
の内容を再確認してください。

## API

- `GET /status`
- `POST /reboot`
- `POST /shutdown`

`POST` は以下のヘッダーが必須です。

- `X-Timestamp`
- `X-Nonce`
- `X-Signature`

本文は `{"confirm":true}` が必須です。

署名対象は以下です。

```text
METHOD + "\n" + PATH + "\n" + TIMESTAMP + "\n" + NONCE + "\n" + SHA256(BODY)
```
