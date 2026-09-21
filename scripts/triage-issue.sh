#!/usr/bin/env bash
# Issueの規模・リスク・担当エージェントの候補を、Jev(TypeSafe AI)で判定する。
#
# **結果は提案であり決定ではない。** 最終判断は人間とClaudeが行う。
# firmware / m5stack-pc-bridge には組み込まない(Issue #196に理由)。
# 外部依存のため `make check` には入れない。
set -euo pipefail

readonly API_URL="https://api.typesafe.ai/v1/systemone"
readonly MODEL="jev-latest"
readonly TIMEOUT_SECS=30
# 応答が想定外でも異常終了させず、終了コードで呼び出し側が分岐できるようにする。
readonly EXIT_UNAVAILABLE=2

usage() {
  cat >&2 <<'USAGE'
使い方: scripts/triage-issue.sh <issue番号>

Issueのタイトルと本文をJevへ送り、規模・リスク・担当の候補を出す。
判定できないときは終了コード2で終わる(既存の運用はそのまま続けられる)。

必要な環境変数:
  TYPESAFE_API_KEY  TypeSafe AIのAPIキー
USAGE
}

fail_unavailable() {
  echo "判定できません: $1" >&2
  echo "手動で判断してください(このスクリプトは提案を出すだけで、他の作業には影響しません)。" >&2
  exit "$EXIT_UNAVAILABLE"
}

issue_number="${1:-}"
if [[ -z "$issue_number" ]]; then
  usage
  exit 1
fi
if [[ ! "$issue_number" =~ ^[0-9]+$ ]]; then
  echo "ERROR: issue番号は数字で指定してください: $issue_number" >&2
  exit 1
fi

for cmd in gh curl python3; do
  command -v "$cmd" >/dev/null 2>&1 || fail_unavailable "$cmd が見つかりません"
done
[[ -n "${TYPESAFE_API_KEY:-}" ]] || fail_unavailable "TYPESAFE_API_KEY が未設定です"

issue_json="$(gh issue view "$issue_number" --json title,body 2>/dev/null)" \
  || fail_unavailable "Issue #$issue_number を取得できません"

# 送るのはIssueのタイトルと本文だけ。secretや家庭内の情報は含めない。
# 質問の型と応答の形は公式仕様(choice / noul / score)に合わせる。
request_body="$(ISSUE_JSON="$issue_json" JEV_MODEL="$MODEL" python3 <<'PY'
import json, os

issue = json.loads(os.environ["ISSUE_JSON"])
state = f"タイトル: {issue.get('title', '')}\n\n本文:\n{issue.get('body', '') or '(本文なし)'}"

print(json.dumps({
    "model": os.environ["JEV_MODEL"],
    "state": state,
    "questions": {
        "size": {
            "type": "choice",
            "instructions": "このIssueの実装規模を見積もってください。",
            "criteria": {
                "small": "1ファイル程度の変更。既存の仕組みに沿って追加するだけ。",
                "medium": "複数ファイルにまたがるが、設計判断は不要。",
                "large": "設計判断が必要、または多くのファイルに波及する。",
            },
        },
        "risk": {
            "type": "choice",
            "instructions": "壊れたときの影響の大きさを評価してください。",
            "criteria": {
                "low": "表示・ドキュメント・開発ツールなど。壊れても復旧が容易。",
                "medium": "通常の機能。壊れると使えなくなるが復旧できる。",
                "high": "認証・署名・PCの電源操作・共有ロック・OTAに関わる。壊すと実害が出る。",
            },
        },
        "agent": {
            "type": "choice",
            "instructions": "どの実装エージェントに任せるのが適切か選んでください。",
            "criteria": {
                "opencode": "仕様が明確で、既存の形に沿って実装できる作業。",
                "codex": "同上。OpenCodeが使えないときの代替。",
                "claude": "設計判断・調査・レビューが主で、実装前に方針を決める必要がある作業。",
            },
        },
        "needs_hardware": {
            "type": "noul",
            "instructions": "完了の確認に、実機(M5Stack本体やWindows PC)での操作が必要か。",
            "criteria": {
                "true": "実機での書き込み・目視・電源操作が必要。",
                "false": "ビルドと自動テストだけで確認できる。",
            },
        },
    },
}))
PY
)" || fail_unavailable "リクエストを組み立てられません"

http_body="$(mktemp)"
trap 'rm -f "$http_body"' EXIT
http_status="$(
  curl -sS -m "$TIMEOUT_SECS" -o "$http_body" -w '%{http_code}' \
    -X POST "$API_URL" \
    -H "Authorization: Bearer $TYPESAFE_API_KEY" \
    -H "Content-Type: application/json" \
    -d "$request_body" 2>/dev/null
)" || fail_unavailable "APIへ接続できません"

# 401/429/5xx はそのまま人間へ見せる。本文はエラー詳細を含み得るので出さない。
[[ "$http_status" == "200" ]] || fail_unavailable "APIが $http_status を返しました"

RESPONSE_FILE="$http_body" ISSUE_NUMBER="$issue_number" python3 <<'PY' || fail_unavailable "応答を解釈できません"
import json, os, sys

with open(os.environ["RESPONSE_FILE"], encoding="utf-8") as f:
    data = json.load(f)
answers = data.get("answers")
if not isinstance(answers, dict):
    sys.exit(1)

def choice(name):
    a = answers.get(name) or {}
    value, conf = a.get("choice"), a.get("confidence")
    if value is None:
        return "判定なし"
    return f"{value}" + (f" (確信度 {conf:.2f})" if isinstance(conf, (int, float)) else "")

def noul(name):
    a = answers.get(name) or {}
    p = a.get("noul")
    if not isinstance(p, (int, float)):
        return "判定なし"
    return f"{'はい' if p >= 0.5 else 'いいえ'} (確率 {p:.2f})"

print(f"Issue #{os.environ['ISSUE_NUMBER']} の判定 (model: {data.get('model', '不明')})")
print(f"  規模:     {choice('size')}")
print(f"  リスク:   {choice('risk')}")
print(f"  担当候補: {choice('agent')}")
print(f"  実機確認: {noul('needs_hardware')}")
usage = data.get("usage") or {}
if usage:
    print(f"  トークン: 入力 {usage.get('input_tokens', '?')} / 出力 {usage.get('output_tokens', '?')}")
print()
# 確信度が低い判定は、モデル自身が迷っている。人間が見る合図として扱う。
low = [k for k in ("size", "risk", "agent")
       if isinstance((answers.get(k) or {}).get("confidence"), (int, float))
       and answers[k]["confidence"] < 0.5]
if low:
    print(f"確信度が低い項目があります({', '.join(low)})。人間が判断してください。")
print("これは提案です。最終判断は人間とClaudeが行ってください。")
PY
