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
- 外部操作の将来経路:
  - `Smartphone -> Cloudflare Worker等 -> M5Stack Core2 -> Windows PC`
  - Windows Agentを直接インターネットへ公開しない

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
- 未検証: WAKE(Wake-on-LAN)の実地確認(PCがONの状態では意味のあるテストができないため)、`dry_run = false`にした実際のSHUTDOWN/REBOOT実行。次セッションでの確認事項とする。
- Windows Agentの配布・常駐化を`windows-agent/install.ps1`(新規)に統合した。バイナリ配置、`config.toml`未作成時のexampleからの生成(ランダムshared_secret付き)、Windows Firewallの受信許可ルール作成、Scheduled Task登録(`SYSTEM`権限・起動時トリガー)を一括で行う。対になる`uninstall.ps1`も追加し、旧`install-scheduled-task.ps1`/`uninstall-scheduled-task.ps1`は置き換えて削除した。
- ユーザーから「Task SchedulerではなくWindowsサービス化したら?」という提案があったが、AGENTS.md/CLAUDE.mdで「Windowsサービス化方針」はCodexレビュー前提と定められているため、今回は実装せず据え置いた。次回、Codexで設計レビューしてから着手する別タスクとする(`windows-service`クレート導入、SCMコールバック対応、ログ出力方式変更などが必要になる想定)。
- NICの`Wake on Magic Packet`/`Shutdown Wake-On-Lan`はドライバ側で有効になっていることを確認済み(`Get-NetAdapterAdvancedProperty`)。`config.h`の`PC_MAC_ADDRESS`/`PC_IP_ADDRESS`/`WOL_BROADCAST_ADDRESS`が実機と一致していることも確認済み(値自体はログに残していない)。

## 次のセッションへの依頼例

```text
AGENTS.md、CLAUDE.md、HANDOFF.mdを読んでから続きの作業をしてください。
まず git status、make check、必要なら pio run -d firmware を確認してください。
```
