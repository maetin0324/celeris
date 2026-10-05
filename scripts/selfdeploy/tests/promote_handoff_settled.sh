#!/usr/bin/env bash
# scripts/selfdeploy/tests/promote_handoff_settled.sh — ADR-0040 付記 2026-10-05（規則 4）:
#
#   promote.sh は「handoff done」を、新が active になっただけでなく、旧が draining（または消えた）ことを
#   `GET /api/v1/releases` の `instances` で確かめてから言う。`sd_instances_settled` / `sd_instances_verdict` の判定を見る。
#   付記 2026-10-05b: 生きている active（role active・drained_at 無し・pid が生きている）だけを数え、
#   読めない（401 など）ことと二重 active を区別する。
#
# 本番には触れない: 入力は一時ファイルの JSON。
# 実行: bash scripts/selfdeploy/tests/promote_handoff_settled.sh（lib.sh が bash の構文のため）
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
SD="$(cd "$HERE/.." && pwd)"

FAIL=0
ok() { echo "ok: $*"; }
ng() { echo "FAIL: $*" >&2; FAIL=1; }

# shellcheck source=/dev/null
. "$SD/lib.sh"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
NEW=0123456789ab
OLD=ba9876543210

check() {
  local desc="$1" want="$2" body="$3"
  printf '%s' "$body" >"$TMP/releases.json"
  if sd_instances_settled "$TMP/releases.json" "$NEW"; then got=0; else got=1; fi
  if [ "$got" = "$want" ]; then ok "$desc"; else ng "$desc: want exit $want, got $got"; fi
}

# 新だけが active → settled。
check "only the new instance is active" 0 \
  "{\"instances\":[{\"release\":\"$NEW\",\"role\":\"active\"}]}"
# 旧が draining で新が active → settled（旧の drain を待つ必要はない）。
check "old is draining, new is active" 0 \
  "{\"instances\":[{\"release\":\"$OLD\",\"role\":\"draining\"},{\"release\":\"$NEW\",\"role\":\"active\"}]}"
# 旧がまだ active（二重 active）→ not settled。
check "old is still active next to the new one" 1 \
  "{\"instances\":[{\"release\":\"$OLD\",\"role\":\"active\"},{\"release\":\"$NEW\",\"role\":\"active\"}]}"
# active が新でない（新が standby）→ not settled。
check "the new instance is only standby" 1 \
  "{\"instances\":[{\"release\":\"$OLD\",\"role\":\"active\"},{\"release\":\"$NEW\",\"role\":\"standby\"}]}"
# active が 1 つも無い → not settled。
check "no active instance" 1 \
  "{\"instances\":[{\"release\":\"$OLD\",\"role\":\"draining\"}]}"
# 本文が壊れている → not settled。
check "unreadable body" 1 "not json"

# ---- 付記 2026-10-05b: 生きている active = role active・drained_at 無し・pid が生きている ----------
# 判定の一語（`sd_instances_verdict`）と、死んだ pid・drained_at の扱い。pid はこの試験自身（生きている）と
# 起こして wait で回収した子（死んでいる）で作る（時計や heartbeat では決めない）。
ALIVE=$$
( : ) &
DEAD=$!
wait "$DEAD" || true
[ -e "/proc/$DEAD" ] && { echo "cannot make a dead pid" >&2; exit 1; }

verdict() {
  local desc="$1" want="$2" body="$3" got
  printf '%s' "$body" >"$TMP/releases.json"
  got="$(sd_instances_verdict "$TMP/releases.json" "$NEW")" || true
  if [ "$got" = "$want" ]; then ok "$desc"; else ng "$desc: want verdict $want, got $got"; fi
}

verdict "verdict: only the new is a live active" settled \
  "{\"instances\":[{\"release\":\"$NEW\",\"role\":\"active\",\"pid\":$ALIVE}]}"
verdict "verdict: old draining (alive), new active" settled \
  "{\"instances\":[{\"release\":\"$OLD\",\"role\":\"draining\",\"pid\":$ALIVE},{\"release\":\"$NEW\",\"role\":\"active\",\"pid\":$ALIVE}]}"
verdict "verdict: old row says active but its process is dead -> not counted" settled \
  "{\"instances\":[{\"release\":\"$OLD\",\"role\":\"active\",\"pid\":$DEAD},{\"release\":\"$NEW\",\"role\":\"active\",\"pid\":$ALIVE}]}"
