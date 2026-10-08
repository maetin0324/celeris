#!/usr/bin/env bash
# CoS chat の prompt cache baseline / after 用 live ベンチ（ADR 2026-10-08-cos-chat-prompt-cache D3.4-2）。
# scripts/dev/cos-chat-live.sh と同じ隔離方式（試験用 data dir・一時 DB・別 port・本番を読まない）。
#
#   bash scripts/dev/cos-chat-bench.sh <repo> <bin dir> <data dir> dry|full [claude-code|codex] [reduced]
#
#   full      : 台本 scripts/dev/cos-chat-bench-script.json を流す（claude-code は LLM run 26 回）。
#               claude_oauth の既存ログインだけを使う。API 課金 source は使わない
#   reduced   : 縮小版（新規 2 thread + 同一 thread 3 turn。codex 等の確認用）
#   第 5 引数 : harness（既定 claude-code）。codex は llm_source=codex_oauth の既存ログインが要る
#
# 結果は <data dir>/evidence/{runs.json,runs.csv,summary.json,tables.md,steps.log}。
# 本番の設定・DB・KB・port は読まない・書かない。終わりに daemon の process group を止める。
set -eu
usage() {
  echo "usage: bash scripts/dev/cos-chat-bench.sh <repo> <bin dir> <data dir> dry|full [claude-code|codex] [reduced]" >&2
  exit 2
}
[ "$#" -ge 4 ] && [ "$#" -le 6 ] || usage
case "$4" in dry|full) ;; *) usage ;; esac
MODE=$4
HARNESS=${5:-claude-code}
REDUCED=0; [ "${6:-}" = reduced ] && REDUCED=1
case "$HARNESS" in claude-code) SRC=claude_oauth ;; codex) SRC=codex_oauth ;; *) usage ;; esac
REPO=$(cd "$1" && pwd); BIN=$(cd "$2" && pwd); mkdir -p "$3"; OUT=$(cd "$3" && pwd)
for exe in celeris celerisctl; do
  [ -x "$BIN/$exe" ] || { echo "missing $BIN/$exe (cargo build -p celeris -p celerisctl)" >&2; exit 2; }
done
# 本番の置き場を data dir にしない（ADR-0095 付記 D-d・ADR-0126）
case "$OUT/" in
  "$HOME/.config/celeris/"*|"$HOME/.local/celeris/"*|/local/celeris/state/*)
    echo "refusing production location as data dir: $OUT" >&2; exit 2 ;;
esac
PORT=${COS_CHAT_BENCH_PORT:-17942}
API="http://127.0.0.1:$PORT/api/v1"
if curl -s -o /dev/null --max-time 2 "http://127.0.0.1:$PORT/"; then
  echo "port $PORT is already in use; set COS_CHAT_BENCH_PORT to a free port" >&2; exit 2
fi
EV="$OUT/evidence"; mkdir -p "$EV" "$OUT/kb/skills" "$OUT/memory" "$OUT/state" "$OUT/workspaces" "$OUT/logs" "$OUT/backups"
log() { printf '%s %s\n' "$(date -u +%H:%M:%S)" "$*" | tee -a "$EV/steps.log"; }
verdict() { printf '%s %s\n' "$1" "$2" | tee -a "$EV/verdict.txt"; }
: > "$EV/verdict.txt"

# --- 設定（試験用 data dir の中だけ） --------------------------------------------------------------
env -u CELERIS_CONFIG -u CELERIS_API_URL "$BIN/celerisctl" knowledge init --root "$OUT/kb" --config "$OUT/config.toml" --db "$OUT/celeris.sqlite3" > "$EV/kb-init.txt" 2>&1 || { echo "kb init failed"; cat "$EV/kb-init.txt"; exit 1; }
cp -r "$REPO/config/skills/cos-operator" "$REPO/config/skills/cos-inbox-triage" "$OUT/kb/skills/"
[ -s "$OUT/api.token" ] || { head -c 24 /dev/urandom | od -An -tx1 | tr -d ' \n' > "$OUT/api.token"; }
chmod 600 "$OUT/api.token"; TOK=$(cat "$OUT/api.token")
cat > "$OUT/config.toml" <<EOF
# cos-chat-bench.sh の一時設定（ADR-0126 試験用 data dir）。本番 ~/.config/celeris は使わない
db = { path = "celeris.sqlite3", worker_read_only = false }
workspace_root = "workspaces"
tick_ms = 1000

[api]
listen = "127.0.0.1:$PORT"
token_file = "api.token"

[[providers]]
id = "bench-$HARNESS"
kind = "adapter"
adapter = "$HARNESS"
llm_source = "$SRC"
tiers = ["frontier", "standard", "cheap"]
# 1 では CoS の turn が task worker と受信箱の triage の後ろで止まった（live-check2 attempt 2）
concurrency = 4

[cos]
enabled = true
harness = "$HARNESS"
llm_source = "$SRC"
tier = "frontier"
max_turns = 20
max_wall_secs = 600

[workspace]
build_cache_dir = "build-cache"

[memory]
dir = "memory"

[knowledge]
root = "kb"
EOF

# --- daemon（CELERIS_* は呼び手の env を継がず、すべて data dir に向ける） ---------------------------
for v in $(env | sed -n 's/^\(CELERIS_[A-Z_]*\)=.*/\1/p'); do unset "$v"; done
export CELERIS_CONFIG="$OUT/config.toml" CELERIS_DB="$OUT/celeris.sqlite3" CELERIS_STATE_DIR="$OUT/state" \
       CELERIS_LOGS_DIR="$OUT/logs" CELERIS_BACKUPS_DIR="$OUT/backups"
