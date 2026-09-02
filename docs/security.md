# Security

## 基本方針

- m5stack-pc-bridgeをインターネットへ直接公開しない。
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

m5stack-pc-bridge側の検証:

- timestamp が許容範囲内であること。
- nonce が未使用であること。
- HMAC-SHA256署名が一致すること。
- 許可されたpathだけを実行すること。
- `POST /reboot` と `POST /shutdown` は、署名済みJSON本文の `confirm: true` を必須にすること。

## Windows Firewall

m5stack-pc-bridgeの待受ポートはプライベートネットワークに限定します。ルーターのポート開放やUPnPによる公開は行いません。

## Telegram Bot token / allowed user / confirmation nonce

- `TELEGRAM_BOT_TOKEN` と `TELEGRAM_ALLOWED_USER_ID` は `firmware/config.toml` に置き、m5stack-pc-bridge用の `BRIDGE_SHARED_SECRET` とは分離する。Telegramや外部中継先には `BRIDGE_SHARED_SECRET` を渡さない。
- どちらもSerialログ、画面表示、コミット、PR本文には出さない。`config.example.toml` にはダミー値だけを置く。
- `from.id` を文字列として `TELEGRAM_ALLOWED_USER_ID` と厳密一致で比較する。一致しないupdateは処理も返信もしない。`callback_query`(インラインボタン)の `from.id` も同様に厳密一致で検証し、一致しない場合はpending確認を操作せず、`answerCallbackQuery` で短い拒否文言だけ返す(`sendMessage` による通常返信はしない)。
- `/reboot` / `/shutdown` は即実行しない。6文字の確認nonceをRAM上だけに生成し、`TELEGRAM_CONFIRM_TTL_MS` で失効させる。確認メッセージには確定ボタン(再起動/シャットダウン)とキャンセルボタンのインラインキーボードを付ける。
- 確定ボタン・キャンセルボタン・従来の確認コマンド (`/confirm_reboot <nonce>` / `/confirm_shutdown <nonce>`) のいずれも、nonce一致・TTL内・actionの種類(reboot/shutdown)一致のときだけ実行する。ボタンの `callback_data` は `confirm:<reboot|shutdown>:<nonce>` / `cancel:<reboot|shutdown>:<nonce>` の形式(Telegramの1-64byte制限内)で、古いメッセージのボタンや別action/別nonceのボタンは一致しないため通らない。
- nonceは実行の成功/失敗、キャンセル、またはnonce不一致・期限切れに関わらず1回で消費し、以降の確定ボタン・キャンセルボタン・`/confirm_*` では再利用できない。これにより同じ確認要求へのnonce総当たりを防ぐ。
- `callback_query` はTelegramの仕様どおり、認可の成否や処理結果によらず必ず `answerCallbackQuery` を呼び、クライアント側のボタン読み込み状態を終える。
- 確認実行時はm5stack-pc-bridge向けHMAC署名付きPOSTを使うため、m5stack-pc-bridge側の `confirm: true` 必須条件・timestamp・nonce検証はTelegram経由でも同様に効く。
- TelegramとのHTTPS通信はサーバー証明書チェーンを検証する。ルートCAは `firmware/src/telegram_root_ca.rs` に埋め込んだ「Go Daddy Root Certificate Authority - G2」(有効期限2037-12-31)。bot tokenは全てのTelegram API URLに含まれるため、経路上の中間者攻撃に対しても証明書検証で保護する。証明書検証省略は使わない。
- Telegramがルート認証局を切り替えた場合、この検証は失敗するようになる(画面が `Telegram: error` になる)。ローテーション手順は `docs/external-access.md` の「CA証明書のローテーション運用」を正本とする。

## shared_secret の保存とACL

- `install.ps1` は `config.toml`(shared_secretを含む)を `%ProgramData%\m5stack-pc-bridge\config.toml` に配置します。
- ACLは `%ProgramData%` からの既定の継承のままにしています(判断: 2026-09-03)。当初はAdministrators/SYSTEM限定に`icacls`でロックダウンする方針でしたが、実機で`icacls`が「成功」と報告しつつ実際には権限が適用されず、誰もアクセスできない空のACLになって復旧できなくなる事象が起きたため撤回しました。この結果、shared_secretはこのPCの他のローカルアカウントからも読める状態になります(このPCを他ユーザーと共有していない前提の運用とする)。
- m5stack-pc-bridgeは、`config.example.toml` のプレースホルダー `shared_secret` と32文字未満の `shared_secret` を起動時に拒否します。
- nonceの保持TTLは `allowed_skew_seconds` と連動し、timestamp skewを許容している間は同じnonceの再利用を拒否します。
- `uninstall.ps1` はデフォルトでインストール先ディレクトリを削除しません。`-RemoveFiles` で明示的に削除できます。
