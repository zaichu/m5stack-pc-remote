import shutil
import subprocess
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPT = REPO_ROOT / "scripts" / "check-crate-list.sh"

MAKEFILE_STUB = """\
SHARED_CRATES := {crates}
print-shared-crates:
\t@for c in $(SHARED_CRATES); do echo "$$c"; done
"""


def _make_fixture(root: Path, listed: str, dirs: list[str]) -> Path:
    root.mkdir(parents=True, exist_ok=True)
    (root / "Makefile").write_text(
        MAKEFILE_STUB.format(crates=listed), encoding="utf-8"
    )
    scripts = root / "scripts"
    scripts.mkdir(exist_ok=True)
    shutil.copy2(SCRIPT, scripts / "check-crate-list.sh")
    shared = root / "shared"
    for name in dirs:
        d = shared / name
        d.mkdir(parents=True, exist_ok=True)
        (d / "Cargo.toml").write_text('[package]\nname = "x"\n', encoding="utf-8")
    return root


def _run(root: Path) -> subprocess.CompletedProcess[str]:
    env = dict(shutil.os.environ)
    env["REPO_ROOT"] = str(root)
    return subprocess.run(
        ["bash", str(SCRIPT)],
        capture_output=True,
        text=True,
        env=env,
        cwd=REPO_ROOT,
    )


def test_matching_list_passes(tmp_path):
    root = _make_fixture(tmp_path / "ok", "aaa bbb", ["aaa", "bbb"])
    proc = _run(root)
    assert proc.returncode == 0, proc.stderr
    assert "CRATE-LIST OK" in proc.stdout


def test_missing_crate_fails(tmp_path):
    root = _make_fixture(tmp_path / "missing", "aaa", ["aaa", "ccc"])
    proc = _run(root)
    assert proc.returncode != 0
    assert "shared/ccc" in proc.stderr


def test_listed_but_absent_dir_fails(tmp_path):
    root = _make_fixture(tmp_path / "extra", "aaa bbb", ["aaa"])
    proc = _run(root)
    assert proc.returncode != 0
    assert "bbb" in proc.stderr
