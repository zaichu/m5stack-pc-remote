# Phases

## Phase 1: Wi-Fi -> WOL -> STATUS

- Rust firmwareプロジェクトを使う。
- Wi-Fiへ接続する。
- 設定ファイルからPCのMAC、IP、broadcast addressを読む。
- Wake-on-LAN Magic Packetを送る。
- TCP connect probeでONLINE/OFFLINEを判定する。
- 画面には最低限のSTATUSとWAKEボタンを表示する。

## Phase 2: M5Stack UI

- ONLINE/OFFLINEを大きく表示する。
- Wi-Fi接続状態、IP、RSSIを表示する。
- WAKE、REBOOT、SHUTDOWNボタンを表示する。
- REBOOT / SHUTDOWN は確認画面を必須にする。

## Phase 3: m5stack-pc-bridge

- Windows起動時にWindows Serviceとして自動起動できるようにする。
- `/status`、`/reboot`、`/shutdown` を提供する。
- HMAC-SHA256、timestamp、nonceでPOSTコマンドを認証する。
- dry-runを初期値にし、設定変更なしで実電源操作を行わない。

## Phase 4: M5Stack -> Agent連携

- M5StackからAgentへ署名付きHTTPリクエストを送る。
- REBOOT / SHUTDOWNの確認UIを実装する。
- 成功/失敗を画面に表示する。

## Phase 5: 外部操作

- [External Access Design](external-access.md) を正本にする。
- コストゼロを絶対条件にする。
- ルーターVPNを前提にできないため、初期案はTelegram Bot API long polling方式にする。
- M5Stackに `/status`、`/wake`、`/reboot`、`/shutdown` のTelegram command処理を追加する。
- `TELEGRAM_BOT_TOKEN` と `TELEGRAM_ALLOWED_USER_ID` を追加し、m5stack-pc-bridge用の `AGENT_SHARED_SECRET` と分離する。
- 外部からも WAKE、STATUS、REBOOT、SHUTDOWN を扱えるようにする。
