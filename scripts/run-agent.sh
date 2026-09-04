#!/usr/bin/env bash
# 別のAIエージェント(OpenCode)へ作業を渡すときの起動ラッパー。
#
# 目的は「進まなくなったら早く止める」こと。素の `opencode run` には
# タイムアウトが無く、上流のレート制限で再試行が固まると**無限に待つ**。
#
# 実際に踏んだ事故(2026-09-05): 3ジョブを並列で回してプロバイダのレート制限
# (`rate_limit_exceeded`)を誘発し、再試行が応答しないまま**5時間**経過した。
# プロセスは生きていてCPUも少し使うため、`ps` では正常に見えてしまう。
# 成果は0件だった。
#
# 判定は「経過時間」ではなく「進んでいるか」で行う。固定タイムアウトだと
# その時間まで待たされるが、停止検知なら数分で戻る。
#
#   進捗の定義: opencodeのログファイルが伸びること。
#   一定時間伸びなければ停止とみなして打ち切る。
#
# レート制限のエラー自体は打ち切り条件にしない。多くは自動で回復するため
# (実測では185回発生し、大半はそのまま続行できた)。打ち切ったときだけ、
# 直前のエラー行を原因として表示する。
set -euo pipefail

model=""
dir=""
prompt_file=""
continue_session=""
stall_minutes="${AGENT_STALL_MINUTES:-6}"
max_minutes="${AGENT_MAX_MINUTES:-90}"
log_file="${AGENT_LOG_FILE:-$HOME/.local/share/opencode/log/opencode.log}"

usage() {
  cat >&2 <<'USAGE'
usage: run-agent.sh --model <model> --dir <worktree> --prompt-file <file>
                    [--continue | --session <id>] [--stall-minutes N] [--max-minutes M]

  --continue       直前のセッションを継続する(対話的に小さく刻むときに使う)
  --session <id>   指定したセッションを継続する

  --stall-minutes  ログが伸びない状態がこの分数続いたら打ち切る (既定 6)
  --max-minutes    進んでいても全体でこの分数を超えたら打ち切る (既定 90)

exit code:
  0   正常終了
  2   停止を検知して打ち切った
  3   全体の上限に達して打ち切った
  それ以外は opencode の終了コード
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --model) model="$2"; shift 2 ;;
    --dir) dir="$2"; shift 2 ;;
    --prompt-file) prompt_file="$2"; shift 2 ;;
    --continue) continue_session="--continue"; shift ;;
    --session) continue_session="--session $2"; shift 2 ;;
    --stall-minutes) stall_minutes="$2"; shift 2 ;;
    --max-minutes) max_minutes="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "ERROR: 不明な引数: $1" >&2; usage; exit 64 ;;
  esac
done

if [[ -z "$model" || -z "$dir" || -z "$prompt_file" ]]; then
  echo "ERROR: --model / --dir / --prompt-file は必須です。" >&2
  usage
  exit 64
fi
if [[ ! -d "$dir" ]]; then
  echo "ERROR: worktreeがありません: $dir" >&2
  exit 64
fi
if [[ ! -f "$prompt_file" ]]; then
  echo "ERROR: promptファイルがありません: $prompt_file" >&2
  exit 64
fi

# モデルは必ず明示する。省略すると既定モデルで動き、結果が期待と違ったときに
# どのモデルの出力なのか後から特定できない。
echo "agent: model=$model dir=$dir stall=${stall_minutes}min max=${max_minutes}min" >&2

log_size() {
  # ログが未作成の場合もあるため 0 を返す。
  wc -c < "$log_file" 2>/dev/null || echo 0
}

out_file="$(mktemp)"
# trap から呼ぶため shellcheck からは到達不能・未使用に見える。
# 指摘されるコードはshellcheckのバージョンで異なる(0.11はSC2329、
# それ以前はSC2317)ため両方を抑制する。
# shellcheck disable=SC2329,SC2317
cleanup() {
  rm -f "$out_file"
}
trap cleanup EXIT

# 大きな作業を1回のプロンプトへ詰め込むと、エージェント内部のループが長くなり
# (実測で step=60 まで到達)、リクエストが肥大して上流のレート制限を踏みやすい。
# `--continue` でターンを分けると各リクエストが小さくなり、途中経過も見える。
# 分割の指針は docs/agent-roles.md を参照。
# shellcheck disable=SC2086 # continue_session は「空」か「2語」のどちらかで、分割させたい
opencode run --dir "$dir" --auto $continue_session -m "$model" "$(cat "$prompt_file")" >"$out_file" 2>&1 &
agent_pid=$!

# 打ち切り時は子プロセスを確実に止める。opencodeはこのシェルの子なので
# PID指定で足りる(`pkill -f "opencode run"` は起動元のshell自身にも一致して
# 自分を殺すため使わない。実際に踏んだ)。
kill_tree() {
  local signal="$1" pid="$2" child
  for child in $(pgrep -P "$pid" 2>/dev/null); do
    kill_tree "$signal" "$child"
  done
  kill "-$signal" "$pid" 2>/dev/null || true
}

kill_agent() {
  # 子を再帰的に辿って落とす。親のPIDだけを kill すると子が残る(実測)。
  #
  # プロセスグループへ負値で送る方法は使わない。opencodeがこのシェルと同じ
  # グループに居る場合、起動元(=このスクリプトを呼んだセッション)ごと巻き込む。
  # 同じ理由で `pkill -f` も使わない(自分のコマンドラインに一致して自滅する。
  # 実際に2回踏んだ)。
  kill_tree TERM "$agent_pid"
  sleep 3
  kill_tree KILL "$agent_pid"
  wait "$agent_pid" 2>/dev/null || true
}

report_last_error() {
  if [[ -f "$log_file" ]]; then
    local line
    line="$(grep -F 'level=ERROR' "$log_file" 2>/dev/null | tail -1 || true)"
    if [[ -n "$line" ]]; then
      # エラー行はプロンプト断片を含み得るため、error.error 以降だけを出す。
      echo "  直前のエラー: ${line##*error.error=}" >&2
    fi
  fi
}

poll_seconds=20
stall_limit=$(( stall_minutes * 60 ))
max_limit=$(( max_minutes * 60 ))
elapsed=0
stalled=0
last_size="$(log_size)"

while kill -0 "$agent_pid" 2>/dev/null; do
  sleep "$poll_seconds"
  elapsed=$(( elapsed + poll_seconds ))

  current_size="$(log_size)"
  if [[ "$current_size" -gt "$last_size" ]]; then
    last_size="$current_size"
    stalled=0
  else
    stalled=$(( stalled + poll_seconds ))
  fi

  if [[ "$stalled" -ge "$stall_limit" ]]; then
    echo "ERROR: ${stall_minutes}分間ログが伸びていません。停止とみなして打ち切ります。" >&2
    report_last_error
    kill_agent
    cat "$out_file"
    exit 2
  fi

  if [[ "$elapsed" -ge "$max_limit" ]]; then
    echo "ERROR: 全体の上限 ${max_minutes}分に達したため打ち切ります。" >&2
    report_last_error
    kill_agent
    cat "$out_file"
    exit 3
  fi
done

wait "$agent_pid"
status=$?
cat "$out_file"
exit "$status"
