# firmware-rust-poc

M5Stack Core2 for AWS firmwareをRustで書けるか検証するPoC。Issue #16参照。

`firmware/`(PlatformIO + Arduino Framework + M5Unified、C++)が本線。このディレクトリは
その置き換えではなく、実現性を確かめるための独立したcargoプロジェクト。PoCが
Issue #16の成功条件を満たすまで、`firmware/` は変更しない。

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
| Wi-Fi / NVS / イベントループ | `esp-idf-svc` |
| GPIO / SPI / I2C | `esp-idf-hal` |

### なぜM5Unified(C++)を使わないか

当初は `m5unified` crate(M5Unified C++ライブラリのRustラッパー)を使い、画面表示の
実機動作までは確認できた。しかしM5UnifiedはESP-IDFの**旧I2Cドライバ**を使うため、
Wi-Fi等でESP-IDFのモダンなドライバ(driver_ng)が同一バイナリにリンクされると、
起動時に必ず `CONFLICT! driver_ng is not allowed to be used with this old driver` で
abortする。切り分けの詳細はIssue #16のコメントを参照。

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
cd firmware-rust-poc
cp src/config.example.rs src/config.rs   # Git管理外。実際の値に書き換える
. ~/export-esp.sh
cargo build --release --target xtensa-esp32-espidf
```

## 書き込み・モニタ

```bash
espflash flash --monitor target/xtensa-esp32-espidf/release/m5remote-rust-poc
```

WSL2から書き込む場合はusbipd-winでUSBデバイスをアタッチしてから実行する。

## 現在のスコープと実機確認結果

Issue #16のPoC成功条件に対する進捗:

- [x] ESP32 / M5Stack Core2へRust firmwareをbuild/flashできる
- [x] Wi-Fi接続できる
- [x] STATUS相当の疎通確認ができる(ICMPではなくTCP connectプローブ)
- [x] Core2画面にONLINE/OFFLINEとWi-Fi状態を表示できる
- [x] Wake-on-LAN Magic Packetを送信できる(コード実装済み)
- [ ] タッチ操作でWAKEできる(実機でのタップ確認が未実施)
- [x] 秘密情報をGitへ入れない構成を維持できる(`src/config.rs`はGit管理外)
- [ ] `make check` または専用verifyにRust firmware PoCのbuild確認を追加

2026-09-01時点の実機確認: AXP192初期化、ディスプレイ初期化、タッチコントローラー
初期化、Wi-Fi接続、STATUS疎通(PCのONLINE検出)まで動作を確認済み。
