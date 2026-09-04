#!/usr/bin/env bash
# 「どのエージェントがどの役割か」が docs/agent-roles.md の外へ書かれていないか検査する。
#
# 背景: 役割の割り当ては頻繁に変わる。以前は CLAUDE.md・AGENTS.md・docs/architecture.md
# ・skillの4か所へ同じ割り当てが複製されており、担当が変わったときに1か所しか
# 更新されず、ドキュメントが実態とずれた状態が残った。
#
# 検査方法: エージェントの製品名と役割語が同じ行に現れたら、それは割り当ての記述と
# みなして落とす。製品名の単なる言及(起動コマンドの例示、事故の記録など)は
# 役割語と同居しない限り通す。
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# 役割の正本。このファイルの中だけは割り当てを書いてよい。
source_of_truth="docs/agent-roles.md"

if [[ ! -f "$source_of_truth" ]]; then
  echo "ERROR: $source_of_truth がありません。役割の正本が必要です。" >&2
  exit 1
fi

# エージェントの製品名。新しいエージェントを使い始めたらここへ追加する。
agent_names='Claude Code|ClaudeCode|Codex|OpenCode'

# 役割語。これと製品名が同じ行にあると「割り当ての記述」とみなす。
role_words='実装エージェント|設計エージェント|統合エージェント|レビューエージェント|が実装|が設計|が担当|割り当て'

violations=0
while IFS= read -r path; do
  [[ "$path" == "$source_of_truth" ]] && continue
  # このスクリプト自身はパターン定義のため除外する。
  [[ "$path" == "scripts/check-agent-roles.sh" ]] && continue

  if matches="$(grep -nE "$agent_names" -- "$path" 2>/dev/null | grep -E "$role_words")"; then
    if [[ "$violations" -eq 0 ]]; then
      echo "ERROR: 役割の割り当ては $source_of_truth にだけ書いてください。" >&2
      echo "       複製すると担当が変わったときに片方だけ古くなります。" >&2
      echo >&2
    fi
    violations=1
    while IFS= read -r line; do
      echo "  $path:$line" >&2
    done <<< "$matches"
  fi
done < <(git ls-files '*.md')

if [[ "$violations" -ne 0 ]]; then
  exit 1
fi

# 正本に「現在の割り当て」節があることを確認する。節ごと消えると、
# 上の検査は何も検出しないまま通ってしまう。
if ! grep -q '^## 現在の割り当て' "$source_of_truth"; then
  echo "ERROR: $source_of_truth に「## 現在の割り当て」節がありません。" >&2
  exit 1
fi

echo "agent roles OK (正本: $source_of_truth)"
