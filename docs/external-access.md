# External Access Design

スマートフォンから外出先で操作するための設計です。最優先条件は **コストゼロ** です。月額課金、従量課金、無料枠超過で課金される可能性がある構成は採用しません。

ユーザー環境は賃貸の無料回線で、自宅ルーターのVPN機能を前提にできません。そのため、外部から自宅LANへ直接入る方式ではなく、常時稼働しているM5Stack Core2が外向きHTTPSでコマンドを取りに行く方式にします。

## 結論

第一候補は **Telegram Bot API long polling方式** です。

```text
Smartphone
  ↓ Telegram app
Telegram Bot API
  ↓ outbound HTTPS long polling
M5Stack Core2
  ↓ WOL or HMAC-signed LAN HTTP
Windows PC
```

自宅側でポート開放は不要です。Windows Agentの管理ポートもLAN内限定のままです。Cloudflare Worker、VPS、課金型キュー、課金型DBは使いません。

## なぜ中継が必要か

賃貸無料回線やCGNAT配下では、外出先スマホから自宅LAN内のM5Stackへ直接到達できないことが多いです。PCがOFFのときはWindows上のソフトウェアも動かないため、PC側にTailscaleやTunnelを入れてもWAKE用途には使えません。

この条件でWAKEを成立させるには、常時給電のM5Stackが外向きにアクセスできる無料の中継先が必要です。

## 絶対条件

- 月額課金サービスを使わない。
- 従量課金サービスを使わない。
- 支払い方法登録が必要なサービスを運用必須経路にしない。
- 無料枠を超えると課金されるサービスを運用必須経路にしない。
- Windows Agentをインターネットへ直接公開しない。
- M5StackまたはWindows Agentの認証を弱めない。
- スマホ外部操作でもREBOOT/SHUTDOWNは確認操作を必須にする。

## 採用案: Telegram Bot API long polling

M5Stackが `getUpdates` を定期実行またはlong pollingし、許可ユーザーからのメッセージだけをコマンドとして扱います。結果通知は `sendMessage` で同じチャットへ返します。

### コマンド

- `/status`
- `/wake`
- `/reboot`
- `/shutdown`

`/reboot` と `/shutdown` は即実行しません。M5Stackは確認メッセージを返し、短時間だけ有効な確認nonceを生成します。

例:

```text
/shutdown
-> Confirm shutdown: /confirm_shutdown 8K3P2Q
```

確認nonceはM5StackのRAM上だけに保持し、期限切れ・使用済み・不一致を拒否します。

### 認可

Telegramの `from.id` をallow-listします。

```cpp
#define TELEGRAM_ALLOWED_USER_ID "123456789"
```

許可ユーザー以外のupdateは無視し、返信もしません。Bot tokenが漏れた場合はBotFatherで即時revokeします。

### secret

M5Stackのローカル設定に以下を追加します。

- `TELEGRAM_BOT_TOKEN`
- `TELEGRAM_ALLOWED_USER_ID`

既存の `AGENT_SHARED_SECRET` とは分離します。

- `TELEGRAM_BOT_TOKEN`: M5StackとTelegram Bot APIの間で使う。
- `AGENT_SHARED_SECRET`: M5StackとWindows Agentの間だけで使う。

Telegramや外部中継先には `AGENT_SHARED_SECRET` を渡しません。

## STATUS

外部STATUSはM5Stackがその場でPCへICMP pingし、結果をTelegramへ返します。

応答例:

```text
PC: ONLINE
Wi-Fi RSSI: -58 dBm
M5Stack IP: 192.168.1.50
Last check: 2026-09-01 00:00:00 JST
```

## WAKE

`/wake` はM5Stackから既存のWake-on-LAN処理を呼びます。PCがOFFでもM5Stackが生きていれば実行できます。

## REBOOT / SHUTDOWN

`/reboot` / `/shutdown` は以下の二段階です。

1. Telegramで操作要求を受ける。
2. M5Stackが確認nonce付きメッセージを返す。
3. 許可ユーザーが `/confirm_reboot <nonce>` または `/confirm_shutdown <nonce>` を送る。
4. M5StackがWindows Agentへ既存のHMAC署名付きPOSTを送る。

Windows Agent側の `confirm: true` 必須条件は維持します。

## poll間隔

初期値:

