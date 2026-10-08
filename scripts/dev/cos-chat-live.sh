#!/usr/bin/env bash
# CoS chat の実機確認（live-check2 の live2-ops.sh を repo に入れたもの。ADR 2026-10-07-cos-live-fixes D5）。
#
#   bash scripts/dev/cos-chat-live.sh <repo> <bin dir> <data dir> dry|full
#
#   repo     : 確かめる tree の worktree（config/skills を写す・commit を記録する）
#   bin dir  : `cargo build -p celeris -p celerisctl` の出力（$CARGO_TARGET_DIR/debug）
#   data dir : 試験用 data dir（config・DB・kb・workspaces・state・log・証跡がすべてこの下。ADR-0126）
#   dry      : daemon 起動・thread 作成・添付 upload まで。LLM を呼ばない（message を送らない）
#   full     : (a)〜(e) を流す。claude_oauth の既存ログインで CoS と task worker の Claude Code が起動する
#
# 本番の設定・DB・KB・port は読まない・書かない（CELERIS_CONFIG ほか CELERIS_* は data dir の下だけを指す）。
# 外部への通信は full の LLM 呼び出しだけ。試験・CI からは呼ばない。
# 終わりに一時 daemon を process group ごと止め、data dir を指す process が残らないことを pgrep で見る。
# 証跡は <data dir>/evidence/（steps.log・verdict.txt・db.txt・kb-inbox*.json・input-manifest.txt・staged.txt …）。
set -eu
usage() {
  echo "usage: bash scripts/dev/cos-chat-live.sh <repo> <bin dir> <data dir> dry|full" >&2
  exit 2
}
[ "$#" -eq 4 ] || usage
case "$4" in dry|full) ;; *) usage ;; esac
MODE=$4
REPO=$(cd "$1" && pwd); BIN=$(cd "$2" && pwd); mkdir -p "$3"; OUT=$(cd "$3" && pwd)
for exe in celeris celerisctl; do
  [ -x "$BIN/$exe" ] || { echo "missing $BIN/$exe (cargo build -p celeris -p celerisctl)" >&2; exit 2; }
