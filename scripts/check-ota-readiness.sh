#!/usr/bin/env bash
# OTA実行前の readiness check。
#
# M5Stack 1台しかない環境で、Wi-Fi経由の firmware 更新(OTA)を始める前に
# 失敗原因を潰すための検査。`make ota-readiness-check` から呼ぶ。
# `make check` には入れない(実機作業を行う端末でだけ意味があるため)。
#
# 検査項目:
#   1. secretの一致(firmware側とbridge側)
#   2. bridge実行ファイル横の firmware.bin / firmware.version の存在
#   3. イメージの妥当性(magic 0xE9、ota_0サイズ)
#   4. 署名付き GET /firmware/manifest の応答と突き合わせ
#   5. 配信版が現在動いている版と違うこと(警告のみ)
#
# secretの扱い:
#   bridge_shared_secret / shared_secret / telegram_bot_token の値は
#   標準出力にも標準エラーにも出さない。取得・比較・署名は python3 の中で
#   完結させ、bash側へ渡すのは一致/不一致などの真偽値と非秘密のメタ情報だけ。
#   ハッシュ値も出さない(生sha256は総当たりの手がかりになるため)。
#   `set -x` は使わない(トレースにsecretが載るため)。
#
# 署名の正本:
#   リクエスト署名の canonical string は shared/pc-remote-signing/src/lib.rs の
#   `canonical_string`(METHOD/PATH/TIMESTAMP/NONCE/SHA256(BODY)を改行連結し、
#   shared_secretでHMAC-SHA256してhex化)。ヘッダ名は
#   firmware/src/bridge_client.rs と firmware/src/ota.rs が送る
#   X-Timestamp / X-Nonce / X-Signature を使う。
#   manifest署名の canonical string は同crateの `manifest_canonical_string`
#   ("FIRMWARE-MANIFEST-v1"で始めるドメイン分離形式)を使う。
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
firmware_config="$repo_root/firmware/config.toml"
bridge_config="$repo_root/m5stack-pc-bridge/config.toml"
partitions_csv="$repo_root/firmware/partitions.csv"
firmware_cargo="$repo_root/firmware/Cargo.toml"
dist_bin="$repo_root/firmware/dist/firmware.bin"

default_bridge_url="http://127.0.0.1:18080"

fails=0
warns=0

pass() { printf '[OK] %s\n' "$1"; }
fail() { printf '[FAIL] %s\n' "$1"; fails=$((fails + 1)); }
warn() { printf '[WARN] %s\n' "$1"; warns=$((warns + 1)); }
skip() { printf '[SKIP] %s\n' "$1"; }

# 設定ファイルが無い環境(CI等)では失敗にせず警告してskipする。
# この検査は実機作業を行う端末でだけ意味がある。
if [[ ! -f "$firmware_config" ]] || [[ ! -f "$bridge_config" ]]; then
  skip "config.toml が無いため検査をskipします(実機作業端末でのみ実行してください)。"
  printf '判定: SKIP (configが無い環境では何も検査しません)\n'
  exit 0
fi

bridge_url="${OTA_BRIDGE_URL:-$default_bridge_url}"

# python3 の KEY=value 出力から値だけ抜く。パイプは使わない
# (SC2312: コマンド置換内のパイプは終了コードを隠すため)。
parse_kv() {
  local line="$1" key="$2"
  local name value
  while IFS='=' read -r name value; do
    if [[ "$name" == "$key" ]]; then
      printf '%s' "$value"
      return 0
    fi
  done <<< "$line"
  return 0
}

# ---- 1. secretの一致 ----
# 値の比較は python3 の中で行い、bash側へは真偽値だけ返す。
# firmware側は build.rs の lookup と同じく bridge_shared_secret を先に見て、
# 無ければ旧key agent_shared_secret を見る。キー名の表示はよいが値は出さない。
secret_out="$(python3 - "$firmware_config" "$bridge_config" <<'PYEOF' || true
import hmac
import sys
import tomllib

# build.rs の placeholder。値自体は表示せず、一致したことだけを返す。
firmware_placeholder = "replace-with-the-same-secret-as-m5stack-pc-bridge"
bridge_placeholder = "replace-with-a-long-random-shared-secret"


def load_secret(path, keys):
    try:
        with open(path, "rb") as f:
            table = tomllib.load(f)
    except (OSError, tomllib.TOMLDecodeError):
        return None, None
    for key in keys:
        value = table.get(key)
        if isinstance(value, str) and value:
            return value, key
    return None, None