- `getUpdates` timeout: 20秒
- エラー時バックオフ: 5秒から60秒
- STATUS自動更新: 既存UIの間隔を維持

通常時のリクエスト数は1日数千回程度になります。Telegram側のレート制限に当たった場合は、操作を失敗として扱い、課金回避のため有料オプションには進みません。

## 不採用案

### ルーターVPN

自宅ルーターのVPN機能がある場合は最も堅い案ですが、今回の環境ではルーターを前提にできないため採用しません。

### Cloudflare Worker / Durable Object

初期の運用必須経路としては採用しません。2026-09-01時点でWorkersやDurable ObjectsにはFree planがありますが、無料枠やプラン条件に依存する設計は「コストゼロを絶対守る」という条件に対して弱いです。

### Tailscale

初期の運用必須経路としては採用しません。2026-09-01時点でPersonal planは無料ですが、外部サービスの無料プランに依存します。またPCがOFFのときはWindows上のTailscaleも動かないため、WAKE用途の主経路にはなりません。

### ntfy.sh public topic

採用しません。無料で試せますが、public topic前提では操作コマンドの秘匿と認可を別途自前で強く作る必要があります。REBOOT/SHUTDOWN用途ではTelegramのユーザーID allow-listの方が扱いやすいです。

### Windows Agentの直接公開

採用しません。HMAC認証があっても、shutdown/rebootを実行する管理ポートをインターネットへ直接置く必要はありません。

## 実装フェーズ

### Phase 5A: Telegram Bot設計 (実装済み)

- BotFatherでbotを作る。
- bot tokenと自分のTelegram user idを取得する。
- `firmware/include/config.example.h` にTelegram設定テンプレートを追加する。
- READMEにセットアップ手順を書く。

### Phase 5B: M5Stack Telegram client (実装済み)

- `getUpdates` で `/status` と `/wake` を処理する。
- `sendMessage` で結果を返す。
- 許可ユーザー以外を無視する。
- tokenやuser idをログに出さない。

### Phase 5C: 危険操作の二段階確認 (実装済み)

- `/reboot` と `/shutdown` で確認nonceを発行する。
- `/confirm_reboot <nonce>` / `/confirm_shutdown <nonce>` を実装する。
- nonceは短時間で期限切れにする。
- Windows Agentへの既存HMAC署名付きPOSTを呼ぶ。

### Phase 5D: 実機確認 (未実施)

- スマホのTelegramから `/status`。
- PC OFF状態で `/wake`。
- PC ON状態で `/reboot`。
- PC ON状態で `/shutdown`。
- すべて結果を `HANDOFF.md` に記録する。

## 実装済み範囲 (2026-09-01時点)

- `firmware/src/telegram_client.h` / `telegram_client.cpp` が `getUpdates` によるlong pollingを、`xTaskCreatePinnedToCore` でcore 0の専用FreeRTOSタスクとして実行する。タッチUI/STATUS更新のメインloop(core 1)を長時間ブロックしない。
- `TELEGRAM_BOT_TOKEN` / `TELEGRAM_ALLOWED_USER_ID` が未設定(placeholderまたは空)の場合はタスクを起動せず、画面表示は `Telegram: disabled` になる。既存のタッチUI・WOL・STATUSはそのまま動作する。
- `from.id` が `TELEGRAM_ALLOWED_USER_ID` と一致しないupdateは無視し、返信しない。`update_id` は次回 `offset` として保持し、再処理しない。起動直後の最初の1バッチは、オフライン中に来たコマンドを実行しないよう、offset調整のみ行い実行はしない。
- `/status` はキャッシュ済みのPC ONLINE/OFFLINE(既存のICMP ping結果)、Wi-Fi RSSI、M5Stack local IPを返す。
- `/wake` は既存の `sendWakeOnLan()` を呼び、成功/失敗を返信後にSTATUSを再取得する。
- `/reboot` / `/shutdown` は即実行せず、6文字の確認nonce(RAM上のみ、`TELEGRAM_CONFIRM_TTL_MS` でTTL)を発行し、`/confirm_reboot <nonce>` / `/confirm_shutdown <nonce>` を案内する。
- 確認コマンドはnonce一致・TTL内・action一致のときだけ実行し、既存の `postAgentCommand("/reboot")` / `postAgentCommand("/shutdown")` (HMAC署名付き)を呼ぶ。成功/失敗に関わらずnonceを消費し、再利用・ブルートフォースを防ぐ。
- HTTP失敗時は5秒から60秒への指数バックオフを行い、画面状態を `Telegram: error` にする。
- Telegram APIとの通信はTLS (`WiFiClientSecure::setCACert()`) でサーバー証明書チェーンを検証する。ルートCAは `firmware/src/telegram_root_ca.h` に埋め込んだ「Go Daddy Root Certificate Authority - G2」(2037-12-31まで有効な自己署名ルート)を使う。`api.telegram.org` の葉証明書自体はおよそ年1回更新されるが、ルートCAを固定していれば葉証明書の更新だけでは検証は壊れない。
- Wake-on-LAN送信・Windows Agentへの署名付きPOST・PC ping結果は `firmware/src/power_controller.h` / `power_controller.cpp` に切り出し、タッチUIとTelegramタスクの両方から呼べるようにした。共有するWiFiUDPソケットとPC状態フラグへの同時アクセスは、内部の1つのFreeRTOSミューテックスで直列化している。Windows Agentへの `postAgentCommand()` はこのミューテックスを保持したままHTTP通信するため、`setConnectTimeout(3000)` / `setTimeout(3000)` で明示的に短いtimeoutを設定し、Agentが応答しない場合でもタッチUIやTelegramタスクを長時間ブロックしないようにしている。

