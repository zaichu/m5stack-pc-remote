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

## Windows起動時の自動起動

管理者PowerShellで実行します。

```powershell
.\install-scheduled-task.ps1
```

削除:

```powershell
.\uninstall-scheduled-task.ps1
```

本番運用で `dry_run = false` にする前に、Windows Firewall で待受ポートをプライベートネットワークに限定してください。

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
