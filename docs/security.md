# Security

## 基本方針

- m5stack-pc-bridgeをインターネットへ直接公開しない。
- LAN内でもshutdown/rebootは無認証にしない。
- 秘密鍵、Wi-Fiパスワード、実MACアドレスをGitに入れない。
- REBOOT / SHUTDOWN はユーザー確認UIを必須にする。

## HMAC署名

wire protocol の正本は `shared/pc-remote-signing/src/lib.rs` です。canonical 文字列とヘッダーはそこで定義します。

```text
METHOD + "\n" + PATH + "\n" + TIMESTAMP + "\n" + NONCE + "\n" + SHA256(BODY)
X-Signature = HMAC-SHA256(shared_secret, canonical)
ヘッダー: X-Timestamp, X-Nonce, X-Signature
```

m5stack-pc-bridge側の検証:

- timestamp が許容範囲内であること。
- nonce が未使用であること。
- HMAC-SHA256署名が一致すること。
- 許可されたpathだけを実行すること。
- `POST /reboot` と `POST /shutdown` は、署名済みJSON本文の `confirm: true` を必須にすること。
- request bodyは128byteに制限し、`confirm` 以外のJSON fieldを拒否すること。
- 認証失敗時のHTTP response bodyは理由を出さず、固定文言だけを返すこと。詳細理由はsecretを含まない内部ログにだけ残す。

## Windows Firewall

m5stack-pc-bridgeの待受ポートはプライベートネットワークに限定します。ルーターのポート開放やUPnPによる公開は行いません。

## Telegram Bot token / allowed user / confirmation nonce

- `TELEGRAM_BOT_TOKEN` と `TELEGRAM_ALLOWED_USER_ID` は `firmware/config.toml` に置き、m5stack-pc-bridge用の `BRIDGE_SHARED_SECRET` とは分離する。Telegramや外部中継先には `BRIDGE_SHARED_SECRET` を渡さない。
- どちらもSerialログ、画面表示、コミット、PR本文には出さない。`config.example.toml` にはダミー値だけを置く。
- `from.id` を文字列として `TELEGRAM_ALLOWED_USER_ID` と厳密一致で比較する。一致しないupdateは処理も返信もしない。`callback_query`(インラインボタン)の `from.id` も同様に厳密一致で検証し、一致しない場合はpending確認を操作せず、`answerCallbackQuery` で短い拒否文言だけ返す(`sendMessage` による通常返信はしない)。
- `/reboot` / `/shutdown` は即実行しない。6文字の確認nonceをRAM上だけに生成し、`TELEGRAM_CONFIRM_TTL_SECS`（秒、既定 60秒）で失効させる。確認メッセージには確定ボタン(再起動/シャットダウン)とキャンセルボタンのインラインキーボードを付ける。
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

## 認証失敗アラートとbridge側のbot token

- m5stack-pc-bridgeは、HTTP認証に失敗したリクエストが3件たまると、Telegramへアラートを送ります(最短送信間隔1時間)。`config.toml` の `telegram_bot_token` と `telegram_chat_id` が両方設定されているときだけ有効で、未設定なら通知しないだけです。
- M5Stack側もTelegramの未許可ユーザーからのアクセスに同じポリシーで通知します。閾値と間隔の正本は `shared/pc-remote-signing` の `AlertThrottle`(`DEFAULT_THRESHOLD` / `DEFAULT_INTERVAL`)で、firmwareとbridgeの両方がこれを使います。値を変えるときはここだけを直します。
- 通知本文には件数だけを書き、送信元IP、ヘッダー値、リクエスト本文は含めません。攻撃者が自由に決められる文字列を自分のチャットへ流すと、なりすましや誘導の材料になるためです。
- 送信URLにはbot tokenが含まれるため、ログにもエラーメッセージにもURLを出しません。
- **bot tokenをWindows側にも置くことを許容する判断(2026-09-03)**。firmware側と同じtokenを `%ProgramData%\m5stack-pc-bridge\config.toml` に置きます。理由は次のとおりです。
  - このファイルには既に `shared_secret` があります。これは電源操作を直接authorizeする、bot tokenより強い鍵です。
  - このファイルを読める攻撃者は既にそのPC上におり、その時点で `shutdown.exe` を直接実行できます。PC制御という観点で新たな能力は増えません。
  - 同じbot tokenは既にM5Stack側のflashに平文で保存されており、実機はflash encryption / secure bootとも無効(`espflash board-info` が `Security features: None`)です。USB接続できれば吸い出せるため、OSログインの後ろにあるWindows PCの方がむしろ保護は強くなります。
  - 増える能力は「Telegramの会話を読める / botとしてメッセージを送れる」の1点です。これはPC制御とは別方向の被害(本人へのフィッシング、コマンドの盗み見)であり、blast radiusを下げたい場合は通知専用の別botを作ってそのtokenだけを置く運用にできます。
- bridgeのTLS検証はrustlsの標準ルート証明書集合(webpki-roots)を使います。firmwareのようなルートCAピン留めはしていません。firmwareは証明書集合を持てない組み込み環境のため1枚に固定していますが、Windows側は通常のルート集合で検証するほうがCAローテーション時に壊れにくいためです。

## 監査ログ

- m5stack-pc-bridgeは、認証成功かつ `confirm: true` の `POST /reboot` / `POST /shutdown` だけを実行ファイル横の `audit.log` へ記録します。
- 記録項目は時刻、操作種別、`dry_run`、結果のみです。`shared_secret`、署名、nonce、リクエスト本文、Telegram tokenは記録しません。
- 操作前の監査ログ追記に失敗した場合、reboot/shutdownは実行せず `500` を返します。
- `audit.log` は約1MBで `audit.log.1` へ1世代ローテーションします。長期保存が必要な場合はWindows側で別途退避してください。