def main():
    firmware_secret, firmware_key = load_secret(sys.argv[1], ("bridge_shared_secret", "agent_shared_secret"))
    bridge_secret, bridge_key = load_secret(sys.argv[2], ("shared_secret",))
    if firmware_secret is None or bridge_secret is None:
        print("SECRET_CHECK=unreadable")
        return
    # placeholderのままでは起動すらしないので先に落とす(値は出さない)。
    if firmware_secret == firmware_placeholder or bridge_secret == bridge_placeholder:
        print("SECRET_CHECK=placeholder")
        print(f"FIRMWARE_KEY={firmware_key}")
        print(f"BRIDGE_KEY={bridge_key}")
        return
    match = hmac.compare_digest(firmware_secret.encode(), bridge_secret.encode())
    # 一致/不一致の真偽値だけを出す。ハッシュも出さない(総当たりの手がかりになるため)。
    print(f"SECRET_CHECK={'match' if match else 'mismatch'}")
    print(f"FIRMWARE_KEY={firmware_key}")
    print(f"BRIDGE_KEY={bridge_key}")


try:
    main()
except Exception:
    # 例外文には設定値が混ざり得るため出さない。固定文だけ返す。
    print("SECRET_CHECK=error")
PYEOF
)"
secret_check="$(parse_kv "$secret_out" SECRET_CHECK)"
firmware_key="$(parse_kv "$secret_out" FIRMWARE_KEY)"
case "$secret_check" in
  match)
    pass "secret一致 (firmware側=$firmware_key bridge側=shared_secret)。401の心配はありません。"
    ;;
  mismatch)
    fail "secret不一致 (firmware側=$firmware_key bridge側=shared_secret)。全リクエストが401になり、復旧にUSB再書き込みが要ります。両configの値を揃えてください(値は表示しません)。"
    ;;
  placeholder)
    fail "secretがplaceholderのままです (firmware側=$firmware_key)。両configへ長いランダム値を設定してください。"
    ;;
  *)
    fail "secretを読み取れませんでした。config.toml のキー名を確認してください(値は表示しません)。"
    ;;
esac

# ---- 2. 配信ファイルの存在 ----
# bridgeは実行ファイルと同じディレクトリの firmware.bin を配信する
# (m5stack-pc-bridge/src/lib.rs の exe_dir_file が正本)。
# 候補を順に探し、firmware.bin を持つ場所を優先する。
delivery_dir=""
searched_dirs=()
consider_dir() {
  local dir="$1"
  [[ -n "$dir" ]] || return 0
  [[ -d "$dir" ]] || return 0
  searched_dirs+=("$dir")
  if [[ -z "$delivery_dir" && -f "$dir/firmware.bin" ]]; then
    delivery_dir="$dir"
  fi
  return 0
}
if [[ -n "${OTA_BRIDGE_DIR:-}" ]]; then
  # 明示指定が最優先。bridgeの配置を自動検出できない環境用。
  if [[ -d "$OTA_BRIDGE_DIR" ]]; then
    searched_dirs+=("$OTA_BRIDGE_DIR")
    delivery_dir="$OTA_BRIDGE_DIR"
  else
    fail "OTA_BRIDGE_DIR がディレクトリではありません: $OTA_BRIDGE_DIR"
  fi
fi
if [[ -d /proc ]]; then
  for pid_dir in /proc/[0-9]*; do
    [[ -L "$pid_dir/exe" ]] || continue
    exe_path="$(readlink "$pid_dir/exe" 2>/dev/null || true)"
    [[ -n "$exe_path" ]] || continue
    exe_name="$(basename "$exe_path")"
    if [[ "$exe_name" == "m5stack-pc-bridge" || "$exe_name" == "m5stack-pc-bridge.exe" ]]; then
      consider_dir "$(dirname "$exe_path")"
    fi
  done
fi
if command -v m5stack-pc-bridge >/dev/null 2>&1; then
  consider_dir "$(dirname "$(command -v m5stack-pc-bridge)")"
fi
# 開発端末で cargo run / cargo build したbridgeの実行場所。
consider_dir "$repo_root/m5stack-pc-bridge/target/debug"
consider_dir "$repo_root/m5stack-pc-bridge/target/release"
if [[ -z "$delivery_dir" && "${#searched_dirs[@]}" -gt 0 ]]; then
  delivery_dir="${searched_dirs[0]}"
fi

delivery_bin=""
if [[ -z "$delivery_dir" ]]; then
  fail "bridgeの実行ファイル場所を特定できません。OTA_BRIDGE_DIR に bridge実行ファイルのあるディレクトリを指定してください。"
