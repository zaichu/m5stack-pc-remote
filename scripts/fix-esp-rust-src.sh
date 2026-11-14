#!/usr/bin/env bash
set -euo pipefail

esp_toolchain="${ESP_TOOLCHAIN:-esp}"
host_toolchain="${RUST_SRC_TOOLCHAIN:-1.100.0}"
esp_sysroot="$(rustc "+$esp_toolchain" --print sysroot)"
host_sysroot="$(rustc "+$host_toolchain" --print sysroot)"
esp_root="$esp_sysroot/lib/rustlib/src/rust/library"
host_root="$host_sysroot/lib/rustlib/src/rust/library"

if [[ ! -d "$esp_root" ]]; then
  echo "ERROR: $esp_toolchain toolchainのrust-srcがありません。espupでrust-src component付きで導入してください。" >&2
  exit 1
fi
if [[ ! -d "$host_root" ]]; then
  echo "ERROR: Rust $host_toolchain のrust-srcがありません。rustup component add rust-src --toolchain $host_toolchain を実行してください。" >&2
  exit 1
fi

for package in windows-sys windows_link; do
  if [[ ! -f "$host_root/$package/Cargo.toml" ]]; then
    echo "ERROR: Rust $host_toolchain rust-srcにlibrary/$packageがありません。" >&2
    exit 1
  fi
  if [[ ! -f "$esp_root/$package/Cargo.toml" ]]; then
    cp -a "$host_root/$package" "$esp_root/$package"
    echo "Rust $host_toolchain rust-srcからlibrary/$packageを補いました。"
  fi
done
