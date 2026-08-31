# Security

## 基本方針

- Windows Agentをインターネットへ直接公開しない。
- LAN内でもshutdown/rebootは無認証にしない。
- 秘密鍵、Wi-Fiパスワード、実MACアドレスをGitに入れない。
- REBOOT / SHUTDOWN はユーザー確認UIを必須にする。

## HMAC署名

署名対象:

```text
METHOD + "\n" +
PATH + "\n" +
TIMESTAMP + "\n" +
NONCE + "\n" +
SHA256(BODY)
```

ヘッダー:

- `X-Timestamp`
- `X-Nonce`
- `X-Signature`

Agent側の検証:

- timestamp が許容範囲内であること。
- nonce が未使用であること。
- HMAC-SHA256署名が一致すること。
- 許可されたpathだけを実行すること。
- `POST /reboot` と `POST /shutdown` は、署名済みJSON本文の `confirm: true` を必須にすること。

## Windows Firewall

Agentの待受ポートはプライベートネットワークに限定します。ルーターのポート開放やUPnPによる公開は行いません。

## shared_secret の保存とACL

- `install.ps1` は `config.toml`(shared_secretを含む)を `%ProgramData%\m5stack-pc-remote-agent\config.toml` に配置します。
- インストール先ディレクトリは `icacls` でAdministratorsとSYSTEMのみに読み書きを制限し、既定の継承ACLを解除します。ローカルの一般ユーザーからは読み取れません。
- ローカル一般ユーザーがshared_secretを読めると、LAN内からREBOOT/SHUTDOWNを実行できる認証鍵が漏れるため、この制限は必須とみなします。
- `uninstall.ps1` はデフォルトでインストール先ディレクトリを削除しません。削除しない場合はACL制限が維持されている前提のため、手動でACLを緩めないでください。
