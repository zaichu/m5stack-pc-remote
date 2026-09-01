#!/usr/bin/env bash
# Fails fast if the old "secrets as Rust source" config file exists.
#
# firmware-rust-poc used to keep local secrets in src/config.rs, a plain Rust
# module with `pub const` string literals. A compiler warning (e.g. an unused
# import) can print the offending source line, which leaked a Telegram bot
# token/user id into build logs (Issue #21). The fix moved secrets into a
# git-ignored config.toml that build.rs reads at build time; this script
# blocks a build before it can happen again on a machine that still carries
# the old file, without ever printing that file's contents.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
legacy_config="firmware-rust-poc/src/config.rs"

if [[ -e "$repo_root/$legacy_config" ]]; then
  echo "ERROR: $legacy_config still exists." >&2
  echo "This file put secrets directly into Rust source, where a compiler warning could print them into build logs (Issue #21)." >&2
  echo "Migrate its values into firmware-rust-poc/config.toml (see firmware-rust-poc/config.example.toml), then delete this file." >&2
  echo "Its contents are not shown here because they may contain real secrets." >&2
  exit 1
fi

exit 0
