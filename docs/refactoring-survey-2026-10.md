# リファクタリング調査 (Issue #244 段階1)

> 一時的な文書。シリーズの最後の PR で消す。

## 調査範囲と方法

- Issue #244 の段階1 (調査のみ)。製品コード・テスト・設定・他の文書は変更していない。
- 対象: `firmware/`, `m5stack-pc-bridge/`, `shared/`。
- 実機・実ネットワーク・実 PC のシャットダウン/再起動は未使用。ホスト側 fmt/clippy/test のみ。
- 電源操作の確認処理と HMAC・timestamp・nonce・ユーザー認可は弱める提案をしていない。

## 共通 crate (`shared/`) の利用状況

- `pc-remote-signing`: 署名・検証・OTA manifest の型と検証・`AlertThrottle`・`BootChecks`。firmware と bridge の両方から使用。
- `config-validation`: IP/port/brightness/timezone の検証、status addr 組み立て。firmware と bridge の設定検証で使用。
- `battery`: 残量計算・電源状態通知の安定判定・IRQ レジスタ定義。firmware から使用。
- `wake-check`: 起動待機・スリープ抑止の純粋ロジック。firmware から使用。

firmware と bridge の間で、shared に置かれていない重複した知識 (同じビジネスロジックの別実装) は見つからなかった。`PowerAction` の表示文言は wire protocol ではないため firmware 側 (`bridge_client.rs` の `PowerActionLabel`) に残しているが、これは意図的な配置であり重複ではない。

## 認証まわりの確認結果

確認した範囲:

- `shared/pc-remote-signing`: canonical 文字列は `METHOD\nPATH\nTIMESTAMP\nNONCE\nSHA256(BODY)`。method は大文字化、path と body hash を含むため、署名の別 endpoint への使い回し (cross-protocol confusion) を防ぐ構造。
- `m5stack-pc-bridge/src/auth.rs` `verify_request`: timestamp skew は `now.unix_timestamp().abs_diff(timestamp)` で `u64` 比較 (i64 オーバーフロー回避済み)。署名検証の後に nonce を `insert_once` する順序。nonce は charset/長さ (<=64) / 上限 (10,000 件) を検証。
- `NonceStore::evict_expired`: `now.abs_diff(*timestamp) <= ttl_seconds`。timestamp に極端値が入ってもオーバーフローしない。
- HMAC 比較は `hmac::Mac::verify_slice` (定数時間) を使用。
- bridge `server.rs`: 電源操作 (`/reboot` `/shutdown` `/sleep`) と firmware 配信 (`/firmware`, `/firmware/manifest`) は HMAC 認証必須。`CommandRequest` は `deny_unknown_fields` + `confirm == true` 必須。body 上限 128 バイト。
- firmware `bridge_client::send_command`: NTP 未同期時は送信前にエラー (timestamp 検証が通らないため)。nonce はハードウェア乱数 + 単調時刻の組み合わせ。
- firmware Telegram: `telegram_user_id_matches` で許可ユーザー以外の update を拒否。`/reboot` `/shutdown` `/sleep` `/update` と設定変更は単回使用 nonce 付き確認を経由し、成功・失敗・期限切れのいずれでも消費 (`consume_pending`)。起動直後の最初の `getUpdates` 結果は `initial_sync_done` が false の間は offset 更新のみで実行しない。
- 操作ロック (`OperationLock`): `ALLOWED_WHILE_LOCKED` の 4 コマンド以外は default-deny。ロック中は本体パネルの電源ボタンも拒否 (`reject_locked`)。

確認した範囲では、HMAC・timestamp・nonce・ユーザー認可に実害のある欠陥は見つからなかった。`/status` のみ無認証だが、電源操作権限を伴わず生存確認のみを返す設計であり、本調査では変更提案に含めない (「やらないと決めたもの」参照)。

## 問題一覧

