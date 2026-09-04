#!/usr/bin/env python3
"""Rust firmware用のNVS設定イメージを生成し、必要なら実機へ書き込む。

config.toml keyとNVS keyの対応はここで持たず、`config_keys.py` が
`firmware/build.rs` と `firmware/src/app_config.rs` から導出したものを正本にする(#76)。
新しい設定キーは源码側(build.rs / app_config.rs)に追加するだけで、このスクリプトの
編集は不要になる。

secret値は表示しない。生成物はsecretを含むためGit管理外に置く。
"""

from __future__ import annotations

import argparse
import csv
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover
    import tomli as tomllib  # type: ignore[no-redef]

sys.path.insert(0, str(Path(__file__).resolve().parent))
from config_keys import ConfigKey, derive_mappings  # noqa: E402


REPO_ROOT = Path(__file__).resolve().parents[1]
FIRMWARE_DIR = REPO_ROOT / "firmware"
DEFAULT_CONFIG = FIRMWARE_DIR / "config.toml"
DEFAULT_OUTDIR = FIRMWARE_DIR / ".nvs-provisioning"
# firmware/partitions.csv のnvs partitionと一致させる(Issue #41/#79、OTA対応の
# カスタムパーティションテーブル)。offsetは変わらないが、sizeはESP-IDF標準の
# OTA構成に合わせて0x6000から0x4000へ縮小した。
DEFAULT_NVS_SIZE = 0x4000
DEFAULT_NVS_OFFSET = 0x9000


def parse_int(value: str) -> int:
    return int(value, 0)


def find_generator() -> tuple[Path, Path]:
    override = os.environ.get("ESP_IDF_NVS_PARTITION_GEN")
    if override:
        generator = Path(override).expanduser()
        if not generator.is_file():
            raise FileNotFoundError(f"ESP_IDF_NVS_PARTITION_GEN が見つかりません: {generator}")
    else:
        candidates = sorted(
            FIRMWARE_DIR.glob(
                ".embuild/espressif/esp-idf/*/components/nvs_flash/"
                "nvs_partition_generator/nvs_partition_gen.py"
            )
        )
        if not candidates:
            raise FileNotFoundError(
                "nvs_partition_gen.py が見つかりません。先に `make firmware-build` を実行してください。"
            )
        generator = candidates[-1]

    python_candidates = sorted(FIRMWARE_DIR.glob(".embuild/espressif/python_env/*/bin/python"))
    python = python_candidates[-1] if python_candidates else Path(sys.executable)
    return python, generator


def load_config(path: Path, keys: tuple[ConfigKey, ...]) -> dict[str, object]:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    apply_toml_aliases(data, keys)
    missing = [key.toml_key for key in keys if not key.optional and key.toml_key not in data]
    if missing:
        raise ValueError("configに不足があります: " + ", ".join(missing))
    validate_config(data)
    return data


def apply_toml_aliases(data: dict[str, object], keys: tuple[ConfigKey, ...]) -> None:
    """build.rsのKEYSが持つ旧TOML key(agent_port等)を新keyへ引き継ぐ。"""
    for key in keys:
        if key.toml_alias and key.toml_key not in data and key.toml_alias in data:
            data[key.toml_key] = data[key.toml_alias]


def validate_config(data: dict[str, object]) -> None:
    # 必須かつ空を許さないキー。derive_mappings() の required 12件のうち、
    # telegram_* は placeholder で無効化できるため除外し、残り 6件を対象にする。
    # 新キーを追加する際は、ここへ追加するか、derive 対象にするかを検討すること。
    non_empty = [
        "wifi_ssid",
        "wifi_password",
        "pc_mac_address",
        "pc_status_addr",
        "bridge_shared_secret",
        "pc_ip_address",
    ]
    empty = [key for key in non_empty if not str(data[key]).strip()]
    if empty:
        raise ValueError("空にできないconfigがあります: " + ", ".join(empty))

    for key in ("wol_port", "bridge_port"):
        value = int(data[key])
        if value < 1 or value > 65535:
            raise ValueError(f"{key} は1..65535で指定してください")

    for key in ("telegram_long_poll_timeout_seconds", "telegram_confirm_ttl_secs"):
        value = int(data[key])
        if value < 1:
            raise ValueError(f"{key} は1以上で指定してください")

    if "daily_report_hour" in data and not -1 <= int(data["daily_report_hour"]) <= 23:
        raise ValueError("daily_report_hour は-1(無効)または0..23で指定してください")

    if "timezone_offset_hours" in data and not -14 <= int(data["timezone_offset_hours"]) <= 14:
        raise ValueError("timezone_offset_hours は-14..14で指定してください")


