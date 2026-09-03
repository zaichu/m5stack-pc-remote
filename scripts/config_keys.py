#!/usr/bin/env python3
"""firmware設定キー対応の導出と整合性チェック。

正本は `firmware/build.rs` の `KEYS` テーブルと `firmware/src/app_config.rs` の
`from_build_config` / `apply_nvs`。この2ファイルから
config.toml key <-> 生成const <-> NVS key の対応を機械的に導出する。

provisionスクリプトはこれを使い、`check` サブコマンドはREADMEの対応表と突き合わせる。
対応表をファイルごとに手で書かなくて済むようにするのが目的(#76)。
扱うのはキー名だけで、設定値(secret)には一切触れない。
"""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
FIRMWARE_DIR = REPO_ROOT / "firmware"

_BUILD_RS = FIRMWARE_DIR / "build.rs"
_APP_CONFIG_RS = FIRMWARE_DIR / "src" / "app_config.rs"
_README_MD = FIRMWARE_DIR / "README.md"


@dataclass(frozen=True)
class ConfigKey:
    toml_key: str
    toml_alias: str | None
    const_name: str
    field: str
    nvs_keys: tuple[str, ...]  # 1つ目が正本、2つ目以降は移行互換の旧NVS key
    optional: bool  # build.rs で .default() を持つ任意key


def _block(text: str, start: str, name: str) -> str:
    match = re.search(start + r"\s*\{(.*?)\n    \}", text, re.DOTALL)
    if match is None:
        raise ValueError(f"{name} 相当のブロックを源码から解析できませんでした")
    return match.group(1)


def parse_build_keys(text: str) -> list[tuple[str, str, str | None, bool]]:
    """build.rsのKEYSを (toml_key, const_name, alias, optional) の列へ。"""
    match = re.search(r"const KEYS: &\[Key\] = &\[(.*?)\n\];", text, re.DOTALL)
    if match is None:
        raise ValueError("build.rs の KEYS テーブルを解析できませんでした")
    entries = []
    for kind, toml_key, const_name, builders in re.findall(
        r"Key::(text|int)\(\s*\"([^\"]+)\",\s*\"([^\"]+)\""
        r"(?:,\s*IntTy::\w+)?"
        r"\s*,?\s*\)"
        r"((?:\s*\.\w+\([^()]*\))*)",
        match.group(1),
        re.DOTALL,
    ):
        alias_match = re.search(r"\.alias\(\"([^\"]+)\"\)", builders)
        entries.append(
            (toml_key, const_name, alias_match.group(1) if alias_match else None, ".default(" in builders)
        )
    if not entries:
        raise ValueError("build.rs の KEYS が空です")
    return entries


def parse_const_to_field(text: str) -> dict[str, str]:
    """from_build_configの `field: build_config::CONST` 対応(const -> field)。"""
    block = _block(text, r"fn from_build_config\(\) -> Self", "from_build_config")
    mapping = dict(
        (const_name, field) for field, const_name in re.findall(r"(\w+):\s*build_config::([A-Z0-9_]+)", block)
    )
    if not mapping:
        raise ValueError("from_build_config の const->field 対応を解析できませんでした")
    return mapping


def parse_field_to_nvs(text: str) -> dict[str, tuple[str, ...]]:
    """apply_nvsの `replace(nvs, &[..], &mut self.field)` 対応(field -> NVS key列)。"""
    block = _block(text, r"fn apply_nvs\(&mut self, nvs: &EspNvs<NvsDefault>\)", "apply_nvs")
    mapping: dict[str, tuple[str, ...]] = {}
    for keys_raw, field in re.findall(
        r"replace\(\s*nvs,\s*&\[([^\]]*)\],\s*&mut\s+self\.(\w+),?\s*\)",
        block,
        re.DOTALL,
    ):
        keys = tuple(re.findall(r"\"([^\"]+)\"", keys_raw))
        if not keys:
            raise ValueError(f"apply_nvs: field `{field}` のNVS key列が空です")
        mapping[field] = keys
    if not mapping:
        raise ValueError("apply_nvs の NVS key 対応を解析できませんでした")
    return mapping