| 順 | 問題 | ファイルパスと行 | 根拠 | 重要度 | 影響範囲 | 変更リスク | 期待効果 | 実機での確認が要るか |
|---|---|---|---|---|---|---|---|---|
| 1 | STATUS 疎通確認 (TCP connect_timeout 800ms) が UI スレッドをブロックし、タッチ処理・画面更新が最大 800ms 止まる | `firmware/src/main.rs:447-451`, `firmware/src/net.rs:24,159-179` | `status_at.elapsed() >= STATUS_INTERVAL` の周期で `net::check_pc_online(..., STATUS_PROBE_TIMEOUT)` を呼び、内部で `TcpStream::connect_timeout(800ms)`。PC オフ/到達不能時に毎 10 秒ブロック。`TOUCH_POLL_INTERVAL` は 20ms | 中 | firmware UI 応答性 | 中 (別スレッド化は ESP32 のヒープ・mbedTLS 制約と競合) | タッチ取りこぼし・画面更新遅延の低減 | 要 (タッチ応答・CPU 負荷) |
| 2 | `main.rs` の単一 `loop` に Wi-Fi / バッテリー / STATUS / タッチ / スリープ / OTA 自己診断が集中し、状態遷移が追いにくい | `firmware/src/main.rs:138-312` (初期化), `312-877` (loop) | `Screen`, `Status`, `notified_online`, `notify_streak`, `power_notify`, `backlight_on`, `calendar_day` 等の可変状態が 1 関数に集中。純粋ロジックは shared へ切り出されているが、遷移の組み立ては host テスト不能 | 中 | firmware 保守性・テスト容易性 | 中〜高 (挙動維持の回帰リスク大) | 状態遷移の可読性・host テスト可能範囲の拡大 | 要 (起動〜スリープまで) |
| 3 | Wi-Fi 再接続とオンラインサービス起動の状態が `Option`/`bool` の複数フラグで暗黙に管理される | `firmware/src/main.rs:61-137` (`start_online_services`), `314-360` (再接続分岐) | `wifi: Option<Wifi>`, `sntp: Option`, `telegram_started: bool`, `notifier: Option`, `status.wifi_connected` を個別更新。`None` 経路では `unsafe { Modem::new() }` で再取得 (`net.rs:75-83`)。遷移条件が散在 | 中 | firmware 起動・再接続・Telegram | 高 (Wi-Fi/Telegram 停止の回帰リスク) | 再接続挙動の明確化・テスト容易化 | 要 (切断・再接続) |
| 4 | Telegram `getUpdates` が最大 32KB を毎ポーリングで `Vec` に蓄積し、JSON 全体を parse する | `firmware/src/telegram.rs:43` (`RESPONSE_MAX_BYTES`), `1880-1917` | `body.extend_from_slice(&chunk[..read])` で最大 32KiB、その後 `serde_json::from_slice(&body)`。ESP32 ヒープ上のピーク使用 | 中 | firmware メモリ (ヒープ) | 中 (streaming parse は大規模変更) | ピークヒープ使用量の低減 | 要 (ヒープ計測) |
| 5 | bridge が firmware 配信時にファイル全体 (数 MB) を `Vec<u8>` に読み込んで応答する | `m5stack-pc-bridge/src/firmware.rs:36-37` (`read_bin`), `server.rs` `firmware_binary` | `std::fs::read(&paths.bin)` で全読み込み。manifest 組み立て (`build_manifest`) も同じ。OTA 配信時のピークメモリ | 中 | bridge メモリ (Windows) | 中 (axum のストリーミング応答への切替) | 配信時のメモリ使用量の低減 | 一部 (Windows 実機でのメモリ観測) |
| 6 | `NonceStore` の 10,000 件上限と 100 件ごとの eviction により、大量リクエスト時に正当な要求が `Replay` で拒否される可能性 | `m5stack-pc-bridge/src/auth.rs:24-64` | `evict_expired` は `seen.len().is_multiple_of(100)` のときのみ実行。上限到達時は `insert_once` が即 `false` を返す | 低 | bridge 認証 (DoS 耐性) | 低〜中 | 認証の可用性向上 | 不要 (unit test で再現可) |
| 7 | `net::wait_for_time_sync` が Telegram スレッドを起動時に最大 10 秒ブロックする | `firmware/src/net.rs:194-213`, `firmware/src/telegram.rs` `Client::run` | `loop` + `std::thread::sleep(250ms)` を最大 10 秒。電源操作は未同期時計で拒否されるため妥当だが、起動直後の Telegram 応答が遅れる | 低 | firmware 起動時の Telegram 応答 | 低 (非ブロッキング化は trivial) | 起動直後の応答性 | 要 (起動時間) |
| 8 | `check_pc_online` のホスト名解決が UI スレッドをブロックする (NVS にホスト名が残っている場合) | `firmware/src/net.rs:159-179` | `addr_text.to_socket_addrs()` を UI スレッドで実行。コード内コメントで IP リテラル推奨とされているが、旧設定の互換経路として残る | 低 | firmware UI (旧設定ユーザー) | 低 | 旧設定でも UI が固まらない | 要 (旧 NVS データ) |

## やらないと決めたもの

- **HMAC・timestamp・nonce・ユーザー認可の緩和**: 「守ること」に反するため、一切提案しない。電源操作の確認処理を弱める変更は行わない。
- **shared crate への重複移動**: 署名・検証・OTA manifest・config 検証・battery/wake-check は既に shared にあり、firmware/bridge 間で重複した知識は見つからなかった。移動は実害が確認できないためやらない。
- **行数削減・ファイル細分化だけを目的とした分割**: KISS/YAGNI に反するためやらない。分割は責務の実害 (テスト不能・状態遷移の不明瞭さ) がある箇所に限る。
- **`/status` の認証化**: 生存確認のみを返し、電源操作権限を伴わない既存設計。変更は互換性を壊すだけで実害が確認できないため、今回やらない (将来 Issue で検討)。
- **SPI/DMA バッファ (4096B) の縮小**: 転送回数増加の実害が確認できないためやらない。
- **実ネットワーク・実 PC シャットダウン/再起動を使ったテストの追加**: 規約 (テストに実 PC を使わない) に反するためやらない。host 側の純粋関数テストとループバックに留める。
- **firmware の `config.toml` / 秘密情報の Git 追加**: 規約違反のためやらない。

## 次のステップ (段階2以降)

効果の大きい順 (上表の順) に小さな PR で実施する。各 PR でホスト側 fmt/clippy/test を通し、実機検証が必要なものは PR 本文に未検証事項として明記する。完了時に未検証事項と残存リスクを報告する。
