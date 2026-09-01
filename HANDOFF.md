# HANDOFF

このファイルは、別の Codex/Claude セッションへ作業状態を引き継ぐためのメモです。恒久的な構成と、時点付きの現在状態を分けて書きます。

## 恒久構成

- リポジトリ: `m5stack-pc-remote`
- M5Stack側: `firmware/`
  - PlatformIO
  - Arduino Framework
  - M5Unified
  - Phase 1 は Wi-Fi、Wake-on-LAN、ICMP ping STATUS
- Windows側: `windows-agent/`
  - Rust
  - HMAC-SHA256認証
  - timestamp + nonceによるリプレイ防止
  - `dry_run = true` を初期値にして、実shutdown/rebootを誤実行しない
- ローカル秘密設定:
  - `firmware/include/config.h`
  - `windows-agent/config.toml`
- 外部操作の経路:
  - `Smartphone -> Telegram Bot API (outbound HTTPS long polling) -> M5Stack Core2 -> Windows PC`
  - Windows Agentを直接インターネットへ公開しない
  - `firmware/src/telegram_client.h` / `telegram_client.cpp` (Telegram long polling、core 0の専用FreeRTOSタスク)
  - `firmware/src/power_controller.h` / `power_controller.cpp` (WOL送信、Windows AgentへのHMAC署名付きPOST、PC ping状態。タッチUIとTelegramタスクの共有ロジック)

## 現在の状態

- 初期プロジェクト骨格を作成済み。
- Phase 1 firmware の最小実装を追加済み。
- firmware にREBOOT/SHUTDOWN確認UIと署名付きAgent POSTの土台を追加済み。
- Rust Windows Agent の認証・設定・dry-run電源操作の土台を追加済み。
- Windows Agent は `POST /reboot` / `POST /shutdown` で署名済みJSON本文の `confirm: true` を必須にする。
- Task SchedulerによるWindows起動時自動起動スクリプトを追加済み。
- `paimon-watch` を参考に、`AGENTS.md`、`CLAUDE.md`、`.claude/skills`、`.githooks`、`Makefile` を整備済み。
- `make check` は成功。Rustテストは10件成功。PlatformIO CLI がこの環境にないため `firmware-build` は警告スキップ。
- `bash -n scripts/*.sh` は成功。
- secretパターンの簡易検索は検出なし。
- Git hooks は `core.hooksPath=.githooks` に設定済み。
- 外部スマホ操作経路は、ユーザーから「コストはゼロにすること。絶対守る」と明示され、さらに「賃貸の無料回線なのでルーター無い」と共有されたため、Cloudflare Worker案とルーターVPN案を下げ、Telegram Bot API long polling方式へ変更した。正本は `docs/external-access.md` と `docs/cost.md`。

## 実機動作確認 (2026-08-31)

- 環境: WSL2 (usbipd-win 5.3.0 で `/dev/ttyUSB0` としてアタッチ) から M5Stack Core2 へ `pio run -d firmware -t upload` で書き込み。
- WSL2内のapt版PlatformIO CLI(4.3.4)がsystem click 8.1.6と非互換で起動不可だったため、`pip3 install --user --break-system-packages platformio` でPlatformIO Core 6.1.19に置き換えた。
- 実機書き込みで2件の不具合を発見・修正済み(`firmware/src/main.cpp`):
  1. `__has_include("config.h")` がESP32ツールチェーン自身の無関係な `sys-include/config.h` を検出してしまい、ローカル `config.h` 未作成時に `config.example.h` へフォールバックせずビルド失敗していた。マクロ未定義判定 (`#ifndef WIFI_SSID`) に変更して修正。
  2. `connectWifi()` の再試行間隔ガードが起動直後の初回呼び出しにも適用され、`WiFi.mode()`が一度も呼ばれないまま `udp.begin()` がlwIPスタック初期化前に実行され `assert failed: tcpip_send_msg_wait_sem ... (Invalid mbox)` でクラッシュループしていた。`wifiConnectStarted` フラグを追加し初回呼び出しはガードを無視するよう修正。
- 修正後、`config.example.h`のダミーSSIDのまま30秒以上クラッシュなしで安定動作(Wi-Fi接続自体はNO_AP_FOUNDで失敗するが想定通り)。
- 実際の2.4GHz帯Wi-Fi設定に切り替えて接続成功。STATUS(ICMP ping)はWindows Firewallの既定設定でICMPv4 Echo Requestがブロックされ100%ロスだったが、
  `netsh advfirewall firewall add rule name="ICMP Allow incoming V4 echo request" protocol=icmpv4:8,any dir=in action=allow` で解消。ONLINE表示・WAKE/REBOOT/SHUTDOWNボタン表示を確認。
