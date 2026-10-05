#!/usr/bin/env bash
# scripts/selfdeploy/tests/promote_live_abort.sh — ADR-0040 付記 2026-10-05b（promote.sh の打ち切りの安全側）:
#
#   本番 2026-10-05 03:44〜03:46 UTC: promote.sh de69d5634efb は `GET /api/v1/releases` をトークン無しで読み
#   （401）、読めないことを「別の active がいる」と記録して、引き継ぎ済みの新 instance を止めた。旧は既に
#   draining（API を閉じていた）ので active が 0 になり、health が 000 になった。
#
#   ここで確かめること（promote.sh を偽の systemctl / curl / sqlite3 / ss で丸ごと走らせる）:
#     A  再現: `/api/v1/releases` が Bearer を要求し、旧は draining、新は active → handoff done（新を止めない）。
#        修正前の promote.sh（トークン無し）ではこのケースが「two active instances」で失敗し、新を止める。
#     B  API が読めない（401）が DB の daemon_instances は読める → DB で settled → handoff done。
#     C  API も DB も読めない → 新を止めない。失敗の文言は「not confirmed … left running」。current は旧のまま。
#     D  旧が生きた active のまま、新も active（二重 active）→ 新を止め、旧が health に答える → 新は起こし直さない。
#     E  二重 active と読めたが、新を止めた後に誰も health に答えない → 新を起こし直す（active 0 を残さない）。
#     F  旧の行が active でもプロセスが死んでいる（pid 無し）→ active に数えない → handoff done。
#     G  旧の行が active でも drained_at が付いている → active に数えない → handoff done。
#
# 本番には触れない: HOME / CELERIS_CONFIG_DIR / CELERIS_STATE_DIR / CELERIS_DB は一時ディレクトリ。
# 待ち時間は SD_SETTLE_WAIT / SD_ACTIVE_RECHECK_WAIT / SD_HANDOFF_WAIT で短くする。
# 実行: bash scripts/selfdeploy/tests/promote_live_abort.sh
#   SD_UNDER_TEST=<dir> で別の版の scripts/selfdeploy（例: git show で取り出した修正前）を同じ試験に掛けられる。
#   SD_TEST_CASES="A E" で走らせるケースを絞れる（既定は全部）。
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
SD="${SD_UNDER_TEST:-$(cd "$HERE/.." && pwd)}"

FAIL=0
ok() { echo "ok: $*"; }
ng() { echo "FAIL: $*" >&2; FAIL=1; }
assert_eq() {
  local desc="$1" want="$2" got="$3"
  if [ "$want" = "$got" ]; then ok "$desc"; else ng "$desc (want=[$want] got=[$got])"; fi
}
assert_log() {
  local desc="$1" pattern="$2"
  if grep -qF -- "$pattern" "$CASE/out.log"; then ok "$desc"; else ng "$desc: log lacks [$pattern]"; fi
}
assert_no_log() {
  local desc="$1" pattern="$2"
  if grep -qF -- "$pattern" "$CASE/out.log"; then ng "$desc: log has [$pattern]"; else ok "$desc"; fi
}
count_calls() { grep -c -- "$1" "$FAKE_STATE/systemctl.log" 2>/dev/null || true; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

OLD=aaaaaaaaaaaa
NEW=bbbbbbbbbbbb
TOKEN=test-token-0123
FAKE_BIN="$WORK/fakebin"
mkdir -p "$FAKE_BIN"

# 生きている pid（この試験自身）と、死んだ pid（起こして wait で回収した子）。
ALIVE_PID=$$
( : ) &
DEAD_PID=$!
wait "$DEAD_PID" || true
[ -e "/proc/$DEAD_PID" ] && { echo "cannot make a dead pid" >&2; exit 1; }

# ---- 偽の道具 ----------------------------------------------------------------

# systemctl: 呼び出しを記録。start/stop は $FAKE_STATE/started-<unit> の有無で表す。
cat >"$FAKE_BIN/systemctl" <<'EOF'
#!/usr/bin/env bash
echo "$*" >>"$FAKE_STATE/systemctl.log"
[ "${1:-}" = --user ] && shift
cmd="${1:-}"; shift || true
case "$cmd" in
  cat) exit 0 ;;
  start) touch "$FAKE_STATE/started-$1" "$FAKE_STATE/ever-started-$1"; exit 0 ;;
  stop) rm -f "$FAKE_STATE/started-$1"; exit 0 ;;
  is-active) exit 1 ;;
  enable | disable | daemon-reload) exit 0 ;;
  show) echo 0; exit 0 ;;
  list-units) exit 0 ;;
  *) exit 0 ;;
