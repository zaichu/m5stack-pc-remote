---
name: verify
description: Run m5stack-pc-remote quality gates before reporting completion, preparing a commit, or opening a PR.
---

# Verify

このスキルが検証コマンドの正本です。`Makefile` の `check` ターゲットが実体です。

## フル品質ゲート

```bash
make check
bash -n scripts/*.sh
```

`make check` は以下を実行します。

- `git diff --check`
- `bash scripts/check-local-firmware-rust-secrets.sh` (Rust firmwareの旧 `src/config.rs` が残っていれば内容を表示せず停止)
- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- `cargo +esp build --release --target xtensa-esp32-espidf`。esp toolchain / `firmware/config.toml` がない環境では警告のみ。

## secret パターン検査

```bash
bash scripts/scan-secrets.sh
```

実Wi-Fiパスワード、実PC MACアドレス、HMAC secret、Windows認証情報を出力やdocsへ残さないでください。
Rust firmwareではsecretをRustソース(`firmware/src/`)へ直接書かず、Git管理外の `firmware/config.toml` を使ってください。旧 `firmware/src/config.rs` / `firmware/src/_config.rs` はビルドログ漏えい防止のため禁止です。