else
  delivery_version="$delivery_dir/firmware.version"
  if [[ -f "$delivery_dir/firmware.bin" ]]; then
    delivery_bin="$delivery_dir/firmware.bin"
    pass "firmware.bin あり ($delivery_dir)。"
    if [[ -f "$delivery_version" ]]; then
      pass "firmware.version あり ($delivery_dir)。"
    else
      warn "firmware.version がありません。manifestのversionは \"unknown\" になります(配信自体は可能です)。"
    fi
  else
    fail "firmware.bin がありません。make firmware-package で作り、bridge実行ファイル横へ置いてください。探した場所: ${searched_dirs[*]:-(なし)}"
  fi
fi

# ---- 3. イメージの妥当性 ----
if [[ -z "$delivery_bin" ]]; then
  skip "配信firmwareが無いためイメージ検査をskipします。"
else
  image_out="$(python3 - "$delivery_bin" "$partitions_csv" "$dist_bin" <<'PYEOF' || true
import csv
import hashlib
import re
import sys


def parse_size(text):
    text = text.strip()
    if text.lower().startswith("0x"):
        return int(text, 16)
    match = re.fullmatch(r"(\d+)([KkMm]?)", text)
    if not match:
        raise ValueError(f"size parse error: {text!r}")
    value = int(match.group(1))
    unit = match.group(2).lower()
    if unit == "k":
        value *= 1024
    elif unit == "m":
        value *= 1024 * 1024
    return value


def ota0_size(csv_path):
    with open(csv_path, newline="", encoding="utf-8") as f:
        for row in csv.reader(f):
            if not row or not row[0].strip() or row[0].strip().startswith("#"):
                continue
            if row[0].strip() == "ota_0":
                return parse_size(row[4])
    raise ValueError("ota_0 row not found")


def main():
    bin_path, csv_path, dist_path = sys.argv[1], sys.argv[2], sys.argv[3]
    try:
        with open(bin_path, "rb") as f:
            data = f.read()
    except OSError:
        print("IMAGE_CHECK=unreadable")
        return
    if not data:
        print("IMAGE_CHECK=empty")
        return
    print("IMAGE_CHECK=readable")
    print(f"MAGIC={data[0]:02x}")
    print(f"SIZE={len(data)}")
    try:
        print(f"OTA0_SIZE={ota0_size(csv_path)}")
    except (OSError, ValueError):
        print("OTA0_SIZE=unknown")
    # 最新ビルドとの突き合わせ用。sha自体は出さず一致/不一致だけ出す。
    try:
        with open(dist_path, "rb") as f:
            dist_data = f.read()
        same = hashlib.sha256(data).digest() == hashlib.sha256(dist_data).digest()
        print(f"DIST_MATCH={'yes' if same else 'no'}")
    except OSError:
        print("DIST_MATCH=unknown")


try:
    main()
except Exception:
    print("IMAGE_CHECK=error")
PYEOF
)"
  image_check="$(parse_kv "$image_out" IMAGE_CHECK)"
  magic="$(parse_kv "$image_out" MAGIC)"
  image_size="$(parse_kv "$image_out" SIZE)"
  ota0_size="$(parse_kv "$image_out" OTA0_SIZE)"
  dist_match="$(parse_kv "$image_out" DIST_MATCH)"
  case "$image_check" in
    readable)
      if [[ "$magic" == "e9" ]]; then
        pass "先頭magic 0xe9 (ESP32アプリイメージ)。"
      else
        fail "先頭magic が 0x${magic:-??} です。ELFやマージ済みイメージを誤って置いていませんか。"
      fi
      if [[ "$ota0_size" == "unknown" ]]; then
        warn "partitions.csv から ota_0 サイズを読めませんでした。サイズ検査をskipします。"
      elif [[ "$image_size" -le "$ota0_size" ]]; then
        pass "サイズ ${image_size} bytes <= ota_0 ${ota0_size} bytes。"
      else
        fail "サイズ ${image_size} bytes が ota_0 ${ota0_size} bytes を超えています。"
      fi
      if [[ "$dist_match" == "no" ]]; then
        warn "配置済みfirmware.bin が firmware/dist/firmware.bin と異なります。古いビルドを置いたままかもしれません。"
      fi
      ;;
    empty)
      fail "firmware.bin が空です。"
      ;;
    *)
      fail "firmware.bin を読み取れませんでした。"
      ;;
  esac
fi

