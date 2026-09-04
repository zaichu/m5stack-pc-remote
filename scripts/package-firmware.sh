#!/usr/bin/env bash
# OTA配信用のアプリイメージ `firmware.bin` と `firmware.version` を生成する。
#
# `make firmware-build` が作るのはELFと bootloader.bin / partition-table.bin だけで、
# m5stack-pc-bridge が `GET /firmware` で配信するアプリイメージは作られない。
# その変換をここで行う(`espflash save-image`、`--merge` なし = アプリイメージ単体)。
#
# 生成物は `firmware/dist/` へ置く。bridgeへの配置手順は m5stack-pc-bridge/README.md 参照。
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
firmware_dir="$repo_root/firmware"
target="xtensa-esp32-espidf"
elf="$firmware_dir/target/$target/release/m5remote-rust"
out_dir="$firmware_dir/dist"

if ! command -v espflash >/dev/null 2>&1; then
  echo "ERROR: espflash が見つかりません。'cargo install espflash' を実行してください。" >&2
  exit 1
fi

if [[ ! -f "$elf" ]]; then
  echo "ERROR: $elf がありません。先に 'make firmware-build' を実行してください。" >&2
  exit 1
fi

# flash sizeは sdkconfig.defaults を正本にする。ここを固定値で書くと、
# sdkconfigだけ変えたときにイメージヘッダのflash size宣言だけが古くなる。
# 実測: --flash-size を省くとespflashは4MB想定でヘッダを書き、実機の16MBと
# 食い違うイメージになる(先頭4バイトが e9050220 / 正しくは e9050240)。
flash_size="$(sed -nE 's/^CONFIG_ESPTOOLPY_FLASHSIZE_([0-9]+MB)=y$/\1/p' \
  "$firmware_dir/sdkconfig.defaults" | head -n1 | tr '[:upper:]' '[:lower:]')"
if [[ -z "$flash_size" ]]; then
  echo "ERROR: sdkconfig.defaults から CONFIG_ESPTOOLPY_FLASHSIZE_*MB を読めませんでした。" >&2
  exit 1
fi

# versionは firmware/Cargo.toml を正本にする。bridgeはこれを manifest の
# `version` として配信し、署名対象に含める。
version="$(sed -nE '0,/^version *= *"([^"]+)"/s//\1/p' "$firmware_dir/Cargo.toml")"
if [[ -z "$version" ]]; then
  echo "ERROR: firmware/Cargo.toml から version を読めませんでした。" >&2
  exit 1
fi

# 未コミットの変更があると、配信した version がどのコミットにも対応しなくなる。
# 実際に踏んだ: Cargo.toml の version を worktree 内で上げただけで 0.2.0 を配信し、
# main は 0.1.0 のまま残った。後から「実機の 0.2.0 は何のコードか」を追えなくなる。
# OTAは戻すのに手間がかかるので、配る前に止める。
if git -C "$repo_root" rev-parse --git-dir >/dev/null 2>&1; then
  if ! git -C "$repo_root" diff --quiet HEAD -- firmware shared 2>/dev/null; then
    echo "ERROR: firmware/ または shared/ に未コミットの変更があります。" >&2
    echo "  配信するイメージがどのコミットのものか追えなくなるため、先にcommitしてください。" >&2
    echo "  意図的に試す場合は OTA_ALLOW_DIRTY=1 を付けてください。" >&2
    if [[ -z "${OTA_ALLOW_DIRTY:-}" ]]; then
      exit 1
    fi
    echo "WARNING: OTA_ALLOW_DIRTY によりdirtyなツリーから生成します。" >&2
  fi
fi

mkdir -p "$out_dir"
bin="$out_dir/firmware.bin"

# --partition-table を渡すと、アプリがota_0(2M)に収まるかを実サイズで検査できる。
# 省くとespflashは既定のパーティション想定(約3.9MB)で検査し、2Mを超える
# イメージを素通しする。
echo "アプリイメージを生成します (flash-size=$flash_size, version=$version)"
espflash save-image \
  --chip esp32 \
  --flash-size "$flash_size" \
  --partition-table "$firmware_dir/partitions.csv" \
  "$elf" "$bin"

# ESP32アプリイメージのmagicは 0xE9。ELFやマージ済みイメージを誤って
# 配信すると、実機は書き込んだ後の起動で初めて失敗する。ここで弾く。
magic="$(head -c1 "$bin" | od -An -tx1 | tr -d ' \n')"
if [[ "$magic" != "e9" ]]; then
  echo "ERROR: $bin のmagicが 0x$magic です。アプリイメージではありません。" >&2
  exit 1
fi

printf '%s\n' "$version" > "$out_dir/firmware.version"

size="$(wc -c < "$bin")"
if command -v sha256sum >/dev/null 2>&1; then
  sha="$(sha256sum "$bin" | cut -d' ' -f1)"
else
  sha="$(shasum -a 256 "$bin" | cut -d' ' -f1)"
fi

echo "生成しました:"
echo "  $bin"
echo "  $out_dir/firmware.version"
echo "  version = $version"
echo "  size    = $size bytes"
echo "  sha256  = $sha"
echo
echo "この sha256 は bridge が返す manifest の sha256 と一致します。"
echo "bridgeへの配置手順は m5stack-pc-bridge/README.md を参照してください。"