- `AGENT_HOST`と`PC_IP_ADDRESS`が別マクロだったため、`config.h`作成時に`PC_IP_ADDRESS`だけ実PCのIPへ書き換えて`AGENT_HOST`をexampleのプレースホルダーのまま放置し、Agentへの接続が`connection refused`になる不具合が発生した。同一PCを指す設定を分離していたこと自体が問題のため、`AGENT_HOST`マクロを廃止して`agentUrl()`は`PC_IP_ADDRESS`を使うよう`firmware/src/main.cpp`・`firmware/include/config.example.h`を修正した。
- Windows Agent(`windows-agent/`)はこのWSL2環境からWindows向けにクロスビルドして実機で動作確認した。WSL2側にはWindows用Rustツールチェーンが無いため、`rustup target add x86_64-pc-windows-gnu` と `sudo apt-get install -y mingw-w64` を追加し、`cargo build --release --target x86_64-pc-windows-gnu` で `pc-remote-agent.exe` を生成。`windows-agent/config.toml`は`firmware/include/config.h`の`AGENT_PORT`/`AGENT_SHARED_SECRET`と値を一致させて作成(値はコミット・ログに残していない)。
- 生成した`.exe`と`config.toml`をPC上の作業ディレクトリへ配置し、`Start-Process`で手動起動して動作確認した(Task Schedulerへの登録はまだ)。Windows Firewallの既定設定で外部ホストからのTCP 18080着信もブロックされていたため、
  `netsh advfirewall firewall add rule name="pc-remote-agent inbound 18080" dir=in action=allow protocol=TCP localport=18080 profile=private` で解消。
- 上記対応後、M5Stack実機からの`SHUTDOWN`/`REBOOT`ボタン操作でエージェントへの署名付きPOSTが両方とも`200`で成功(HMAC認証・confirm必須チェックとも正常)。`dry_run = true`のため実際の電源操作は未実行。
- Windows Agentの配布・常駐化を`windows-agent/install.ps1`(新規)に統合した。バイナリ配置、`config.toml`未作成時のexampleからの生成(ランダムshared_secret付き)、Windows Firewallの受信許可ルール作成、Scheduled Task登録(`SYSTEM`権限・起動時トリガー)を一括で行う。対になる`uninstall.ps1`も追加し、旧`install-scheduled-task.ps1`/`uninstall-scheduled-task.ps1`は置き換えて削除した。実機にインストールし、PC再起動後もScheduled Taskの起動時トリガーで自動的にエージェントが立ち上がることを確認済み。
- ユーザーから「Task SchedulerではなくWindowsサービス化したら?」という提案があったが、AGENTS.md/CLAUDE.mdで「Windowsサービス化方針」はCodexレビュー前提と定められているため、今回は実装せず据え置いた。次回、Codexで設計レビューしてから着手する別タスクとする(`windows-service`クレート導入、SCMコールバック対応、ログ出力方式変更などが必要になる想定)。
- `config.h`の`AGENT_SHARED_SECRET`が`config.example.h`のプレースホルダー文字列のまま変更されておらず、公開リポジトリに載っている既知の値でHMAC認証が通ってしまっていた(認証として機能していない状態)。ランダムな48文字の秘密鍵を生成して`config.h`と`windows-agent/config.toml`の両方に反映した(値はコミット・ログに残していない)。**新規セットアップ時は`AGENT_SHARED_SECRET`を必ずランダム値に変更すること。**
- `dry_run = false`にした上で、M5Stack実機からのREBOOT/SHUTDOWNボタン操作で実際にPCが再起動・シャットダウンすることを確認した。どちらの操作もこのセッション(WSL2)自体を巻き込んで終了するため、セッションを再開しながら検証した。
- WAKE(Wake-on-LAN)は当初、実機で全く起動しなかった。切り分けの過程で2件の不具合を発見・修正した:
  1. `config.h`の`PC_MAC_ADDRESS`がWindowsの`ipconfig`表示形式(ハイフン区切り `XX-XX-XX-XX-XX-XX`)のままで、firmwareの`parseMac()`はコロン区切りを要求するため常にパース失敗し(`invalid PC_MAC_ADDRESS`)、マジックパケット自体が送信されていなかった。コロン区切りに修正。
  2. 上記を直しても起動せず、スマートフォンの別WOLアプリからは同じ2.4GHz Wi-Fiから成功したため切り分けた結果、実ネットワークのサブネットが`/16`(`255.255.0.0`)であるのに`config.h`の`WOL_BROADCAST_ADDRESS`が`/24`前提の値(`x.x.1.255`)になっており、正しい`/16`のブロードキャストアドレス(`x.x.255.255`)ではなかったため、ESP32のlwIPからは通常のユニキャスト宛て(実質どこにも届かない)として送信されていた。スマホアプリは全体ブロードキャスト`255.255.255.255`を使っていたため、サブネットマスクの誤りの影響を受けずに成功していた。恒久対策として`WOL_BROADCAST_ADDRESS`設定自体を廃止し、`firmware/src/main.cpp`は`255.255.255.255`固定で送信するように変更した(サブネットマスクに関わらず動作する)。
