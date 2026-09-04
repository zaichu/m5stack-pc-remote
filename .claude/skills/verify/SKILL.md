---
name: verify
description: Run m5stack-pc-remote quality gates before reporting completion, preparing a commit, or opening a PR.
---

# Verify

このスキルが検証コマンドの正本です。`Makefile` の `check` ターゲットが実体です。

## フル品質ゲート

```bash
make check
```

`bash -n scripts/*.sh` は `make check` に含まれるようになったため、個別に実行する必要はありません。

`make check` は以下を実行します（`Makefile` が正本）。

- `git diff --check`
- `bash scripts/check-staged-secret-paths.sh`
- `bash scripts/scan-secrets.sh` — gitleaks があれば git履歴を含めて走査し、無ければ既知パターンの fallback（CI では gitleaks 必須）
- `python3 scripts/config_keys.py check` — firmware設定キー対応を `firmware/build.rs` と `firmware/src/app_config.rs` から導出し、`firmware/README.md` のNVS対応表と突合する
- `cargo fmt --manifest-path m5stack-pc-bridge/Cargo.toml --check`
- `cargo clippy --manifest-path m5stack-pc-bridge/Cargo.toml --all-targets -- -D warnings`
- `cargo test --manifest-path m5stack-pc-bridge/Cargo.toml`（`shared/pc-remote-signing` は `make test` で明示的に実行）
- `cargo build --manifest-path m5stack-pc-bridge/Cargo.toml --release --target x86_64-pc-windows-gnu`（`cfg(windows)` のため通常の test/clippy には含まれず、mingw 有無で skip）
- `cargo +esp build --release --target xtensa-esp32-espidf`（esp toolchain / `firmware/config.toml` が無い環境では警告して skip）
- `bash -n scripts/*.sh` と `shellcheck scripts/*.sh`（shellcheck が無い環境では警告して skip）

## secret パターン検査

```bash
bash scripts/scan-secrets.sh
```

実Wi-Fiパスワード、実PC MACアドレス、HMAC secret、Windows認証情報を出力やdocsへ残さないでください。
Rust firmwareではsecretをRustソース(`firmware/src/`)へ直接書かず、Git管理外の `firmware/config.toml` を使ってください。旧 `firmware/src/config.rs` / `firmware/src/_config.rs` はビルドログ漏えい防止のため禁止です。
