#!/usr/bin/env python3
"""Rust firmwareのbuild logへローカルsecret値が出ていないか検査する。

値そのものはstdout/stderrへ出さない。
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover
    import tomli as tomllib  # type: ignore[no-redef]


SENSITIVE_KEYS = {
    "wifi_ssid",
    "wifi_password",
    "pc_mac_address",
    "agent_shared_secret",
    "bridge_shared_secret",
    "pc_ip_address",
    "pc_status_addr",
    "telegram_bot_token",
    "telegram_allowed_user_id",
}

TOKEN_PATTERN = re.compile(r"[0-9]{6,}:[A-Za-z0-9_-]{20,}")


def main() -> int:
    if len(sys.argv) != 3:
        print(
            "usage: check-firmware-build-log-secrets.py <config.toml> <build.log>",
            file=sys.stderr,
        )
        return 2

    config_path = Path(sys.argv[1])
    log_path = Path(sys.argv[2])

    config = tomllib.loads(config_path.read_text(encoding="utf-8"))
    log = log_path.read_text(encoding="utf-8", errors="replace")

    if TOKEN_PATTERN.search(log):
        print(
            "ERROR: Rust firmware build logにTelegram bot tokenらしき文字列を検出しました。",
            file=sys.stderr,
        )
        print("secretを含む可能性があるため、build log本文は表示しません。", file=sys.stderr)
        return 1

    leaked_keys: list[str] = []
    for key in sorted(SENSITIVE_KEYS):
        value = config.get(key)
        if value is None:
            continue
        text = str(value)
        if len(text) < 4 or text.startswith("replace-with-") or text.startswith("your-"):
            continue
        if text in log:
            leaked_keys.append(key)

    if leaked_keys:
        print(
            "ERROR: Rust firmware build logにローカルconfigの値が出力されています: "
            + ", ".join(leaked_keys),
            file=sys.stderr,
        )
        print("secretを含む可能性があるため、build log本文は表示しません。", file=sys.stderr)
        return 1

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