def write_csv(path: Path, config: dict[str, object], keys: tuple[ConfigKey, ...]) -> None:
    with path.open("w", encoding="utf-8", newline="") as fp:
        writer = csv.writer(fp)
        writer.writerow(["key", "type", "encoding", "value"])
        writer.writerow(["m5remote", "namespace", "", ""])
        for key in keys:
            if key.toml_key not in config:
                # 任意key(defaultを持つkey)がconfigに無いときはNVSへも書かない。
                # 起動時にビルド時configへfallbackする。
                continue
            value = str(config[key.toml_key])
            # 1つ目が正本NVS key、2つ目以降は移行互換の旧key(agent_port等)。
            for nvs_key in key.nvs_keys:
                writer.writerow([nvs_key, "data", "string", value])


def generate_image(
    python: Path,
    generator: Path,
    csv_path: Path,
    image_path: Path,
    size: int,
) -> None:
    command = [
        str(python),
        str(generator),
        "generate",
        str(csv_path),
        str(image_path),
        str(size),
    ]
    result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    if result.returncode != 0:
        print("ERROR: NVSイメージ生成に失敗しました。secret保護のため詳細出力は表示しません。", file=sys.stderr)
        raise SystemExit(result.returncode)


def write_device(port: str, offset: int, image_path: Path) -> None:
    espflash = shutil.which("espflash")
    if not espflash:
        raise FileNotFoundError("espflash が見つかりません")

    command = [
        espflash,
        "write-bin",
        "--port",
        port,
        hex(offset),
        str(image_path),
    ]
    result = subprocess.run(command)
    if result.returncode != 0:
        raise SystemExit(result.returncode)


def main() -> int:
    parser = argparse.ArgumentParser(description="Rust firmware用NVS設定を生成する")
    parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    parser.add_argument("--outdir", type=Path, default=DEFAULT_OUTDIR)
    parser.add_argument("--size", type=parse_int, default=DEFAULT_NVS_SIZE)
    parser.add_argument("--offset", type=parse_int, default=DEFAULT_NVS_OFFSET)
    parser.add_argument("--write", action="store_true", help="生成したNVSイメージを実機へ書き込む")
    parser.add_argument("--yes", action="store_true", help="NVS partition上書きを明示承認する")
    parser.add_argument("--port", help="書き込み先シリアルポート。例: /dev/ttyUSB0")
    args = parser.parse_args()

    if args.write and (not args.yes or not args.port):
        print("--write には --yes と --port が必要です。", file=sys.stderr)
        return 2

    if args.size % 4096 != 0:
        print("--size は4096の倍数にしてください。", file=sys.stderr)
        return 2

    config_path = args.config.resolve()
    outdir = args.outdir.resolve()
    outdir.mkdir(parents=True, exist_ok=True)
    os.chmod(outdir, 0o700)

    keys = derive_mappings()
    config = load_config(config_path, keys)
    python, generator = find_generator()
    image_path = outdir / "m5remote-nvs.bin"

    with tempfile.TemporaryDirectory(prefix="m5remote-nvs-") as temp_dir:
        csv_path = Path(temp_dir) / "m5remote-nvs.csv"
        write_csv(csv_path, config, keys)
        generate_image(python, generator, csv_path, image_path, args.size)

    os.chmod(image_path, 0o600)
    print(f"NVSイメージを生成しました: {image_path}")
    print(f"NVS offset: {hex(args.offset)}, size: {hex(args.size)}")

    if args.write:
        print("NVS partitionへ書き込みます。secret値は表示しません。")
        sys.stdout.flush()
        write_device(args.port, args.offset, image_path)
        print("NVS partitionへの書き込みが完了しました。")
    else:
        print("実機へ書き込む場合は --write --yes --port <PORT> を付けて再実行してください。")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