esac
EOF

# curl: -o <file> に本文、http code を標準出力へ。
#   /api/v1/health   新が start 済みなら新（active）、そうでなく $FAKE_OLD_HEALTH=1 なら旧（active）。
#                    $FAKE_OLD_DRAINS_ON_NEW=1 なら、新が一度でも start した後の旧は答えない（draining で
#                    listener を閉じた実機の旧）。どちらも無ければ 000（誰も答えない）。
#   /api/v1/releases Authorization: Bearer <token> が無ければ 401。あれば $FAKE_STATE/releases.json を返す
#                    （$FAKE_RELEASES_HTTP が立っていればその code で本文無し）。
#   /healthz (GUI)   start 済みの GUI。
cat >"$FAKE_BIN/curl" <<'EOF'
#!/usr/bin/env bash
out=/dev/null url="" auth=""
while [ $# -gt 0 ]; do
  case "$1" in
    -o) out="$2"; shift ;;
    -H) case "$2" in Authorization:*) auth="${2#Authorization: }" ;; esac; shift ;;
    -w | -m | -X | --data-binary | --max-redirs | --connect-timeout | --max-time) shift ;;
    -*) ;;
    *) url="$1" ;;
  esac
  shift
done
echo "$url auth=[$auth]" >>"$FAKE_STATE/curl.log"
body="" code=000
case "$url" in
  *:7710/api/v1/health)
    if [ -e "$FAKE_STATE/started-celeris@$FAKE_NEW" ]; then
      body="{\"release\":\"$FAKE_NEW\",\"role\":\"active\",\"schema_version\":47}"; code=200
    elif [ "${FAKE_OLD_HEALTH:-0}" = 1 ] && [ -e "$FAKE_STATE/started-celeris@$FAKE_OLD" ] \
      && ! { [ "${FAKE_OLD_DRAINS_ON_NEW:-0}" = 1 ] && [ -e "$FAKE_STATE/ever-started-celeris@$FAKE_NEW" ]; }; then
      body="{\"release\":\"$FAKE_OLD\",\"role\":\"active\",\"schema_version\":47}"; code=200
    fi ;;
  *:7710/api/v1/releases)
    if [ -n "${FAKE_RELEASES_HTTP:-}" ]; then
      code="$FAKE_RELEASES_HTTP"
    elif [ "$auth" != "Bearer $FAKE_TOKEN" ]; then
      body='{"type":"urn:celeris:problem:unauthorized","status":401}'; code=401
    else
      body="$(cat "$FAKE_STATE/releases.json")"; code=200
    fi ;;
  *:7700/healthz)
    for sha in $FAKE_NEW $FAKE_OLD; do
      if [ -e "$FAKE_STATE/started-celeris-gui@$sha" ]; then
        body="{\"release\":\"$sha\",\"name\":\"celeris-gui\"}"; code=200; break
      fi
    done ;;
esac
printf '%s' "$body" >"$out"
printf '%s' "$code"
[ "$code" = 000 ] && exit 7
exit 0
EOF

# sqlite3: `.backup` は空ファイル。`-json` の SELECT は $FAKE_STATE/db-instances.json の中身
# （無ければ exit 1 = DB が読めない）。それ以外（schema_version の問い合わせ）は 47。
cat >"$FAKE_BIN/sqlite3" <<'EOF'
#!/usr/bin/env bash
json=0
for a in "$@"; do
  case "$a" in
    .backup*) p="${a#.backup }"; p="${p#\'}"; p="${p%\'}"; echo fake-db >"$p"; exit 0 ;;
    -json) json=1 ;;
  esac