## CA証明書のローテーション運用

- `firmware/src/telegram_root_ca.h` にピン留めしているのはリーフ証明書ではなくルートCA(有効期限2037-12-31)。Telegramがリーフ証明書だけを通常更新している間は、このファイルの更新は不要。
- ただし、Telegramが将来ルートCAごと切り替えた場合(認証局の変更、ルート更新など)、TLSハンドシェイクが失敗し始める。症状はM5Stackの画面が `Telegram: polling` から `Telegram: error` に変わり、Serialログに `telegram getUpdates failed` 系のメッセージが出ることで気づける。
- 復旧手順: `openssl s_client -connect api.telegram.org:443 -servername api.telegram.org -showcerts` で現在のチェーンを取得し直し、新しいルート証明書のPEMで `firmware/src/telegram_root_ca.h` の `TELEGRAM_ROOT_CA_PEM` を差し替え、ファイル先頭のコメント(subject、有効期限、SHA-256 fingerprint、取得日)も更新したうえで再ビルド・再書き込みする。
- token自体はURLに埋め込まれるが、証明書検証によって経路上の第三者による通信内容の盗聴・改ざん耐性が確保される。

## 未実装 / 残リスク

- **実機・実Telegram疎通は確認済み。** 実bot token・実user idを`firmware/include/config.h`(Git管理外)へ設定し、M5Stack Core2実機へ書き込み後、`/status`、`/wake`、`/reboot`+`/confirm_reboot <nonce>`、`/shutdown`+`/confirm_shutdown <nonce>` が動作することを2026-09-01 JSTに確認済み。
- **ルートCAのハードコード運用に依存する。** 上記「CA証明書のローテーション運用」のとおり、Telegramがルート認証局を切り替えた場合は手動でのfirmware更新が必要になる。監視や自動更新の仕組みはない。
- Telegram Bot APIの仕様や制限が将来変わる可能性はある。コストが発生する変更が必要になった場合は採用しない。
- Bot tokenが漏れると第三者がbot APIへアクセスできる。BotFatherでrevokeし、M5Stackのconfigを更新する。
- Telegramアカウントが乗っ取られると許可ユーザーとして操作される。スマホ側のロックとTelegramの二段階認証を有効にする。
- M5StackがOFFLINEなら外部操作はできない。
- `/status` のPC ONLINE/OFFLINEは既存のSTATUS更新間隔(`STATUS_INTERVAL_MS`)でキャッシュされた値を返す。Telegramからのリクエストのたびに即時pingはしていないため、直近の状態変化から最大 `STATUS_INTERVAL_MS` 程度のずれがありうる(`/wake` 実行後は明示的に再取得する)。

## 参照

- Telegram Bot API: https://core.telegram.org/bots/api
- WireGuard: https://www.wireguard.com/
- Cloudflare Workers pricing: https://developers.cloudflare.com/workers/platform/pricing/
- Cloudflare Durable Objects pricing: https://developers.cloudflare.com/durable-objects/platform/pricing/
- Tailscale pricing: https://tailscale.com/pricing