cd "$OUT"
PATH="$BIN:$PATH" setsid "$BIN/celeris" --config "$OUT/config.toml" > "$OUT/daemon.log" 2>&1 &
DPID=$!; echo "$DPID" > "$OUT/daemon.pid"
stop_daemon() {
  kill -TERM -- "-$DPID" 2>/dev/null || true
  for _ in $(seq 1 30); do kill -0 "$DPID" 2>/dev/null || break; sleep 1; done
  kill -KILL -- "-$DPID" 2>/dev/null || true
  sleep 1
  # 自分の shell と呼び出し元は "$OUT" を引数に持つので除き、試験用 config で起動した process だけを見る
  if pgrep -f -- "--config $OUT/config.toml" > "$EV/pgrep-after-stop.txt"; then log "LEFTOVER processes:"; cat "$EV/pgrep-after-stop.txt"; else log "pgrep (config $OUT/config.toml): none left"; fi
}
trap stop_daemon EXIT

api() { curl -sS --max-time 30 -H "Authorization: Bearer $TOK" "$@"; }
js() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$@"; }
UP=
for _ in $(seq 1 60); do
  if api -o /dev/null -w '%{http_code}' "$API/chat/threads" 2>/dev/null | grep -q 200; then UP=1; break; fi
  sleep 1
done
[ -n "$UP" ] || { log "daemon did not answer on $API (see $OUT/daemon.log)"; exit 1; }
log "daemon pid $DPID up on $API (commit $(git -C "$REPO" rev-parse --short=12 HEAD), mode $MODE)"
NOW=$(date -u +%Y-%m-%dT%H:%M:%S.000Z)
sqlite3 "$OUT/celeris.sqlite3" "insert into projects (id,title,request,status,created_at,updated_at,slug) values ('01M4AW00000000000000000PRJ','agent-platform (live fixture)','cos-chat-live fixture project for KB scope','active','$NOW','$NOW','agent-platform')"
log "fixture project: $(sqlite3 "$OUT/celeris.sqlite3" "select id||' '||slug||' '||status from projects")"

CLAUDE_VERSION=$(claude --version 2>/dev/null | head -1 || echo unknown)
[ "$HARNESS" = codex ] && CLAUDE_VERSION="codex $(codex --version 2>/dev/null | head -1 || echo unknown)"
python3 "$REPO/scripts/dev/cos-chat-bench.py" "$API" "$TOK" "$OUT" "$REPO" "$MODE" "$REDUCED" "$CLAUDE_VERSION" || { log "driver failed"; cp "$OUT/daemon.log" "$EV/daemon.log"; exit 1; }
cp "$OUT/daemon.log" "$EV/daemon.log"
log "done; evidence in $EV (summary.json, tables.md)"
