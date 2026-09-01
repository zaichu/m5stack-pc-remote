#!/usr/bin/env bash
# Builds the Rust firmware PoC (Issue #16) when the environment can.
#
# Skips with a warning instead of failing when the Xtensa Rust toolchain or the
# local secrets file is missing, mirroring how `make firmware-build` behaves
# without PlatformIO. This keeps `make check` usable on machines (and in CI)
# that do not carry the multi-GB ESP-IDF toolchain.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
poc_dir="$repo_root/firmware-rust-poc"
target="xtensa-esp32-espidf"

if [[ ! -d "$poc_dir" ]]; then
  echo "WARNING: $poc_dir not found; skipping Rust firmware PoC build."
  exit 0
fi

# Blocks on the old secrets-in-Rust-source file (Issue #21) without printing
# its contents. Runs even when the esp toolchain is missing below, so CI (and
# any machine without it) still catches a leftover legacy file.
bash "$repo_root/scripts/check-local-firmware-rust-secrets.sh"

if ! command -v cargo >/dev/null 2>&1; then
  echo "WARNING: cargo not found; skipping Rust firmware PoC build."
  exit 0
fi

if ! rustup toolchain list 2>/dev/null | grep -q '^esp'; then
  echo "WARNING: 'esp' Rust toolchain not installed (see espup); skipping Rust firmware PoC build."
  exit 0
fi

if ! command -v ldproxy >/dev/null 2>&1; then
  echo "WARNING: ldproxy not found (cargo install ldproxy); skipping Rust firmware PoC build."
  exit 0
fi

if [[ ! -f "$poc_dir/config.toml" ]]; then
  echo "WARNING: $poc_dir/config.toml not found (copy config.example.toml); skipping Rust firmware PoC build."
  exit 0
fi

# espup writes the toolchain environment here; sourcing it is what makes the
# Xtensa clang/gcc visible to the esp-idf-sys build script.
if [[ -f "$HOME/export-esp.sh" ]]; then
  # shellcheck disable=SC1091
  source "$HOME/export-esp.sh" >/dev/null 2>&1 || true
fi

echo "Building Rust firmware PoC for $target"
cd "$poc_dir"
cargo +esp build --release --target "$target"