# ---- 4. bridgeの応答 + 5. 版の違い ----
# 署名付き GET /firmware/manifest を送り、200・sha256/size突き合わせ・manifest署名
# を検証する。版の比較は警告に留める。secretは python3 の中だけで使う。
manifest_out="$(python3 - "$bridge_config" "$bridge_url" "$delivery_bin" "$firmware_cargo" <<'PYEOF' || true
import hashlib
import hmac
import json
import re
import secrets
import sys
import time
import tomllib
import urllib.error
import urllib.request


def load_bridge_secret(path):
    try:
        with open(path, "rb") as f:
            table = tomllib.load(f)
    except (OSError, tomllib.TOMLDecodeError):
        return None
    value = table.get("shared_secret")
    return value if isinstance(value, str) and value else None


def request_signature(secret, method, path, timestamp, nonce):
    # 正本は shared/pc-remote-signing/src/lib.rs の canonical_string。
    # METHOD/PATH/TIMESTAMP/NONCE/SHA256(BODY)を改行連結しHMAC-SHA256してhex化。
    # GET /firmware/manifest のBODYは空(bridge側も b"" で検証する)。
    body_sha = hashlib.sha256(b"").hexdigest()
    canonical = f"{method.upper()}\n{path}\n{timestamp}\n{nonce}\n{body_sha}"
    return hmac.new(secret.encode(), canonical.encode(), hashlib.sha256).hexdigest()


def manifest_signature(secret, version, size, sha256, created_at):
    # 正本は shared/pc-remote-signing/src/lib.rs の manifest_canonical_string。
    canonical = f"FIRMWARE-MANIFEST-v1\n{version}\n{size}\n{sha256}\n{created_at}"
    return hmac.new(secret.encode(), canonical.encode(), hashlib.sha256).hexdigest()


def local_version(cargo_path):
    try:
        with open(cargo_path, encoding="utf-8") as f:
            text = f.read()
    except OSError:
        return None
    match = re.search(r'^version\s*=\s*"([^"]+)"', text, re.MULTILINE)
    return match.group(1) if match else None


def main():
    config_path, base_url, bin_path, cargo_path = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
    secret = load_bridge_secret(config_path)
    if secret is None:
        print("MANIFEST_CHECK=config-unreadable")
        return
    timestamp = int(time.time())
    # bridgeのNonceStoreは英数字と -_. のみ・64文字以内を受け付ける。
    nonce = f"ota-check-{timestamp}-{secrets.token_hex(8)}"
    path = "/firmware/manifest"
    signature = request_signature(secret, "GET", path, timestamp, nonce)
    # ヘッダ名は firmware/src/bridge_client.rs と firmware/src/ota.rs が送るものと同じ。
    request = urllib.request.Request(
        base_url.rstrip("/") + path,
        headers={"X-Timestamp": str(timestamp), "X-Nonce": nonce, "X-Signature": signature},
    )
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            status = response.status
            body = response.read()
    except urllib.error.HTTPError as e:
        print(f"MANIFEST_CHECK=http-{e.code}")
        return
    except Exception:
        print("MANIFEST_CHECK=unreachable")
        return
    if status != 200:
        print(f"MANIFEST_CHECK=http-{status}")
        return
    try:
        manifest = json.loads(body)
        version = manifest["version"]
        size = manifest["size"]
        sha256 = manifest["sha256"]
        created_at = manifest["created_at"]
        manifest_sig = manifest["signature"]
        if not isinstance(version, str) or not version:
            raise ValueError("version")
        if not isinstance(size, int) or size <= 0:
            raise ValueError("size")
        if not isinstance(sha256, str) or len(sha256) != 64:
            raise ValueError("sha256")
        int(sha256, 16)
        if not isinstance(created_at, str) or not created_at:
            raise ValueError("created_at")
        if not isinstance(manifest_sig, str) or not manifest_sig:
            raise ValueError("signature")
    except (ValueError, KeyError, TypeError, json.JSONDecodeError):
        # 未検証のmanifest本文はログへ出さない(field名と理由だけに留める)。
        print("MANIFEST_CHECK=bad-manifest")
        return
    # 署名検証。通るまで公開値は信用しない。
    expected = manifest_signature(secret, version, size, sha256.lower(), created_at)
    if not hmac.compare_digest(expected, manifest_sig.lower()):
        print("MANIFEST_CHECK=bad-signature")
        return
    print("MANIFEST_CHECK=ok")
    print(f"MANIFEST_VERSION={version}")
    current = local_version(cargo_path)
    if current is None or version == "unknown":
        # firmware.version 未配置時は版比較ができない。
        print("VERSION_CHECK=unknown")
    elif version == current:
        print("VERSION_CHECK=same")
    else:
        print("VERSION_CHECK=different")
    # sha256/sizeの突き合わせは配置ファイルを正とする。配置場所不明なら比較不可。
    # bridgeが配信中でも、手元の配置場所を特定できなければ何を焼くか
    # 検証できないため、ここでは突き合わせ不可として落とす(fail-closed)。
    if bin_path:
        try:
            with open(bin_path, "rb") as f:
                data = f.read()
            local_sha = hashlib.sha256(data).hexdigest()
            local_size = len(data)
        except OSError:
            print("FILE_MATCH=unreadable")
            return
        if local_size == size and hmac.compare_digest(local_sha, sha256.lower()):
            print("FILE_MATCH=yes")
        else:
            print("FILE_MATCH=no")
            print(f"LOCAL_SIZE={local_size}")
            print(f"MANIFEST_SIZE={size}")
            return
    else:
        print("FILE_MATCH=unknown")
        return


