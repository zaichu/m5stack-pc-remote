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

build_log="$(mktemp)"
cleanup() {
  rm -f "$build_log"
}
trap cleanup EXIT

set +e
cargo +esp build --release --target "$target" >"$build_log" 2>&1
status=$?
set -e

if grep -E -q '[0-9]{6,}:[A-Za-z0-9_-]{20,}' "$build_log"; then
  echo "ERROR: Rust firmware build logにTelegram bot tokenらしき文字列を検出しました。" >&2
  echo "secretを含む可能性があるため、build log本文は表示しません。" >&2
  exit 1
fi

if command -v python3 >/dev/null 2>&1; then
  if python3 "$repo_root/scripts/check-firmware-rust-build-log-secrets.py" \
    "$poc_dir/config.toml" "$build_log"; then
    :
  else
    exit 1
  fi
else
  echo "WARNING: python3 が見つからないため、config.toml値とのbuild log照合をskipします。" >&2
fi

cat "$build_log"
exit "$status"