def derive_mappings(firmware_dir: Path | None = None) -> tuple[ConfigKey, ...]:
    """3つの源码を突合して設定キー対応を導出する。対応漏れは例外にする。"""
    firmware_dir = firmware_dir or FIRMWARE_DIR
    build_text = (firmware_dir / "build.rs").read_text(encoding="utf-8")
    app_text = (firmware_dir / "src" / "app_config.rs").read_text(encoding="utf-8")

    build_keys = parse_build_keys(build_text)
    const_to_field = parse_const_to_field(app_text)
    field_to_nvs = parse_field_to_nvs(app_text)

    derived = []
    problems = []
    consts_seen = set()
    for toml_key, const_name, alias, optional in build_keys:
        consts_seen.add(const_name)
        field = const_to_field.get(const_name)
        if field is None:
            problems.append(f"build.rs の `{const_name}` に対応するfieldがfrom_build_configに無い")
            continue
        nvs_keys = field_to_nvs.get(field)
        if nvs_keys is None:
            problems.append(f"field `{field}` (config key `{toml_key}`) に対応するNVS keyがapply_nvsに無い")
            continue
        derived.append(ConfigKey(toml_key, alias, const_name, field, nvs_keys, optional))

    for const_name in const_to_field:
        if const_name not in consts_seen:
            problems.append(f"from_build_config の `{const_name}` がbuild.rsのKEYSに無い")
    fields_with_build = {const_to_field[c] for c in consts_seen if c in const_to_field}
    for field in field_to_nvs:
        if field not in fields_with_build:
            problems.append(f"apply_nvs の field `{field}` がfrom_build_configに無い")

    if problems:
        raise ValueError("設定キー対応の整合性を取れません:\n  " + "\n  ".join(problems))
    return tuple(derived)


def parse_readme_table(text: str) -> set[tuple[str, str]]:
    """READMEの「対応するNVS key」表から (nvs_key, toml_key) の組を抽出する。"""
    match = re.search(r"対応するNVS key:\n(.*?)(?:\n\n)", text, re.DOTALL)
    if match is None:
        raise ValueError("READMEに「対応するNVS key」表が見つかりません")
    pairs = set()
    for nvs_key, toml_key in re.findall(r"^\|\s*`([^`]+)`\s*\|\s*`([^`]+)`\s*\|", match.group(1), re.MULTILINE):
        pairs.add((nvs_key, toml_key))
    return pairs


def check(firmware_dir: Path | None = None) -> list[str]:
    """導出 mappings と README表の差分を指摘列で返す。秘密値には触れない。"""
    firmware_dir = firmware_dir or FIRMWARE_DIR
    problems = []
    try:
        mappings = derive_mappings(firmware_dir)
    except ValueError as e:
        return [str(e)]

    expected = {(key.nvs_keys[0], key.toml_key) for key in mappings}
    try:
        listed = parse_readme_table((firmware_dir / "README.md").read_text(encoding="utf-8"))
    except ValueError as e:
        return [str(e)]

    for nvs_key, toml_key in sorted(expected - listed):
        problems.append(f"README表に無い: NVS `{nvs_key}` <- config `{toml_key}`")
    for nvs_key, toml_key in sorted(listed - expected):
        problems.append(f"README表にあって源码に無い: NVS `{nvs_key}` <- config `{toml_key}`")
    return problems


def main(argv: list[str]) -> int:
    if argv and argv[0] == "check":
        problems = check()
        if problems:
            print("config key整合性チェック失敗:", file=sys.stderr)
            for problem in problems:
                print(f"  - {problem}", file=sys.stderr)
            return 1
        print(f"config key整合性OK ({len(derive_mappings())} keys)")
        return 0
    if argv and argv[0] == "list":
        for key in derive_mappings():
            legacy = ",".join(key.nvs_keys[1:]) or "-"
            print(f"{key.toml_key}\t{key.const_name}\t{key.nvs_keys[0]}\t{legacy}\t{'optional' if key.optional else 'required'}")
        return 0
    print("usage: config_keys.py [check|list]", file=sys.stderr)
    return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