done
if [ "$json" = 1 ]; then
  [ -f "$FAKE_STATE/db-instances.json" ] || exit 1
  cat "$FAKE_STATE/db-instances.json"
  exit 0
fi
echo 47
EOF

# ss: 何も LISTEN していない。
cat >"$FAKE_BIN/ss" <<'EOF'
#!/usr/bin/env bash
echo "State Recv-Q Send-Q Local Address:Port Peer Address:Port Process"
EOF
chmod +x "$FAKE_BIN"/*

# ---- 一時の state ----------------------------------------------------------------

row() { # row <release> <role> <pid> [drained_at]
  if [ -n "${4:-}" ]; then
    printf '{"instance_id":"%s","release":"%s","pid":%s,"role":"%s","started_at":"2026-10-05T03:00:00Z","heartbeat_at":"2026-10-05T03:44:00Z","handoff_requested_at":null,"drained_at":"%s"}' "$1-$2" "$1" "$3" "$2" "$4"
  else
    printf '{"instance_id":"%s","release":"%s","pid":%s,"role":"%s","started_at":"2026-10-05T03:00:00Z","heartbeat_at":"2026-10-05T03:44:00Z","handoff_requested_at":null,"drained_at":null}' "$1-$2" "$1" "$3" "$2"
  fi
}
instances_json() { # instances_json <row>... → {"instances":[...]}
  local out="" r
  for r in "$@"; do out="$out${out:+,}$r"; done
  printf '{"instances":[%s]}' "$out"
}

setup() {
  local name="$1" sha
  CASE="$WORK/$name"
  mkdir -p "$CASE/home" "$CASE/fake"
  export HOME="$CASE/home"
  export CELERIS_CONFIG_DIR="$HOME/.config/celeris"
  export CELERIS_STATE_DIR="$HOME/.local/celeris"
  export CELERIS_DB="$CELERIS_STATE_DIR/celeris.sqlite3"
  export FAKE_STATE="$CASE/fake" FAKE_OLD="$OLD" FAKE_NEW="$NEW" FAKE_TOKEN="$TOKEN"
  export SD_REPO="$CASE/no-repo"
  unset CELERIS_CONFIG SD_LOG_FILE FAKE_RELEASES_HTTP FAKE_OLD_HEALTH FAKE_OLD_DRAINS_ON_NEW
  # 本番の paths.env（CELERIS_BACKUPS_DIR / CELERIS_LOGS_DIR）を環境から継がない（log と backup を一時側へ）。
  unset CELERIS_BACKUPS_DIR CELERIS_LOGS_DIR
  mkdir -p "$CELERIS_CONFIG_DIR" "$CELERIS_STATE_DIR/releases" "$CELERIS_STATE_DIR/backups"
  printf 'db = "%s"\n' "$CELERIS_DB" >"$CELERIS_CONFIG_DIR/config.toml"
  printf '%s\n' "$TOKEN" >"$CELERIS_CONFIG_DIR/api.token"
  echo fake-db >"$CELERIS_DB"
  for sha in "$OLD" "$NEW"; do
    mkdir -p "$CELERIS_STATE_DIR/releases/$sha/bin"
    printf '#!/bin/sh\nexit 0\n' >"$CELERIS_STATE_DIR/releases/$sha/bin/celeris"
    chmod +x "$CELERIS_STATE_DIR/releases/$sha/bin/celeris"
    printf '{"schema_version": 47}\n' >"$CELERIS_STATE_DIR/releases/$sha/manifest.json"
    printf '{"ok": true, "live_ok": true}\n' >"$CELERIS_STATE_DIR/releases/$sha/verify.json"
  done
  ln -sfn "releases/$OLD" "$CELERIS_STATE_DIR/current"
  # 旧が動いている（live 昇格になる）。
  touch "$FAKE_STATE/started-celeris@$OLD" "$FAKE_STATE/started-celeris-gui@$OLD"
  export FAKE_OLD_HEALTH=1
}

run_promote() {
  set +e
  PATH="$FAKE_BIN:$PATH" SD_STOP_WAIT=1 SD_SETTLE_WAIT=2 SD_ACTIVE_RECHECK_WAIT=2 SD_HANDOFF_WAIT=5 \
    bash "$SD/promote.sh" "$NEW" >"$CASE/out.log" 2>&1
  RC=$?
  set -e
}
dump_on_fail() { if [ "$FAIL" -ne 0 ]; then echo "---- $CASE/out.log" >&2; cat "$CASE/out.log" >&2; echo "---- systemctl.log" >&2; cat "$FAKE_STATE/systemctl.log" >&2; FAIL=0; FAILED_CASES="$FAILED_CASES $CASE"; fi; }
FAILED_CASES=""
want_case() { case " ${SD_TEST_CASES:-A B C D E F G} " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

# ---- A: 再現（/api/v1/releases は Bearer 必須。旧 draining・新 active）-----------------------
if want_case A; then
setup A-token-required
instances_json "$(row "$OLD" draining "$ALIVE_PID")" "$(row "$NEW" active "$ALIVE_PID")" >"$FAKE_STATE/releases.json"
run_promote
assert_eq "A: promote.sh exit" 0 "$RC"
assert_eq "A: the new instance was not stopped" 0 "$(count_calls "stop celeris@$NEW")"
assert_log "A: handoff done" "handoff done"
assert_no_log "A: no 'two active instances'" "two active instances"
assert_eq "A: current -> new" "releases/$NEW" "$(readlink "$CELERIS_STATE_DIR/current")"
grep -q "api/v1/releases auth=\[Bearer $TOKEN\]" "$FAKE_STATE/curl.log" && ok "A: /api/v1/releases was read with the token" || ng "A: /api/v1/releases was read without the token"
dump_on_fail
fi

# ---- B: API は 401（トークンが合わない）が DB は読める → DB で settled -------------------------
if want_case B; then
setup B-db-fallback
printf 'wrong-token\n' >"$CELERIS_CONFIG_DIR/api.token"
instances_json "$(row "$OLD" draining "$ALIVE_PID")" "$(row "$NEW" active "$ALIVE_PID")" >"$FAKE_STATE/releases.json"
printf '[%s,%s]' "$(row "$OLD" draining "$ALIVE_PID")" "$(row "$NEW" active "$ALIVE_PID")" >"$FAKE_STATE/db-instances.json"
run_promote
assert_eq "B: promote.sh exit" 0 "$RC"
assert_eq "B: the new instance was not stopped" 0 "$(count_calls "stop celeris@$NEW")"
assert_log "B: fell back to the DB" "reading daemon_instances from the DB read-only"
assert_log "B: settled via db" "daemon_instances (db): the only live active is $NEW"
assert_eq "B: current -> new" "releases/$NEW" "$(readlink "$CELERIS_STATE_DIR/current")"
dump_on_fail
fi

# ---- C: API も DB も読めない → 新を残して失敗、文言は実態どおり -----------------------------------
if want_case C; then
setup C-unreadable
export FAKE_RELEASES_HTTP=401
run_promote
[ "$RC" -ne 0 ] && ok "C: promote.sh failed (exit $RC)" || ng "C: promote.sh should fail"
assert_eq "C: the new instance was not stopped" 0 "$(count_calls "stop celeris@$NEW")"
[ -e "$FAKE_STATE/started-celeris@$NEW" ] && ok "C: the new instance is still running" || ng "C: the new instance is not running"
assert_log "C: wording says it was left running" "was left running"
assert_log "C: wording says not confirmed" "live handoff not confirmed"
assert_no_log "C: no 'two active instances'" "two active instances"
assert_eq "C: current unchanged" "releases/$OLD" "$(readlink "$CELERIS_STATE_DIR/current")"
[ -f "$CELERIS_STATE_DIR/releases/$NEW/promote_failed.json" ] && ok "C: promote_failed.json written (GUI banner)" || ng "C: promote_failed.json missing"
dump_on_fail
fi

# ---- D: 二重 active（旧は生きた active のまま）→ 新を止め、旧が答えるので起こし直さない -------------
if want_case D; then
setup D-two-active
instances_json "$(row "$OLD" active "$ALIVE_PID")" "$(row "$NEW" active "$ALIVE_PID")" >"$FAKE_STATE/releases.json"
run_promote
[ "$RC" -ne 0 ] && ok "D: promote.sh failed (exit $RC)" || ng "D: promote.sh should fail"
assert_eq "D: the new instance was stopped once" 1 "$(count_calls "stop celeris@$NEW")"
assert_eq "D: the new instance was started once (not again)" 1 "$(count_calls "start celeris@$NEW")"
assert_log "D: wording says two active instances" "two active instances (the old celeris was still active"
assert_log "D: an active remained" "an active celeris is still serving after stopping celeris@$NEW"
assert_eq "D: current unchanged" "releases/$OLD" "$(readlink "$CELERIS_STATE_DIR/current")"
dump_on_fail
fi

# ---- E: 二重 active と読めたが、新を止めると誰も答えない → 新を起こし直す -------------------------
if want_case E; then
setup E-restart-new
export FAKE_OLD_DRAINS_ON_NEW=1
instances_json "$(row "$OLD" active "$ALIVE_PID")" "$(row "$NEW" active "$ALIVE_PID")" >"$FAKE_STATE/releases.json"
run_promote
[ "$RC" -ne 0 ] && ok "E: promote.sh failed (exit $RC)" || ng "E: promote.sh should fail"
assert_eq "E: the new instance was stopped once" 1 "$(count_calls "stop celeris@$NEW")"
assert_eq "E: the new instance was started twice (restarted)" 2 "$(count_calls "start celeris@$NEW")"
[ -e "$FAKE_STATE/started-celeris@$NEW" ] && ok "E: the new instance is running at the end" || ng "E: the new instance is not running at the end"
assert_log "E: wording says it was started again" "starting celeris@$NEW again"
assert_log "E: wording asks to rerun" "rerun promote.sh $NEW"
assert_eq "E: current unchanged" "releases/$OLD" "$(readlink "$CELERIS_STATE_DIR/current")"
dump_on_fail
fi

# ---- F: 旧の行は active だがプロセスが死んでいる → active に数えない -------------------------------
if want_case F; then
setup F-dead-old
instances_json "$(row "$OLD" active "$DEAD_PID")" "$(row "$NEW" active "$ALIVE_PID")" >"$FAKE_STATE/releases.json"
run_promote
assert_eq "F: promote.sh exit" 0 "$RC"
assert_eq "F: the new instance was not stopped" 0 "$(count_calls "stop celeris@$NEW")"
assert_log "F: the dead row is logged as dead" "$OLD:active pid=$DEAD_PID dead"
assert_eq "F: current -> new" "releases/$NEW" "$(readlink "$CELERIS_STATE_DIR/current")"
dump_on_fail
fi

# ---- G: 旧の行は active だが drained_at が付いている → active に数えない -----------------------------
if want_case G; then
setup G-drained-old
instances_json "$(row "$OLD" active "$ALIVE_PID" 2026-10-05T03:44:52Z)" "$(row "$NEW" active "$ALIVE_PID")" >"$FAKE_STATE/releases.json"
run_promote
assert_eq "G: promote.sh exit" 0 "$RC"
assert_eq "G: the new instance was not stopped" 0 "$(count_calls "stop celeris@$NEW")"
assert_eq "G: current -> new" "releases/$NEW" "$(readlink "$CELERIS_STATE_DIR/current")"
dump_on_fail
fi

if [ -n "$FAILED_CASES" ]; then
  echo "promote_live_abort: FAILED ($FAILED_CASES)" >&2
  exit 1
fi
echo "promote_live_abort: all ok"
