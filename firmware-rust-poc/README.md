# firmware-rust-poc

M5Stack Core2 for AWS firmwareをRustで書けるか検証するPoC。Issue #16参照。

`firmware/`(PlatformIO + Arduino Framework + M5Unified、C++)が本線。このディレクトリは
その置き換えではなく、実現性を確かめるための独立したcargoプロジェクト。PoCが
Issue #16の成功条件を満たすまで、`firmware/` は変更しない。

## 技術構成

- `esp-idf-sys`(std、`binstart`)を使った std ベースのRust実装(no_stdではない)
- ディスプレイ・タッチ・電源(AXP192)初期化は、既存のM5Unified(C++)を薄くラップした
  [`m5unified`](https://crates.io/crates/m5unified) crateに委譲する。Core2固有の電源投入
  シーケンスをRustで再実装しない方針。
- `components/m5unified-rs/`: `m5unified-sys` 0.3.8のネイティブシムをvendor(経緯は
  同ディレクトリの `VENDORED.md` 参照)。ESP-IDFのコンポーネントマネージャーが
  ビルド時に `m5stack/M5Unified` と `m5stack/M5GFX` を取得する(要ネットワーク接続)。

## 前提ツール

```bash
cargo install espup ldproxy espflash
espup install --targets esp32
. ~/export-esp.sh   # 新しいターミナルを開くたびに必要
```

## ビルド

```bash
cd firmware-rust-poc
. ~/export-esp.sh
cargo build --release
```

初回ビルドはESP-IDF本体とM5Unified/M5GFXコンポーネントのダウンロードが走るため時間がかかる。

## 書き込み・モニタ

```bash
espflash flash --monitor target/xtensa-esp32-espidf/release/m5remote-rust-poc
```

またはPlatformIO版と同様にUSB経由でWSL2にusbipd-winでアタッチしてから実行する。

## 現在のスコープ

`hello from rust` を画面に表示し、シリアルにハートビートログを出すだけの最小サンプル
(`src/main.rs`)。Issue #16の成功条件(Wi-Fi接続、WOL送信、STATUS疎通確認、タッチでの
WAKE)は未実装。まずCore2の画面・電源初期化(AXP192)が `m5unified` crateで実機上動くかを
確認する段階。

## 実機確認結果

- 2026-09-01: M5Stack Core2 for AWS実機でビルド・書き込みし、画面に緑文字で
  「hello from rust」が表示され、シリアルにハートビートログが継続出力される
  ことを確認した。Core2の電源投入(AXP192)・SPIディスプレイ初期化が
  `m5unified` crate経由で成立することを実証できた。
  Wi-Fi接続、WOL送信、STATUS疎通確認、タッチでのWAKEは未実装。
