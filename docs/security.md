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
