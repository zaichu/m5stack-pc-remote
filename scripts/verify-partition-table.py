#!/usr/bin/env python3
"""ビルド後、実際に生成されたパーティションテーブルが firmware/partitions.csv と
一致することを確認する。

`build.rs` が `CARGO_MANIFEST_DIR` から `partitions.csv` の絶対パスを生成済み
`sdkconfig.defaults` へ追加する。設定の解決に失敗しても、ESP-IDFのビルドは
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
PARTITION_TABLE_BIN_GLOB = (
    FIRMWARE_DIR / "target" / "xtensa-esp32-espidf" / "release" / "build" / "esp-idf-sys"
)


def find_partition_table_bins() -> list[Path]:
    return sorted(
        PARTITION_TABLE_BIN_GLOB.glob("*/out/build/partition_table/partition-table.bin")
    )


def select_partition_table_bin(argv: list[str]) -> Path:
    """検証対象の partition-table.bin を選ぶ。

    明示パス(ファイルまたは OUT_DIR)が与えられればそれを使う。無ければ
    ビルド成果物のうち更新時刻が最新のものを選び、辞書順の最後に依存しない。

    成果物が見つからない場合は FileNotFoundError を送出する(呼び出し側で
    失敗として扱う)。明示パスが存在しない場合も自動検索へは切り替えず、
    そのまま FileNotFoundError を送出する。
    """
    if argv:
        candidate = Path(argv[0])
        if candidate.is_dir():
            matches = sorted(candidate.rglob("partition-table.bin"))
            if not matches:
                raise FileNotFoundError(f"partition-table.bin が見つかりません: {candidate}")
            # 更新時刻が最新のものを優先する。
            return max(matches, key=lambda p: p.stat().st_mtime)
        if not candidate.exists():
            raise FileNotFoundError(f"partition-table.bin が見つかりません: {candidate}")
        return candidate
    bins = find_partition_table_bins()
    if not bins:
        raise FileNotFoundError(
            "partition-table.bin が見つかりません。先に firmware をビルドしてください。"
        )
    return max(bins, key=lambda p: p.stat().st_mtime)


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


def main(argv: list[str] | None = None) -> int:
    if argv is None:
        argv = sys.argv[1:]

    if not PARTITIONS_CSV.exists():
        print(f"ERROR: {PARTITIONS_CSV} が見つかりません。", file=sys.stderr)
        return 2

    try:
        selected = select_partition_table_bin(argv)
    except FileNotFoundError as e:
        print(f"ERROR: {e}", file=sys.stderr)
        return 2
    if not selected.exists():
        print(
            "ERROR: esp-idf-sysの生成物partition-table.binが見つかりません。"
            "先に `cargo +esp build --release --target xtensa-esp32-espidf` を実行してください。",
            file=sys.stderr,
        )
        return 2

    try:
        gen_tool = find_gen_esp32part()
    except FileNotFoundError as e:
        print(
            f"ERROR: {e} パーティションテーブルを検証できませんでした。",
            file=sys.stderr,
        )
        return 3

    result = subprocess.run(
        [sys.executable, str(gen_tool), str(selected)],
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
            "  CONFIG_PARTITION_TABLE_CUSTOM_FILENAMEのパス解決に失敗し、"
            "ダミーのpartition_tableにfallbackした可能性があります。"
            "firmware/sdkconfig.defaultsのコメントを確認してください。",
            file=sys.stderr,
        )
        return 1

    print(f"partition table OK ({len(expected)} partitions, {selected})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