try:
    main()
except Exception:
    # 例外文には応答内容が混ざり得るため出さない。
    print("MANIFEST_CHECK=error")
PYEOF
)"
manifest_check="$(parse_kv "$manifest_out" MANIFEST_CHECK)"
file_match="$(parse_kv "$manifest_out" FILE_MATCH)"
manifest_version="$(parse_kv "$manifest_out" MANIFEST_VERSION)"
version_check="$(parse_kv "$manifest_out" VERSION_CHECK)"
case "$manifest_check" in
  ok)
    pass "GET /firmware/manifest が200を返し、manifest署名が有効です (version=${manifest_version:-?})。"
    ;;
  http-401)
    fail "manifest取得が401でした。bridgeが別secretで動いている可能性があります(値は表示しません)。bridgeの再起動状態とconfigを確認してください。"
    ;;
  http-404)
    fail "manifest取得が404でした。bridge側にfirmware.binが置かれていません。"
    ;;
  http-*)
    fail "manifest取得が ${manifest_check#http-} を返しました。"
    ;;
  unreachable)
    fail "bridge (${bridge_url}) に接続できません。bridgeが起動しているか確認してください。"
    ;;
  bad-manifest)
    fail "manifestの形式が不正です。bridgeの応答を確認してください(内容は表示しません)。"
    ;;
  bad-signature)
    fail "manifestの署名検証に失敗しました。bridgeのsecretとconfigを確認してください(値は表示しません)。"
    ;;
  *)
    fail "manifest取得に失敗しました。"
    ;;
esac
if [[ "$manifest_check" == "ok" ]]; then
  case "$file_match" in
    yes)
      pass "manifestのsha256/sizeが配置firmware.binと一致します。"
      ;;
    no)
      fail "manifestのsha256/sizeが配置firmware.binと一致しません。bridgeが別ファイルを配信しているか、配置後に更新されました。"
      ;;
    unknown)
      fail "bridgeはfirmwareを配信中ですが、手元の配置場所を特定できないため突き合わせできません。OTA_BRIDGE_DIR にbridge実行ファイルのあるディレクトリを指定してください。"
      ;;
    *)
      fail "配置firmware.binとの突き合わせができませんでした。"
      ;;
  esac
  # 5. 版の違い。同一版の更新は無意味なため警告する(失敗にはしない)。
  # 実機の版を直接問う手段は無いため、手元の firmware/Cargo.toml の版を
  # 「USBで書き込んだ実機の版」の代理として比べる。初回OTA前は同じ版に
  # なることが多く、その場合は警告が出るのが正常。
  case "$version_check" in
    different)
      pass "配信版(${manifest_version})が手元の版と異なります。更新テストとして意味があります。"
      ;;
    same)
      warn "配信版(${manifest_version})が手元の版と同じです。更新しても変化が無く、テストとして無意味です。版を上げてからOTAしてください。"
      ;;
    *)
      warn "配信版と手元の版を比べられませんでした(manifest version=${manifest_version:-?})。"
      ;;
  esac
fi

# ---- 判定 ----
if [[ "$fails" -gt 0 ]]; then
  printf '判定: NOT READY (%d件のFAIL、%d件のWARN)\n' "$fails" "$warns"
  exit 1
fi
if [[ "$warns" -gt 0 ]]; then
  printf '判定: READY(注意あり) (%d件のWARN)\n' "$warns"
else
  printf '判定: READY\n'
fi
exit 0
