#!/usr/bin/env bash
# 旧方式の「secretをRustソースへ書く」設定ファイルが残っていたら即停止する。
#
# 旧configは `pub const` 文字列を含むRust moduleだったため、コンパイラ警告が
# 該当ソース行を表示するとsecretがログへ出る。現在はGit管理外config.tomlを
# build.rsで読む方式なので、旧ファイルが残る端末では中身を表示せずbuildを止める。
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
legacy_configs=(
  "firmware/src/config.rs"
  "firmware/src/_config.rs"
)

for legacy_config in "${legacy_configs[@]}"; do
  if [[ -e "$repo_root/$legacy_config" ]]; then
    echo "ERROR: $legacy_config still exists." >&2
    echo "このファイルはsecretをRustソースへ直接置いている可能性があり、コンパイラ警告でビルドログへ漏れる恐れがあります。" >&2
    echo "値を firmware/config.toml へ移し、旧ファイルを削除してください。" >&2
    echo "secretを含む可能性があるため、内容は表示しません。" >&2
    exit 1
  fi
done

exit 0
