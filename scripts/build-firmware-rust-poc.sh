#!/usr/bin/env bash
# Rust firmwareを、環境が揃っている場合だけbuildする。
#
# Xtensa Rust toolchainやローカル設定が無い端末ではfailではなく警告でskipする。
# ESP-IDF toolchainを持たないCI/開発端末でも `make check` を使えるようにするため。
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
poc_dir="$repo_root/firmware-rust-poc"
target="xtensa-esp32-espidf"

if [[ ! -d "$poc_dir" ]]; then
  echo "WARNING: $poc_dir が見つからないため、Rust firmware buildをskipします。"
  exit 0
fi

# 旧方式のsecret入りRustソースが残っていないか、内容を表示せず先に確認する。
# esp toolchainが無い端末でもこの検査だけは実行する。
bash "$repo_root/scripts/check-local-firmware-rust-secrets.sh"

if ! command -v cargo >/dev/null 2>&1; then
  echo "WARNING: cargo が見つからないため、Rust firmware buildをskipします。"
  exit 0
fi

if ! rustup toolchain list 2>/dev/null | grep -q '^esp'; then
  echo "WARNING: 'esp' Rust toolchain が未導入のため、Rust firmware buildをskipします。"
  exit 0
fi

if ! command -v ldproxy >/dev/null 2>&1; then
  echo "WARNING: ldproxy が見つからないため、Rust firmware buildをskipします。"
  exit 0
fi

if [[ ! -f "$poc_dir/config.toml" ]]; then
  echo "WARNING: $poc_dir/config.toml が見つからないため、Rust firmware buildをskipします。"
  exit 0
fi

# espupが出力する環境変数を読み、esp-idf-sys build scriptからXtensa toolchainを見えるようにする。
if [[ -f "$HOME/export-esp.sh" ]]; then
  # shellcheck disable=SC1091
  source "$HOME/export-esp.sh" >/dev/null 2>&1 || true
fi

echo "Rust firmwareをbuildします: $target"
cd "$poc_dir"
cargo +esp build --release --target "$target"