done
# 本番の置き場を data dir にしない（ADR-0095 付記 D-d・ADR-0126）
case "$OUT/" in
  "$HOME/.config/celeris/"*|"$HOME/.local/celeris/"*|/local/celeris/state/*)
    echo "refusing production location as data dir: $OUT" >&2; exit 2 ;;
esac
PORT=${COS_CHAT_LIVE_PORT:-17932}
API="http://127.0.0.1:$PORT/api/v1"
if curl -s -o /dev/null --max-time 2 "http://127.0.0.1:$PORT/"; then
  echo "port $PORT is already in use; set COS_CHAT_LIVE_PORT to a free port" >&2; exit 2
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
# cos-chat-live.sh の一時設定（ADR-0126 試験用 data dir）。本番 ~/.config/celeris は使わない
db = { path = "celeris.sqlite3", worker_read_only = false }
workspace_root = "workspaces"
tick_ms = 1000

[api]
listen = "127.0.0.1:$PORT"
token_file = "api.token"

[[providers]]
id = "claude-live"
kind = "adapter"
adapter = "claude-code"
llm_source = "claude_oauth"
tiers = ["frontier", "standard", "cheap"]
# 1 では CoS の turn が task worker と受信箱の triage の後ろで止まった（live-check2 attempt 2）
concurrency = 4

[cos]
enabled = true
harness = "claude-code"
llm_source = "claude_oauth"
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

# --- 添付の素材（外部から取らない。python で生成） ----------------------------------------------
python3 - "$OUT" <<'PY'
import struct, sys, zlib
out = sys.argv[1]
def png(path, w, h, px):
    raw = b"".join(b"\x00" + b"".join(bytes(px(x, y)) for x in range(w)) for y in range(h))
    def chunk(t, d): return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xffffffff)
    open(path, "wb").write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
                           + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))
# (b) 32x32 の青い正方形
png(f"{out}/blue.png", 32, 32, lambda x, y: (20, 60, 220))
# (d) 画面の模型: 灰色の画面、上に濃紺の header、右端からはみ出した赤い button
def ui(x, y):
    if y < 24: return (20, 30, 70)
    if 60 <= y < 84 and x >= 250: return (220, 30, 30)
    return (235, 235, 235)
png(f"{out}/screen.png", 320, 160, ui)
# (e) 1 頁の PDF（ASCII だけの本文）
lines = ["Celeris cos-chat-live test document", "Fact: the test codeword is SAKURA-77.",
         "Fact: Celeris CoS chat runs keep one Claude session per thread."]
text = "BT /F1 14 Tf 50 750 Td 18 TL " + " ".join(f"({l}) Tj T*" for l in lines) + " ET"
objs = ["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
        f"<< /Length {len(text)} >>\nstream\n{text}\nendstream", "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"]
body, offs = b"%PDF-1.4\n", []
for i, o in enumerate(objs, 1):
    offs.append(len(body)); body += f"{i} 0 obj\n{o}\nendobj\n".encode()
x = len(body)
body += f"xref\n0 {len(objs)+1}\n0000000000 65535 f \n".encode() + b"".join(f"{o:010d} 00000 n \n".encode() for o in offs)
body += f"trailer\n<< /Size {len(objs)+1} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n".encode()
open(f"{out}/facts.pdf", "wb").write(body)
PY

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

THREAD=$(api -X POST -H 'content-type: application/json' "$API/chat/threads" \
  -d '{"title":"cos-chat-live","project_id":null,"client_thread_id":"cos-chat-live-'"$$"'"}' | tee "$EV/thread.json" | js 'd["thread"]["id"]')
log "thread $THREAD"
upload() { api -X POST -F "client_upload_id=$2" -F "file=@$OUT/$1" "$API/chat/threads/$THREAD/attachments" | tee "$EV/att-$1.json" | js 'd["attachment"]["id"]+" "+d["attachment"]["state"]+" "+d["attachment"]["sha256"]'; }
A_BLUE=$(upload blue.png u-blue); A_SCREEN=$(upload screen.png u-screen); A_PDF=$(upload facts.pdf u-pdf)
log "attachments: blue=$A_BLUE screen=$A_SCREEN pdf=$A_PDF"
A_BLUE=${A_BLUE%% *}; A_SCREEN=${A_SCREEN%% *}; A_PDF=${A_PDF%% *}

if [ "$MODE" = dry ]; then log "dry: stop before any message (no LLM call)"; exit 0; fi

# --- 1 往復ずつ送って run の終端を待つ -----------------------------------------------------------
# run_id は POST の応答に頼らない（queued の message では null。dispatcher が後で run を割り当てる）。
# GET /chat/threads/{t}/messages を client_message_id で引き、run_id が付くのを待つ（上限つきの保険だけ時間で持つ）。
send() { # $1 key, $2 text, $3 attachment ids json
  python3 -c 'import json,sys; print(json.dumps({"client_message_id":sys.argv[1],"text":sys.argv[2],"attachment_ids":json.loads(sys.argv[3]),"reply_to_id":None,"mode":"queue","resume_queue":False}))' "$1" "$2" "$3" > "$EV/msg-$1.json"
  api -X POST -H 'content-type: application/json' "$API/chat/threads/$THREAD/messages" --data-binary "@$EV/msg-$1.json" > "$EV/post-$1.json"
  RUN=
  for _ in $(seq 1 150); do
    RUN=$(api "$API/chat/threads/$THREAD/messages?limit=200" | js '([m.get("run_id") for m in d["items"] if m.get("client_message_id")==sys.argv[2]] + [None])[0] or ""' "$1" 2>/dev/null || true)
    [ -n "$RUN" ] && break
    sleep 2
  done
  if [ -z "$RUN" ]; then log "turn $1: no run_id on GET messages"; S=none; return 0; fi
  log "turn $1 run_id=$RUN"
  S=
  for _ in $(seq 1 120); do
    api "$API/chat/threads/$THREAD/runs/$RUN" > "$EV/run-$1.json"
    S=$(js 'd["run"]["state"]' < "$EV/run-$1.json")
    case "$S" in completed|failed|cancelled|stopped|interrupted) break ;; esac
    sleep 5
  done
  log "turn $1 run $RUN state=$S session_mode=$(js 'd["run"].get("session_mode")' < "$EV/run-$1.json")"
}
send t1 "cos-chat-live です（試験用 daemon）。合言葉は「みかん42」です。覚えてください。返事は「了解」だけで十分です。" '[]'
send t2 "前の発言の合言葉を答えてください。また添付画像は何色の正方形ですか。2 点を 1 行で。" "[\"$A_BLUE\"]"
send t3 "試験用 daemon に task を 1 件起票してください。PATH の celerisctl を --api-url を付けずに使ってください（celerisctl add）。title は「cos-chat-live: CoS 起票の確認」、objective は「CoS チャットからの起票の実機確認。作業は不要。」、check-cmd は「true」、reason は「人が cos-chat-live で起票を依頼」。起票した task id を 1 行で返してください。" '[]'
# (d) D1: 起票の request（POST /api/v1/tasks）の attachment_ids で作成と pin を 1 回の operation にする
send t4 "添付の screenshot は設定画面です。赤いボタンが右端で画面からはみ出しています。ボタンが画面内に収まるよう直す UI 修正の task を起票し、この screenshot をその task に引き渡してください。cos-operator skill の attachments.md のとおり、POST /api/v1/tasks の本文の attachment_ids にこの添付 id を入れた 1 回の operation で起票と pin を行ってください（起票してから pin はしない）。作った task id と、operation の result に返った attachment_ids を返してください。" "[\"$A_SCREEN\"]"
# (e) D2: POST /api/v1/knowledge/inbox（operation knowledge.record）で候補作成と pin を 1 回にする
send t5 "添付の PDF を知識ベースに取り込みたいです。内容を読んで KB の候補（scope は project:agent-platform）を作り、PDF をその候補に pin してください。cos-operator skill の attachments.md のとおり、POST /api/v1/knowledge/inbox の本文の attachment_ids にこの添付 id を入れた 1 回の operation（knowledge.record）で行ってください（celerisctl knowledge record --attachment-id でも同じ operation になります）。候補 id と result に返った attachment_ids、PDF に書かれた codeword を返してください。" "[\"$A_PDF\"]"

# --- 証跡の採取（試験用 DB だけを読む） -----------------------------------------------------------
DB="$OUT/celeris.sqlite3"
q() { sqlite3 -header -separator ' | ' "$DB" "$1"; }
qv() { sqlite3 "$DB" "$1"; }
{
  echo "## chat_runs"; q "select run_id, state, reason, session_row_id, resolved_config_json, started_at, finished_at from chat_runs where thread_id='$THREAD' order by started_at"
  echo "## node_sessions"; q "select id, kind, thread_id, session_id, turns, summary_through_seq, model from node_sessions"
  echo "## messages"; q "select seq, role, state, run_id, substr(text,1,160) as text from chat_messages where thread_id='$THREAD' order by seq"
  echo "## cos_operations"; q "select id, action, state, target_id, reason, result_json from cos_operations order by created_at"
  echo "## events cos_operation"; q "select task_id, seq, json from events where json_extract(json,'$.type')='cos_operation' order by id"
  echo "## attachment refs (pins)"; q "select * from chat_attachment_refs order by created_at"
  echo "## tasks"; q "select id, status, kind, title from tasks order by created_at"
} > "$EV/db.txt" 2>&1 || true

# (d) task.create の operation が applied で、作成時の pin（owner_kind=task）が screenshot に付いているか
D_OP=$(qv "select id from cos_operations where action='task.create' and state='applied' and result_json like '%$A_SCREEN%' order by created_at limit 1" 2>/dev/null || true)
D_TASK=$(qv "select owner_id from chat_attachment_refs where attachment_id='$A_SCREEN' and owner_kind='task' limit 1" 2>/dev/null || true)
D_PIN_OP=$(qv "select count(*) from cos_operations where action='attachment.reference' and payload_json like '%$A_SCREEN%'" 2>/dev/null || echo 0)
log "(d) task.create op=${D_OP:-none} task=${D_TASK:-none} separate attachment.reference ops=$D_PIN_OP"
# 起票された UI 修正 task の最初の run が始まるまで待ち、入力 manifest（prompt.txt）と stage を見る
P=
if [ -n "$D_TASK" ]; then
  for _ in $(seq 1 60); do
    P=$(find "$OUT" -path "*$D_TASK*" -name prompt.txt 2>/dev/null | head -1 || true)
    [ -n "$P" ] || P=$(find "$OUT" -name prompt.txt -exec grep -l "$D_TASK" {} + 2>/dev/null | head -1 || true)
    [ -n "$P" ] && break; sleep 5
  done
fi
find "$OUT" -name prompt.txt > "$EV/prompt-files.txt" 2>/dev/null || true
if [ -n "$P" ] && grep -q '## 入力の添付' "$P"; then
  sed -n '/## 入力の添付/,/^## [^入]/p' "$P" > "$EV/input-manifest.txt"; log "manifest in $P"
else
  log "no input attachments in the first prompt.txt of ${D_TASK:-<no task>} (see prompt-files.txt)"
fi
find "$OUT/workspaces" -path '*attachments*' -type f -exec ls -l {} \; > "$EV/staged.txt" 2>/dev/null || true
if [ -n "$D_OP" ] && [ -n "$D_TASK" ] && [ -s "$EV/input-manifest.txt" ] && grep -q "$A_SCREEN\|screen.png" "$EV/input-manifest.txt"; then
  verdict "(d)" "PASS task=$D_TASK op=$D_OP (pinned at create; first run manifest lists screen.png)"
else
  verdict "(d)" "FAIL task=${D_TASK:-none} op=${D_OP:-none} manifest=$([ -s "$EV/input-manifest.txt" ] && echo yes || echo no)"
fi

# (e) knowledge.record の operation が applied で、GET /knowledge/inbox/{id} の provenance に PDF が出るか
E_OP=$(qv "select id from cos_operations where action='knowledge.record' and state='applied' order by created_at limit 1" 2>/dev/null || true)
E_CAND=$(qv "select owner_id from chat_attachment_refs where attachment_id='$A_PDF' and owner_kind='knowledge_inbox' limit 1" 2>/dev/null || true)
api "$API/knowledge/inbox" > "$EV/kb-inbox.json" || true
for c in $(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); [print(i["id"]) for i in d.get("items",[])]' "$EV/kb-inbox.json" 2>/dev/null); do
  api "$API/knowledge/inbox/$c" > "$EV/kb-inbox-$c.json" || true
done
E_PROV=
if [ -n "$E_CAND" ] && [ -s "$EV/kb-inbox-$E_CAND.json" ]; then
  E_PROV=$(js '",".join(p["attachment_id"] for p in d.get("provenance",[]))' < "$EV/kb-inbox-$E_CAND.json" 2>/dev/null || true)
fi
log "(e) knowledge.record op=${E_OP:-none} candidate=${E_CAND:-none} provenance=${E_PROV:-none}"
case ",$E_PROV," in
  *",$A_PDF,"*) [ -n "$E_OP" ] && verdict "(e)" "PASS candidate=$E_CAND op=$E_OP (provenance has $A_PDF)" \
                  || verdict "(e)" "FAIL candidate=$E_CAND has provenance but no applied knowledge.record operation" ;;
  *) verdict "(e)" "FAIL candidate=${E_CAND:-none} op=${E_OP:-none} provenance=${E_PROV:-none}" ;;
esac

cp "$OUT/daemon.log" "$EV/daemon.log"
log "done; evidence in $EV (verdict.txt)"
# どれか 1 つでも FAIL なら非 0 で終わる（運用セッションの live3 の指摘）
if grep -q 'FAIL' "$EV/verdict.txt"; then exit 1; fi
