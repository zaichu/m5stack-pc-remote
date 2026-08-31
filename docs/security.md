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

## Telegram Bot token / allowed user / confirmation nonce

- `TELEGRAM_BOT_TOKEN` と `TELEGRAM_ALLOWED_USER_ID` は `firmware/include/config.h` に置き、Windows Agent用の `AGENT_SHARED_SECRET` とは分離する。Telegramや外部中継先には `AGENT_SHARED_SECRET` を渡さない。
- どちらもSerialログ、画面表示、コミット、PR本文には出さない。`config.example.h` にはダミー値だけを置く。
- `from.id` を文字列として `TELEGRAM_ALLOWED_USER_ID` と厳密一致で比較する。一致しないupdateは処理も返信もしない。
- `/reboot` / `/shutdown` は即実行しない。6文字の確認nonceをRAM上だけに生成し、`TELEGRAM_CONFIRM_TTL_MS` で失効させる。
- 確認コマンド (`/confirm_reboot <nonce>` / `/confirm_shutdown <nonce>`) はnonce一致・TTL内・actionの種類(reboot/shutdown)一致のときだけ実行する。
- nonceは実行の成功/失敗、またはnonce不一致・期限切れに関わらず1回で消費し、以降の `/confirm_*` では再利用できない。これにより同じ確認要求へのnonce総当たりを防ぐ。
- 確認実行時は既存のWindows Agent向けHMAC署名付きPOST (`postAgentCommand`) をそのまま使うため、Windows Agent側の `confirm: true` 必須条件・timestamp・nonce検証はTelegram経由でも同様に効く。
- TelegramとのHTTPS通信 (`WiFiClientSecure`) は `setInsecure()` でサーバー証明書検証を省略している。トークン自体は漏れないが、経路上の第三者による通信内容の盗聴・改ざんに対する保証は弱い。残リスクとして `docs/external-access.md` に明記する。

## shared_secret の保存とACL

- `install.ps1` は `config.toml`(shared_secretを含む)を `%ProgramData%\m5stack-pc-remote-agent\config.toml` に配置します。
- インストール先ディレクトリは `icacls` でAdministratorsとSYSTEMのみに読み書きを制限し、既定の継承ACLを解除します。ローカルの一般ユーザーからは読み取れません。
- ローカル一般ユーザーがshared_secretを読めると、LAN内からREBOOT/SHUTDOWNを実行できる認証鍵が漏れるため、この制限は必須とみなします。
- `uninstall.ps1` はデフォルトでインストール先ディレクトリを削除しません。削除しない場合はACL制限が維持されている前提のため、手動でACLを緩めないでください。
