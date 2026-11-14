import subprocess
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPTS_DIR = REPO_ROOT / "scripts"

import importlib.util

_SPEC = importlib.util.spec_from_file_location(
    "verify_partition_table", SCRIPTS_DIR / "verify-partition-table.py"
)
vpt = importlib.util.module_from_spec(_SPEC)
assert _SPEC.loader is not None
_SPEC.loader.exec_module(vpt)


def _make_csv(path: Path) -> Path:
    path.write_text(
        "nvs,data,nvs,,0x4000,\n"
        "ota_0,app,ota_0,,2M,\n",
        encoding="utf-8",
    )
    return path


def test_select_raises_when_no_artifacts(tmp_path, monkeypatch):
    monkeypatch.setattr(vpt, "PARTITION_TABLE_BIN_GLOB", tmp_path / "missing-glob-root")
    with pytest.raises(FileNotFoundError):
        vpt.select_partition_table_bin([])


def test_select_explicit_missing_file_raises(tmp_path):
    missing = tmp_path / "does-not-exist.bin"
    with pytest.raises(FileNotFoundError):
        vpt.select_partition_table_bin([str(missing)])


def test_select_explicit_missing_dir_raises(tmp_path):
    d = tmp_path / "empty-dir"
    d.mkdir()
    with pytest.raises(FileNotFoundError):
        vpt.select_partition_table_bin([str(d)])


def test_select_explicit_existing_file_returns_it(tmp_path):
    f = tmp_path / "partition-table.bin"
    f.write_bytes(b"x")
    assert vpt.select_partition_table_bin([str(f)]) == f


def test_select_explicit_dir_returns_newest(tmp_path):
    d = tmp_path / "build"
    d.mkdir()
    older = d / "a" / "partition-table.bin"
    newer = d / "b" / "partition-table.bin"
    older.parent.mkdir()
    newer.parent.mkdir()
    older.write_bytes(b"old")
    newer.write_bytes(b"new")
    import os
    import time

    now = time.time()
    os.utime(older, (now - 10, now - 10))
    os.utime(newer, (now, now))
    assert vpt.select_partition_table_bin([str(d)]) == newer


def test_select_no_arg_returns_newest(tmp_path, monkeypatch):
    root = tmp_path / "root"
    root.mkdir()
    g = root / "esp-idf-sys"
    g.mkdir()
    bin_path = g / "out" / "build" / "partition_table" / "partition-table.bin"
    bin_path.parent.mkdir(parents=True)
    bin_path.write_bytes(b"x")
    monkeypatch.setattr(vpt, "PARTITION_TABLE_BIN_GLOB", root)
    # glob pattern expects */out/build/...; emulate by monkeypatching find function instead
    monkeypatch.setattr(vpt, "find_partition_table_bins", lambda: [bin_path])
    assert vpt.select_partition_table_bin([]) == bin_path


def test_main_missing_artifact_returns_2(tmp_path, monkeypatch):
    csv_path = _make_csv(tmp_path / "partitions.csv")
    monkeypatch.setattr(vpt, "PARTITIONS_CSV", csv_path)
    monkeypatch.setattr(vpt, "PARTITION_TABLE_BIN_GLOB", tmp_path / "no-such")
    monkeypatch.setattr(vpt, "find_partition_table_bins", lambda: [])
    rc = vpt.main([])
    assert rc == 2


def test_main_missing_gen_tool_returns_3(tmp_path, monkeypatch, capsys):
    csv_path = _make_csv(tmp_path / "partitions.csv")
    bin_path = tmp_path / "partition-table.bin"
    bin_path.write_bytes(b"x")
    monkeypatch.setattr(vpt, "PARTITIONS_CSV", csv_path)
    monkeypatch.setattr(vpt, "find_gen_esp32part", lambda: (_ for _ in ()).throw(FileNotFoundError("gen_esp32part.py が見つかりません")))
    rc = vpt.main([str(bin_path)])
    assert rc == 3
    captured = capsys.readouterr()
    assert "検証できませんでした" in captured.err or "検証できなかった" in captured.err


def test_main_success_returns_0(tmp_path, monkeypatch):
    csv_path = _make_csv(tmp_path / "partitions.csv")
    bin_path = tmp_path / "partition-table.bin"
    bin_path.write_bytes(b"x")
    tool_path = tmp_path / "gen_esp32part.py"
    tool_path.write_text("# stub\n", encoding="utf-8")
    monkeypatch.setattr(vpt, "PARTITIONS_CSV", csv_path)
    monkeypatch.setattr(vpt, "find_gen_esp32part", lambda: tool_path)

    expected_stdout = (
        "# ESP-IDF Partition Table\n"
        "# Name, Type, SubType, Offset, Size, Flags\n"
        "nvs,data,nvs,0x9000,16K,\n"
        "ota_0,app,ota_0,0x10000,2M,\n"
    )

    class FakeResult:
        returncode = 0
        stdout = expected_stdout
        stderr = ""

    monkeypatch.setattr(
        vpt.subprocess, "run", lambda *a, **k: FakeResult()
    )
    rc = vpt.main([str(bin_path)])
    assert rc == 0


def test_main_mismatch_returns_1(tmp_path, monkeypatch):
    csv_path = _make_csv(tmp_path / "partitions.csv")
    bin_path = tmp_path / "partition-table.bin"
    bin_path.write_bytes(b"x")
    tool_path = tmp_path / "gen_esp32part.py"
    tool_path.write_text("# stub\n", encoding="utf-8")
    monkeypatch.setattr(vpt, "PARTITIONS_CSV", csv_path)
    monkeypatch.setattr(vpt, "find_gen_esp32part", lambda: tool_path)

    class FakeResult:
        returncode = 0
        stdout = (
            "# ESP-IDF Partition Table\n"
            "# Name, Type, SubType, Offset, Size, Flags\n"
            "nvs,data,nvs,0x9000,8K,\n"
            "ota_0,app,ota_0,0x10000,2M,\n"
        )
        stderr = ""

    monkeypatch.setattr(vpt.subprocess, "run", lambda *a, **k: FakeResult())
    rc = vpt.main([str(bin_path)])
    assert rc == 1


def test_explicit_missing_does_not_fallback(tmp_path, monkeypatch):
    # artifact exists in search path but explicit path missing -> must fail, not use fallback
    csv_path = _make_csv(tmp_path / "partitions.csv")
    fallback_bin = tmp_path / "fallback.bin"
    fallback_bin.write_bytes(b"x")
    monkeypatch.setattr(vpt, "PARTITIONS_CSV", csv_path)
    monkeypatch.setattr(vpt, "find_partition_table_bins", lambda: [fallback_bin])
    missing = tmp_path / "missing-explicit.bin"
    rc = vpt.main([str(missing)])
    assert rc == 2