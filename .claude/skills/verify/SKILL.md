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
- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- `pio run -d firmware`。PlatformIO CLI がない環境では警告のみ。

## secret パターン検査

```bash
bash scripts/scan-secrets.sh
```

実Wi-Fiパスワード、実PC MACアドレス、HMAC secret、Windows認証情報を出力やdocsへ残さないでください。