- 上記全ての修正後、実機でWi-Fi接続、STATUS(ONLINE表示)、WAKE(WOL)、REBOOT、SHUTDOWNの一連の操作が最初から最後まですべて実際に動作することを確認した。Phase 1のスコープ(Wi-Fi接続 -> Wake-on-LAN -> STATUS、REBOOT/SHUTDOWN)は実機で完全に検証済み。
- NICの`Wake on Magic Packet`/`Shutdown Wake-On-Lan`はドライバ側で有効になっていることを確認済み(`Get-NetAdapterAdvancedProperty`)。BIOS(ASUS TUF GAMING B660M-PLUS D4)側の設定は今回変更していない(スマホからのWOL成功により、ドライバ設定で十分と判明したため)。

## Telegram Bot API 外部操作の実装 (2026-09-01)

- `docs/external-access.md` のTelegram Bot API long polling方式(Phase 5A/5B/5C)をfirmwareに実装した。
- `firmware/src/telegram_client.h` / `telegram_client.cpp` を新規追加。`getUpdates` によるlong pollingをcore 0の専用FreeRTOSタスクとして実行し、タッチUI/STATUS更新(core 1のメインloop)を長時間ブロックしないようにした。
- `firmware/src/power_controller.h` / `power_controller.cpp` を新規追加。既存の`sendWakeOnLan()` / `postAgentCommand()` / PC ping状態を`main.cpp`から切り出し、タッチUIとTelegramタスクの両方から呼べるようにした。共有するWiFiUDPソケットとPC状態フラグへの同時アクセスは内部の1つのFreeRTOSミューテックスで直列化している。
- `/status`、`/wake`、`/reboot`+`/confirm_reboot <nonce>`、`/shutdown`+`/confirm_shutdown <nonce>` を実装。`from.id`が`TELEGRAM_ALLOWED_USER_ID`と一致しないupdateは無視・返信なし。確認nonceはRAM上のみ、`TELEGRAM_CONFIRM_TTL_MS`でTTL、成功/失敗・不一致に関わらず1回で消費(再利用不可)。
- `firmware/include/config.example.h`(および実機用のローカル`config.h`、コミット対象外)に`TELEGRAM_BOT_TOKEN`、`TELEGRAM_ALLOWED_USER_ID`、`TELEGRAM_LONG_POLL_TIMEOUT_SECONDS`、`TELEGRAM_CONFIRM_TTL_MS`を追加。`TELEGRAM_BOT_TOKEN`/`TELEGRAM_ALLOWED_USER_ID`がplaceholderのままだとTelegramタスク自体を起動せず、画面は`Telegram: disabled`になる(既存のタッチUI・WOL・STATUSは無変更で動作)。
- `make check`(Rust fmt/clippy/test + `pio run -d firmware`)、`bash -n scripts/*.sh`、`git diff --check`、secretパターン検索はすべて成功・検出なし。この環境には実際にPlatformIO CLIが入っており`firmware-build`はスキップされず実行された。
- 実bot token・実user idを`firmware/include/config.h`(Git管理外)へ設定し、M5Stack Core2実機へ`pio run -d firmware -t upload --upload-port /dev/ttyUSB0`で書き込み成功。
- 実Telegram疎通の確認結果(2026-09-01 JST):
  - `/status`: botから返信あり。PCのONLINE状態、Wi-Fi RSSI、M5Stack IP、最終確認時刻が返ることを確認。
  - `/wake`: botから`WOL sent`の返信あり。Wake-on-LAN送信コマンドがTelegram経由で呼べることを確認。
  - `/reboot`: 即時再起動せず、`/confirm_reboot <nonce>`の確認コマンド案内が返ることを確認。
  - `/shutdown`: 即時シャットダウンせず、`/confirm_shutdown <nonce>`の確認コマンド案内が返ることを確認。
  - `/confirm_reboot <nonce>`: nonce付き確認コマンド送信後、Windows PCが正常に再起動することを確認。
  - `/confirm_shutdown <nonce>`: nonce付き確認コマンド送信後、Windows PCが正常にシャットダウンすることを確認。

## Codexレビュー対応: TLS証明書検証の有効化 (2026-09-01)

