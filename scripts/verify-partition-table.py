#!/usr/bin/env python3
"""ビルド後、実際に生成されたパーティションテーブルが firmware/partitions.csv と
一致することを確認する。

`firmware/sdkconfig.defaults` の `CONFIG_PARTITION_TABLE_CUSTOM_FILENAME` は、
embuildがesp-idf-sysのビルド時に生成する合成CMakeプロジェクト(target/配下の
一時ディレクトリ)を基準にした相対パスで `partitions.csv` を指す(詳細は
sdkconfig.defaultsのコメントを参照)。この解決に失敗しても、ESP-IDFのビルドは
致命的エラーにはならず、空のダミーpartition_tableへ静かにfallbackする。
つまり「ビルドが成功する」ことは「意図したOTA用パーティション構成が実際に
使われている」ことを保証しない。このスクリプトはビルド成果物を直接検証して
そのギャップを埋める。

実機は不要。`cargo build` が生成した partition-table.bin を、ESP-IDF自身の
gen_esp32part.py でデコードして比較する(バイナリ形式を自前で再実装しない)。
"""

from __future__ import annotations

import csv
import glob
import re
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
FIRMWARE_DIR = REPO_ROOT / "firmware"
PARTITIONS_CSV = FIRMWARE_DIR / "partitions.csv"
# embuildがビルドのたびに `target/<triple>/<profile>/` 直下へコピーする、
# hashディレクトリに依存しない固定パス。
PARTITION_TABLE_BIN = FIRMWARE_DIR / "target" / "xtensa-esp32-espidf" / "release" / "partition-table.bin"


def parse_csv_rows(text: str) -> list[tuple[str, int]]:
    """(name, size) の列を返す。offsetは自動計算のため比較対象にしない。"""
    entries = []
    for row in csv.reader(text.splitlines()):
        if not row or row[0].strip().startswith("#") or not row[0].strip():
            continue
        name = row[0].strip()
        entries.append((name, parse_size(row[4].strip())))
    return entries


def parse_size(text: str) -> int:
    if text.lower().startswith("0x"):
        return int(text, 16)
    match = re.fullmatch(r"(\d+)([KkMm]?)", text)
    if not match:
        raise ValueError(f"partitions.csv: sizeを解釈できません: {text!r}")
    value = int(match.group(1))
    unit = match.group(2).lower()
    if unit == "k":
        value *= 1024
    elif unit == "m":
        value *= 1024 * 1024
    return value


def find_gen_esp32part() -> Path:
    # esp-idf-sysが取得するESP-IDFのバージョンは複数併存し得るため、
    # インストール済みのものをどれか1つ見つければよい(ツール自体はversion間で安定)。
    candidates = sorted(
        glob.glob(
            str(
                FIRMWARE_DIR
                / ".embuild/espressif/esp-idf/*/components/partition_table/gen_esp32part.py"
            )
        )
    )
    if not candidates:
        raise FileNotFoundError(
            "gen_esp32part.py が見つかりません。esp toolchainが未導入か、"
            "一度もビルドしていない可能性があります。"
        )
    return Path(candidates[0])


def main() -> int:
    if not PARTITIONS_CSV.exists():
        print(f"ERROR: {PARTITIONS_CSV} が見つかりません。", file=sys.stderr)
        return 2
    if not PARTITION_TABLE_BIN.exists():
        print(
            f"ERROR: {PARTITION_TABLE_BIN} が見つかりません。"
            "先に `cargo +esp build --release --target xtensa-esp32-espidf` を実行してください。",
            file=sys.stderr,
        )
        return 2

    try:
        gen_tool = find_gen_esp32part()
    except FileNotFoundError as e:
        print(f"WARNING: {e} パーティションテーブル検証をskipします。", file=sys.stderr)
        return 0

    result = subprocess.run(
        [sys.executable, str(gen_tool), str(PARTITION_TABLE_BIN)],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        print(f"ERROR: gen_esp32part.py がpartition-table.binを解析できませんでした:\n{result.stderr}", file=sys.stderr)
        return 1

    expected = parse_csv_rows(PARTITIONS_CSV.read_text(encoding="utf-8"))
    actual = parse_csv_rows(result.stdout)

    if expected != actual:
        print("ERROR: ビルドされたpartition-table.binがfirmware/partitions.csvと一致しません。", file=sys.stderr)
        print(f"  期待値 (partitions.csv): {expected}", file=sys.stderr)
        print(f"  実際値 (partition-table.bin): {actual}", file=sys.stderr)
        print(
            "  CONFIG_PARTITION_TABLE_CUSTOM_FILENAMEの相対パス解決に失敗し、"
            "ダミーのpartition_tableにfallbackした可能性があります。"
            "firmware/sdkconfig.defaultsのコメントを確認してください。",
            file=sys.stderr,
        )
        return 1

    print(f"partition table OK ({len(expected)} partitions, {PARTITION_TABLE_BIN})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