verdict "verdict: old row says active but drained_at is set -> not counted" settled \
  "{\"instances\":[{\"release\":\"$OLD\",\"role\":\"active\",\"pid\":$ALIVE,\"drained_at\":\"2026-10-05T03:44:52Z\"},{\"release\":\"$NEW\",\"role\":\"active\",\"pid\":$ALIVE}]}"
verdict "verdict: old alive and active next to the new -> two_active" two_active \
  "{\"instances\":[{\"release\":\"$OLD\",\"role\":\"active\",\"pid\":$ALIVE},{\"release\":\"$NEW\",\"role\":\"active\",\"pid\":$ALIVE}]}"
verdict "verdict: pid unknown (0) counts as alive (conservative) -> two_active" two_active \
  "{\"instances\":[{\"release\":\"$OLD\",\"role\":\"active\",\"pid\":0},{\"release\":\"$NEW\",\"role\":\"active\",\"pid\":$ALIVE}]}"
verdict "verdict: new is standby, old active -> new_not_active" new_not_active \
  "{\"instances\":[{\"release\":\"$OLD\",\"role\":\"active\",\"pid\":$ALIVE},{\"release\":\"$NEW\",\"role\":\"standby\",\"pid\":$ALIVE}]}"
verdict "verdict: only a draining old -> no_active" no_active \
  "{\"instances\":[{\"release\":\"$OLD\",\"role\":\"draining\",\"pid\":$ALIVE}]}"
verdict "verdict: new active but its process is dead -> no_active" no_active \
  "{\"instances\":[{\"release\":\"$NEW\",\"role\":\"active\",\"pid\":$DEAD}]}"
verdict "verdict: 401 problem body (no instances) -> unreadable" unreadable \
  '{"type":"urn:celeris:problem:unauthorized","status":401}'
verdict "verdict: not json -> unreadable" unreadable 'not json'

# summary は生死と drained を見せる（ログで人が読む）。
printf '%s' "{\"instances\":[{\"release\":\"$OLD\",\"role\":\"active\",\"pid\":$DEAD},{\"release\":\"$NEW\",\"role\":\"draining\",\"pid\":$ALIVE,\"drained_at\":\"x\"}]}" >"$TMP/releases.json"
got="$(sd_instances_summary "$TMP/releases.json")"
want="$OLD:active pid=$DEAD dead $NEW:draining pid=$ALIVE alive drained"
if [ "$got" = "$want" ]; then ok "summary shows dead/alive/drained"; else ng "summary: want [$want] got [$got]"; fi

# DB の代替読み（本物の sqlite3 で一時 DB を作る。本番の DB には触れない）。
if command -v sqlite3 >/dev/null 2>&1; then
  sqlite3 "$TMP/t.sqlite3" "CREATE TABLE daemon_instances (instance_id TEXT PRIMARY KEY, \"release\" TEXT, pid INTEGER, role TEXT, started_at TEXT, heartbeat_at TEXT, handoff_requested_at TEXT, drained_at TEXT);
    INSERT INTO daemon_instances VALUES ('o','$OLD',$ALIVE,'draining','2026-10-05T03:00:00Z','2026-10-05T03:44:00Z','2026-10-05T03:44:51Z',NULL);
    INSERT INTO daemon_instances VALUES ('n','$NEW',$ALIVE,'active','2026-10-05T03:44:51Z','2026-10-05T03:44:53Z',NULL,NULL);"
  if sd_instances_from_db "$TMP/t.sqlite3" "$TMP/db.json" && [ "$(sd_instances_verdict "$TMP/db.json" "$NEW")" = settled ]; then
    ok "db fallback: daemon_instances read read-only -> settled"
  else
    ng "db fallback: could not read the temporary DB or wrong verdict ($(cat "$TMP/db.json" 2>/dev/null))"
  fi
  sqlite3 "$TMP/empty.sqlite3" "CREATE TABLE t (x);"
  if sd_instances_from_db "$TMP/empty.sqlite3" "$TMP/db2.json"; then ng "db fallback: a DB without daemon_instances must fail"; else ok "db fallback: a DB without daemon_instances fails (unreadable)"; fi
else
  echo "skip: sqlite3 not installed (db fallback not checked)"
fi

if [ "$FAIL" -ne 0 ]; then
  echo "FAILED" >&2
  exit 1
fi
echo "all promote_handoff_settled checks passed"