- PR #10のCodexレビューで「`WiFiClientSecure::setInsecure()`によりTelegram Bot APIのサーバー証明書検証を無効化しており、bot tokenがURLに含まれ`/reboot`・`/shutdown`の外部操作経路でもあるためマージ不可」という指摘を受け、対応した。
- `firmware/src/telegram_root_ca.h`を新規追加。`openssl s_client`で実際に`api.telegram.org:443`のTLSチェーンを取得し、葉証明書(GoDaddy発行、約1年で更新)ではなく自己署名のルートCA(「Go Daddy Root Certificate Authority - G2」、有効期限2037-12-31)を埋め込んだ。ファイル内にsubject・有効期限・SHA-256 fingerprint・取得日をコメントで記録済み。
- `firmware/src/telegram_client.cpp`の`sendReply()`・pollingループの両方で`client.setInsecure()`を`client.setCACert(TELEGRAM_ROOT_CA_PEM)`に置き換えた。`setInsecure()`の呼び出しはリポジトリ内に残っていない(コメントの言及のみ)。
- bot token・user idをログへ出さない方針は変更なし(既存のまま維持)。
- ついでの対応として、`firmware/src/power_controller.cpp`の`postAgentCommand()`にHTTPClientの明示timeout(`setConnectTimeout(3000)` / `setTimeout(3000)`)を追加した。この呼び出しは`powerMutex`を保持したまま実行されるため、Windows Agentが応答しない場合でもタッチUI・Telegramタスクを長時間ブロックしないようにする狙い。
- CA証明書のローテーション運用(将来Telegramがルート認証局を切り替えた場合の症状・復旧手順)を`docs/external-access.md`・`docs/security.md`・README.mdに追記した。
- 検証: `pio run -d firmware`(証明書埋め込み後の再ビルド含む)、`make check`、`bash -n scripts/*.sh`、`git diff --check`、secretパターン検索。すべて成功・検出なし。
- 埋め込んだのはルート認証局の公開証明書のみで、秘密情報ではない。

## Telegram inline keyboardによる確認操作の改善 (2026-09-01)

- `firmware/src/telegram_client.cpp` を変更し、`/reboot` / `/shutdown` の確認メッセージにインラインキーボード(確定ボタン「Reboot」/「Shutdown」とCancelボタン)を付けた。ボタンをタップするだけでnonce付き `/confirm_reboot <nonce>` / `/confirm_shutdown <nonce>` を手入力せずに確定・キャンセルできる。
- `callback_data` は `confirm:<reboot|shutdown>:<nonce>` / `cancel:<reboot|shutdown>:<nonce>` の形式(Telegramの1-64byte制限内)。`callback_query` を受けたら `parseCallbackData()` でaction/typeを検証し、既存の `consumePendingConfirm()` (旧 `handleConfirmation` から共通化)でpending nonce/action/TTLと突き合わせる。一致した場合だけ既存の `PowerController::postAgentCommand("/reboot")` / `("/shutdown")` を呼ぶ。
- `callback_query` の `from.id` も `message` と同じく `TELEGRAM_ALLOWED_USER_ID` と厳密一致で検証する。不一致の場合はpending確認を操作せず、`answerCallbackQuery` で短い拒否文言だけ返す(`sendMessage` による通常返信はしない)。
- 成功/失敗/キャンセル/nonce不一致/期限切れのいずれでも `consumePendingConfirm()` がpendingを消費するため、古いボタンの再タップやnonce総当たりは通らない。すべての `callback_query` で `answerCallbackQuery` を呼び、Telegramクライアント側のボタン読み込み状態を終える。
- 既存の `/confirm_reboot <nonce>` / `/confirm_shutdown <nonce>` テキストコマンドは後方互換としてそのまま残した。
- `WiFiClientSecure::setInsecure()` は使っていない(既存どおり `setCACert(TELEGRAM_ROOT_CA_PEM)` を維持)。bot tokenをログへ出す変更もしていない。
- ドキュメント更新: `README.md`、`docs/external-access.md`(Phase 5E追記)、`docs/security.md`。
- 検証: `make check`、`bash -n scripts/*.sh`、`git diff --check`、`rg -n "setInsecure\(" firmware`(該当なし)、secretパターン検索。実機での動作確認(インラインボタンのタップ)は未実施(このセッションでは実機接続なし)。次回実機作業時に `/reboot` と `/shutdown` それぞれでボタンタップによる確定・キャンセルを確認し、`HANDOFF.md` へ結果を追記すること。

## 次のセッションへの依頼例

```text
AGENTS.md、CLAUDE.md、HANDOFF.mdを読んでから続きの作業をしてください。
まず git status、make check、必要なら pio run -d firmware を確認してください。
```
