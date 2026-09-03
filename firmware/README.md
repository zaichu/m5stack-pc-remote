# firmware

M5Stack Core2 for AWS firmware(Rust実装)。旧C++/Arduino/M5Unified版は
Rust版の実運用検証を経て削除済み(#24)。主要機能(Wi-Fi / WOL / STATUS / UI /
REBOOT / SHUTDOWN / Telegram)は実機で動作確認済み。

## 技術構成

`esp-idf-sys`/`esp-idf-svc`/`esp-idf-hal`(std、`binstart`)ベースの **純Rust** 実装。
既存のM5Unified(C++)には依存しない。

| 役割 | クレート |
|---|---|
| 電源管理(AXP192) | `axp192` |
| ディスプレイ(ILI9342C) | `mipidsi` |
| 描画 | `embedded-graphics` |
| タッチ(FT6336U) | `ft6x36` |
| I2Cバス共有 | `embedded-hal-bus` |
| Wi-Fi / NVS / SNTP / HTTP(S) | `esp-idf-svc` |
| HMAC-SHA256 署名 | `hmac` / `sha2` / `hex` |
| Telegram JSON | `serde_json` |
| GPIO / SPI / I2C | `esp-idf-hal` |

### なぜM5Unified(C++)を使わないか

当初は `m5unified` crate(M5Unified C++ライブラリのRustラッパー)を検討し、画面表示の
実機動作までは確認できた。しかしM5UnifiedはESP-IDFの**旧I2Cドライバ**を使うため、
Wi-Fi等でESP-IDFのモダンなドライバ(driver_ng)が同一バイナリにリンクされると、
起動時に必ず `CONFLICT! driver_ng is not allowed to be used with this old driver` で
abortするため採用しない(詳細は #16)。

純Rustスタックにすることで、リンクされるI2Cドライバが1系統に揃い、この競合が
構造的に発生しなくなる。

## ハードウェア定義(M5GFXのCore2 autodetect実装に準拠)

- LCD ILI9342C 320x240: MOSI=23, MISO=38(未使用), SCLK=18, DC=15, CS=5、SPI 40MHz
  half-duplex/write-only
- LCDリセット: AXP192 GPIO4 / LCD電源: AXP192 LDO2 3300mV / バックライト: AXP192 DCDC3 2800mV
- タッチ FT6336U: I2C 0x38、INT=39
- AXP192: I2C 0x34(タッチと同一バス SDA=21, SCL=22 @400kHz)

## 前提ツール

```bash
cargo install espup ldproxy espflash
espup install --targets esp32
. ~/export-esp.sh   # 新しいターミナルを開くたびに必要
```

## セットアップ

```bash
cd firmware
cp config.example.toml config.toml   # Git管理外。実際の値に書き換える
. ~/export-esp.sh
cargo build --release --target xtensa-esp32-espidf
```

秘密情報は `config.toml`(Git管理外、TOML)にだけ書く。Rustソース(`src/`配下)へ
secretを直接書かない。`build.rs` がビルド時に `config.toml` を読み、
`$OUT_DIR/generated_config.rs` を生成して `src/main.rs` が `include!()` で取り込む。
生成した定数は未使用でも `#[allow(dead_code)]` により警告が出ないため、コンパイラの
unused警告がソース行としてbot token等をビルドログへ出す事故が起きない。旧方式の
`src/config.rs` が残っていると `scripts/check-local-firmware-secrets.sh`
(`make firmware-build` から自動実行)がbuildを止める。

起動時はESP-IDF NVSの `m5remote` namespaceを先に読み、存在するkeyだけ実行時設定へ
反映する。NVSが未設定ならビルド時configをそのまま使うため、既存のbuild/flash運用は
維持される。

対応するNVS key:

| NVS key | config.toml key | 用途 |
|---|---|
| `wifi_ssid` | `wifi_ssid` | Wi-Fi SSID |
| `wifi_pass` | `wifi_password` | Wi-Fi password |
| `pc_mac` | `pc_mac_address` | Wake-on-LAN送信先MACアドレス |
| `wol_port` | `wol_port` | Wake-on-LAN送信先port |
| `status_addr` | `pc_status_addr` | STATUS確認先TCP address |
| `bridge_port` | `bridge_port` | m5stack-pc-bridge port |
| `bridge_secret` | `bridge_shared_secret` | m5stack-pc-bridge HMAC secret |
| `pc_ip` | `pc_ip_address` | m5stack-pc-bridge接続先IP |
| `tg_token` | `telegram_bot_token` | Telegram bot token |
| `tg_user_id` | `telegram_allowed_user_id` | 許可するTelegram user id |
| `tg_poll_secs` | `telegram_long_poll_timeout_seconds` | Telegram long polling timeout |
| `tg_ttl_secs` | `telegram_confirm_ttl_secs` | 再起動/シャットダウン確認TTL |
| `report_hour` | `daily_report_hour` | 定期レポートを送るローカル時刻(0-23、範囲外で無効) |
| `tz_offset` | `timezone_offset_hours` | UTCからのローカル時刻のずれ(JSTなら9) |

この表は `scripts/config_keys.py` が `firmware/build.rs` と `src/app_config.rs` から
導出する対応と機械的に突合される(`make config-key-check`、`make check`に含まれる)。
源码と表のどちらか片方にkeyを足しただけではCIが通らない。

NVS上では全keyを文字列として保存する。`wol_port`、`bridge_port`、
`telegram_long_poll_timeout_seconds`、`telegram_confirm_ttl_secs`、
`daily_report_hour`、`timezone_offset_hours` は起動時に数値へ変換する。
既存NVSに残る `agent_port` / `agent_secret` は移行互換として読み込む。

現時点の正本運用は `config.toml` 更新後に再build/flashする方式。NVS provisioningのみで
secretを差し替える手順は、NVS partitionを書き換えてもWi-Fiや実機起動に影響しないことを
確認してから運用手順に昇格する。

NVSイメージ生成:

```bash
cd ..
make firmware-nvs-image
```

生成先は `firmware/.nvs-provisioning/m5remote-nvs.bin`。secretを含むため
Git管理外にしている。

実機NVSを書き換える場合:

```bash
python3 scripts/provision-firmware-nvs.py --write --yes --port /dev/ttyUSB0
```

デフォルトは現行partition tableのNVS offset `0x9000`、size `0x6000`。partition tableを
変更した場合は `--offset` と `--size` を指定する。書き込み後は再起動時に
`NVS設定を読み込みました` と表示される。

## 書き込み・モニタ

```bash
espflash flash --monitor target/xtensa-esp32-espidf/release/m5remote-rust
```

WSL2から書き込む場合はusbipd-winでUSBデバイスをアタッチしてから実行する。

### シリアルを自前スクリプトで読むときの注意

ESP32の自動リセット回路はUSB-UARTのDTR/RTSでENとGPIO0を駆動する。pyserial等で
ポートを開閉すると、closeのタイミングでこの2線の状態次第では**ダウンロードモード
(bootloader)のまま止まり、アプリが起動しない**。この状態は画面が真っ暗になるだけで
シリアルにも何も出ないため、故障と紛らわしい。

自前でポートを開く場合は、closeの前に両線を解放する。

```python
ser.dtr = False
ser.rts = False
ser.close()
```

ダウンロードモードで止まってしまった場合は次で復帰する。

```bash
espflash reset --port /dev/ttyUSB0
```

## 実装済み機能

Phase 1相当(Wi-Fi / WOL / STATUS / タッチUI)に加え、既存C++実装の設計を移植した
以下を実装済み:

- **REBOOT / SHUTDOWN**(`src/bridge_client.rs`): m5stack-pc-bridgeへのHMAC-SHA256署名付きPOST。
  canonical文字列 `POST\n{path}\n{timestamp}\n{nonce}\n{sha256hex(body)}` と
  本文 `{"confirm":true}` 必須をC++版と揃えてある。NTP未同期のクロックでは送信前に弾く。
  画面上はPCがONLINEのときだけボタンが出て、確認画面(CANCEL/OK)を必ず経由する。
- **Telegram連携**(`src/telegram.rs`): Bot APIへのアウトバウンドHTTPS long polling。
  `/status` `/wake` `/reboot` `/shutdown` `/confirm_reboot <nonce>`
  `/confirm_shutdown <nonce>` とインラインキーボードによる確認。`from.id` が
  `TELEGRAM_ALLOWED_USER_ID` と一致しない更新は実行しない。確認nonceは単回使用・TTL付きで、
  一致・不一致・期限切れのいずれでも消費する。起動後の最初のバッチはoffsetを進めるだけで
  実行しない。TLSはルートCAをピン留めして検証する(`src/telegram_root_ca.rs`、
  C++版 `telegram_root_ca.h` と同じ証明書)。
  専用スレッドで動かすため、long pollingがタッチUIやSTATUS更新を止めない。
  電源操作はUIスレッドとの間を `Mutex` で直列化する(C++版のFreeRTOSミューテックス相当)。
- **実行時設定変更**(`src/settings.rs`): `/set_ip <ipv4>` `/set_status_addr <host:port>`
  `/set_wol_port <n>` `/settings`(現在値表示)`/confirm_set <nonce>`(手入力フォールバック)。
  対象は `pc_ip_address` / `pc_status_addr` / `wol_port` の3値のみで、REBOOT/SHUTDOWNと
  同じnonce確認フローを経由する。値の検証(`config-validation` crate)は確認発行前に行い、
  NVSへの書き込みに成功したときだけ即時反映する。`wifi_ssid` / `telegram_bot_token` などの
  自己断線し得る値は対象外で、これらはUSB経由のNVS provisioningのまま(#42設計)。
  `/lock`中はREBOOT/SHUTDOWNと同様に変更系コマンドを拒否する。

## 現在のスコープと実機確認結果

初期検証条件は達成済み。現在は実運用検証中:

- [x] ESP32 / M5Stack Core2へRust firmwareをbuild/flashできる
- [x] Wi-Fi接続できる
- [x] STATUS相当の疎通確認ができる(TCP connectプローブ)
- [x] Core2画面にONLINE/OFFLINEとWi-Fi状態を表示できる
- [x] Wake-on-LAN Magic Packetを送信できる(実機のタッチ操作から送信を確認)
- [x] タッチ操作でWAKEできる(実機タップで `touch: x=194 y=194 in_wake_button=true` →
      `WAKE tapped` → `WOL sent` を確認)
- [x] 秘密情報をGitへ入れない構成を維持できる(`config.toml`はGit管理外。Rustソースへ
      secretを直接書かない)
- [x] `make check` にRust firmwareのbuild確認を追加(`make firmware-build`。
      espツールチェーン/ldproxy/`config.toml` が無い環境では警告してスキップ)

2026-09-01時点の実機確認: AXP192初期化、ディスプレイ初期化、タッチコントローラー
初期化(FT6236U、firmware_id=16/panel_id=17をM5GFXの報告値と一致確認)、Wi-Fi接続、
STATUS疎通(PCのONLINE検出)まで動作を確認済み。タッチはFT6x36のレポートレジスタを
5秒ごとにダンプし、未タッチ時に `td_status=0` を正しく読めることまで確認した。

タッチ座標系については、`ft6x36` の `Orientation::Portrait`(ドライバのデフォルト)が
恒等変換であり、Core2のタッチパネルは既にディスプレイと同じ座標系(x 0..319、
y 0..279。y 240..279 は画面下の物理ボタン帯)で報告するため、変換不要と確認した。
WAKEボタンの当たり判定は `y >= 180` で、画面上の緑ボタンと画面下の物理ボタン帯の
どちらのタップでも成立する。

Wi-Fiは15秒ごとにリンク状態を確認し、切断されていれば再接続する(C++版
`connectWifi()` の再試行間隔と同じ方針)。
