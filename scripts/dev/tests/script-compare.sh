#!/usr/bin/env bash
# scripts/dev/script-compare.py の preflight・report の破れの検出・sample を一時の木で確かめる（境界 ADR 付記 P1-E）。
# 本物の store が書いた記録での集計は crates/celeris/tests/script_compare_report.rs。ここは実 migration の schema に手で
# 行を書いて、store が書かない破れ（未知の outcome・正規化・差分を隠した match）を作る。本番に触れない。
# 使い方: bash scripts/dev/tests/script-compare.sh [試す script の path。既定は repo の script]
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
tool="${1:-$root/scripts/dev/script-compare.py}"
work="$(mktemp -d)"
srv=""
cleanup() { [ -z "$srv" ] || kill "$srv" 2>/dev/null || true; rm -rf "$work"; }
trap cleanup EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }
run() { python3 -I "$tool" "$@"; }
expect() { # expect <exit> <desc> <cmd...>（stdout を $work/out に残す）
  local want="$1" desc="$2" rc=0
  shift 2
  "$@" >"$work/out" 2>"$work/err" || rc=$?
  [ "$rc" = "$want" ] || { cat "$work/out" "$work/err" >&2; fail "$desc: exit $rc（期待 $want）"; }
  echo "ok: $desc (exit $rc)"
}
has() { grep -q -F -- "$1" "$work/out" || { cat "$work/out" >&2; fail "出力に無い: $1"; }; }

# --- 読み取り専用の固定: script の SQL に書き込みの語が無く、sqlite3 は -readonly と query_only で開く ---
if grep -n -E '\b(INSERT|UPDATE|DELETE|DROP|ALTER|REPLACE|VACUUM|CREATE)\b' "$tool"; then fail "書き込みの SQL がある"; fi
grep -q '"-readonly"' "$tool" || fail "-readonly が無い"
grep -q 'PRAGMA query_only=ON' "$tool" || fail "query_only が無い"

# --- 実 migration の schema の一時 DB ---
schema_db() { # schema_db <path> <schema の版>
  {
    echo "CREATE TABLE events (id INTEGER PRIMARY KEY, task_id TEXT NOT NULL, seq INTEGER NOT NULL, ts TEXT NOT NULL, json TEXT NOT NULL);"
    echo "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);"
    echo "INSERT INTO schema_migrations VALUES ($2, 'x');"
    echo "CREATE TABLE chat_messages (id TEXT PRIMARY KEY, client_message_id TEXT, created_at TEXT NOT NULL);"
    cat "$root"/crates/task-core/migrations/0103_script_domains.sql
    cat "$root"/crates/task-core/migrations/0104_script_cos_digest.sql
    echo "ALTER TABLE script_domain_modes ADD COLUMN canary_bps INTEGER;"
    cat "$root"/crates/task-core/migrations/0105_script_generations_s5.sql
    cat "$root"/crates/task-core/migrations/0106_script_inbox.sql
    cat "$root"/crates/task-core/migrations/0107_cos_digest_shadow_outbox.sql
  } | sqlite3 "$1"
}
insert_record() { # insert_record <db> <id> <outcome> [normalized の JSON] [ts の JSON]
  local normalized="${4:-[]}" ts="${5:-{\"a\":1\}}"
  sqlite3 "$1" "INSERT INTO events VALUES ($2,'t',$2,'2026-11-03T00:00:0$2Z',json_object('type','script_compare_recorded','domain','lane_rules','generation','g1','bundle_sha256','s','outcome','$3','error',NULL,'elapsed_us',10,'normalized',json('$normalized'),'safety_violations',json('[]'),'canary_selected',json('false'),'rust',json('{\"a\":1}'),'ts',json('$ts')));"
}
guard_row() { sqlite3 "$1" "INSERT OR REPLACE INTO script_guard_state (domain, generation_id, mode, calls, clean_streak, updated_at) VALUES ('lane_rules','g1','compare',$2,$3,'x');"; }

db="$work/report.sqlite3"
schema_db "$db" 107
sqlite3 "$db" "INSERT INTO script_generations VALUES ('g1','s','1.0.0','x','{}','active','x','x');
UPDATE script_domain_modes SET mode='compare', generation_id='g1', version=1 WHERE domain='lane_rules';"
insert_record "$db" 1 match
guard_row "$db" 1 1
expect 0 "矛盾の無い 1 件は exit 0" run report --db "$db"
has '"match_rate": 1.0'
expect 4 "1 件では ready でない" run report --db "$db" --require-ready lane_rules
expect 0 "表の形式" run report --db "$db" --format table
has 'lane_rules g1 1 0 0 0 0 1.0'
has 'not ready: clean_streak 1 < 1000'

# report の前後で DB の byte が変わらない。
before="$(sha256sum "$db")"
run report --db "$db" >/dev/null
[ "$before" = "$(sha256sum "$db")" ] || fail "report が DB を変えた"
# -readonly で開いた sqlite3 は書けない（script の開き方の前提）。
if sqlite3 -readonly -batch "$db" "INSERT INTO schema_migrations VALUES (999,'x')" 2>/dev/null; then fail "-readonly で書けた"; fi

# 未知の outcome（将来の variant を含む）は数えずに止める。
cp "$db" "$work/unknown.sqlite3"
insert_record "$work/unknown.sqlite3" 2 explained
guard_row "$work/unknown.sqlite3" 1 1
expect 1 "未知の outcome は exit 1" run report --db "$work/unknown.sqlite3"
has "unknown outcome 'explained'"
# 正規化の記録は差分を隠す疑いとして止める（S2-2 に規則が無い）。
cp "$db" "$work/norm.sqlite3"
insert_record "$work/norm.sqlite3" 2 match '["lowercase"]'
guard_row "$work/norm.sqlite3" 2 2
expect 1 "正規化の記録は exit 1" run report --db "$work/norm.sqlite3"
has 'normalized is not empty'
# match なのに rust と ts が違う記録も止める。
cp "$db" "$work/hide.sqlite3"
insert_record "$work/hide.sqlite3" 2 match '[]' '{"a":2}'
guard_row "$work/hide.sqlite3" 2 2
expect 1 "差分を隠した match は exit 1" run report --db "$work/hide.sqlite3"
has 'outcome match but rust != ts'
# 読めない標本・無い DB・表の無い DB。
printf 'not json\n' >"$work/bad.jsonl"
expect 1 "壊れた標本は exit 1" run report --db "$db" --samples "$work/bad.jsonl"
expect 2 "無い DB は exit 2" run report --db "$work/none.sqlite3"
sqlite3 "$work/empty.sqlite3" "CREATE TABLE x (y);"
expect 3 "表の無い DB は exit 3" run report --db "$work/empty.sqlite3"
has 'tables_missing'

# 標本と log の集計。
cat >"$work/samples.jsonl" <<'S'
{"at":"2026-11-03T00:00:00Z","pid":7,"rss_kb":100,"script_tick_wait":{"ticks":10,"p50_us":0,"p95_us":0,"max_us":0,"tick_budget":0,"budget_us":200000}}
{"at":"2026-11-03T01:00:00Z","pid":7,"rss_kb":150,"script_tick_wait":{"ticks":10,"p50_us":1,"p95_us":5,"max_us":9,"tick_budget":0,"budget_us":200000}}
S
cat >"$work/tick.log" <<'S'
INFO slow tick elapsed_ms=2500
INFO slow tick phases tunnel_ms=2000
INFO script_tick_wait ticks=10
WARN script_tick_wait tick_budget=2
S
expect 0 "標本と log の集計" run report --db "$db" --samples "$work/samples.jsonl" --tick-log "$work/tick.log"
has '"growth": 50'
has '"p95_us_max": 5'
has '"slow_ticks": 1'
has '"tick_budget_warnings": 1'

# --- sample: 偽の /proc と API の stub ---
mkdir -p "$work/proc/4242"
printf 'Name:\tceleris\nVmHWM:\t 2048 kB\nVmRSS:\t 1024 kB\nThreads:\t9\n' >"$work/proc/4242/status"
printf 'secret-token\n' >"$work/token"
cat >"$work/stub.py" <<'PY'
import http.server, json, sys
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        ok = self.path == "/api/v1/daemon" and self.headers.get("Authorization") == "Bearer secret-token"
        body = json.dumps({"now": "x", "snapshot": {"script_tick_wait": {"ticks": 3, "p50_us": 1, "p95_us": 2, "max_us": 3,
                           "tick_budget": 0, "budget_us": 200000}}}).encode()
        self.send_response(200 if ok else 401); self.end_headers(); self.wfile.write(body if ok else b"{}")
    def do_POST(self):
        raise SystemExit("sample must not POST")
    def log_message(self, *a): pass
s = http.server.HTTPServer(("127.0.0.1", 0), H)
open(sys.argv[1], "w").write(str(s.server_address[1]))
s.serve_forever()
PY
python3 -I "$work/stub.py" "$work/port" &
srv=$!
for _ in $(seq 1 100); do [ -s "$work/port" ] && break; sleep 0.05; done
expect 0 "sample は GET /daemon と status を読む" run sample --pid 4242 --proc-root "$work/proc" --api "http://127.0.0.1:$(cat "$work/port")" --token-file "$work/token"
has '"rss_kb": 1024'
has '"p95_us": 2'
if grep -q secret-token "$work/out"; then fail "sample の出力に token がある"; fi
expect 2 "API の 401 は exit 2" run sample --pid 4242 --proc-root "$work/proc" --api "http://127.0.0.1:$(cat "$work/port")" --token-file "$work/proc/4242/status"
expect 2 "sample の引数の不足" run sample --pid 4242 --proc-root "$work/proc"

# --- preflight: 偽の本番・測定・/proc ---
prod="$work/prod"; meas="$work/measure"; mbin="$work/measure-bin"; state="$work/state"; proc="$work/fakeproc"
fresh_tree() {
  rm -rf "$prod" "$meas" "$mbin" "$state" "$proc"
  mkdir -p "$prod" "$meas/k2/b" "$mbin/rel1/bin" "$state/releases/new/script-bundle" "$proc"
  schema_db "$prod/celeris.sqlite3" 107
  cat >"$prod/config.toml" <<C
[db]
path = "$prod/celeris.sqlite3"

[api]
listen = "127.0.0.1:7710"

[llm_proxy]
listen = "127.0.0.1:18100"

[script]
state_dir = "$state"
C
  schema_db "$meas/k2/b/celeris.db" 102
  cat >"$meas/k2/b/daemon.toml" <<C
db = "$meas/k2/b/celeris.db"

[api]
listen = "127.0.0.1:54827"
C
  cat >"$state/releases/new/gate.json" <<'G'
{"ok":true,"steps":[{"step":"control-ts-install","exit":0},{"step":"control-ts-bundle","exit":0},{"step":"script-bundle-verify","exit":0},{"step":"script-bundle-tests","exit":0}]}
G
  daemon_proc 100 "$state/releases/new/bin/celeris" "$prod/config.toml" new
  daemon_proc 200 "$mbin/rel1/bin/celeris" "$meas/k2/b/daemon.toml" rel1
  # bwrap の包みは daemon に数えない。
  mkdir -p "$proc/201"; printf 'bwrap\0--ro-bind\0--config\0x\0' >"$proc/201/cmdline"
}
daemon_proc() { mkdir -p "$proc/$1"; printf '%s\0--config\0%s\0--release\0%s\0' "$2" "$3" "$4" >"$proc/$1/cmdline"; }
pre() { run preflight --config "$prod/config.toml" --state-dir "$state" --measure-root "$meas" --measure-bin "$mbin" --proc-root "$proc" "$@"; }

fresh_tree
expect 0 "全部整った preflight は exit 0" pre --estimate-since 2026-10-20T00:00:00Z
has '"lane_rules_records_after_dedup": 0'
# 見込みは記録の重複の抑え（同じ task の直前と同じ入力は 1 件）を当てて数える。
routing() { sqlite3 "$prod/celeris.sqlite3" "INSERT INTO events VALUES ($1,'$2',$1,'2026-10-2$1T00:00:00Z',json_object('type','routing_decided','record',json_object('decision',json_object('source','$3','features',json('{\"f\":$4}')))));"; }
routing 1 t1 default 1; routing 2 t1 default 1; routing 3 t1 default 2; routing 4 t2 hint 1; routing 5 t3 human 1
expect 0 "見込みの数え方" pre --estimate-since 2026-10-20T00:00:00Z
has '"lane_rules_policy_runs": 4'
has '"lane_rules_distinct_tasks": 2'
has '"lane_rules_records_after_dedup": 3'
expect 2 "見込みの時刻の形が違えば exit 2" pre --estimate-since "x' OR 1=1"
# 測定の DB は開かない（-shm・-wal を作らず、byte も変わらない）。
[ ! -e "$meas/k2/b/celeris.db-shm" ] || fail "測定の DB を開いた"

fresh_tree
rm -rf "$state/releases/new/script-bundle"
expect 3 "gate の bundle が無い release は human" pre
has '"id": "release_gate_has_script_steps"'

fresh_tree
sed -i '/^\[script\]/,$d' "$prod/config.toml"
expect 3 "[script] が無い本番は human" pre

fresh_tree
sqlite3 "$prod/celeris.sqlite3" "DELETE FROM schema_migrations; INSERT INTO schema_migrations VALUES (102,'x');"
expect 3 "古い schema は human" pre

fresh_tree
printf 'active_bundle_sha256 = "x"\n' >>"$prod/config.toml"
expect 1 "消した欄が残る本番は fail" pre

fresh_tree
sqlite3 "$prod/celeris.sqlite3" "INSERT INTO script_generations VALUES ('g1','s','1.0.0','x','{}','active','x','x');
UPDATE script_domain_modes SET mode='compare', generation_id='g1', version=1 WHERE domain='lane_rules';"
expect 1 "本番が rust でなければ fail" pre

fresh_tree
sed -i "s|^db = .*|db = \"$prod/celeris.sqlite3\"|" "$meas/k2/b/daemon.toml"
expect 1 "測定が本番と同じ DB なら fail" pre
has '測定'

fresh_tree
sed -i 's|127.0.0.1:54827|127.0.0.1:7710|' "$meas/k2/b/daemon.toml"
expect 1 "測定が本番と同じ listen なら fail" pre

fresh_tree
printf '\n[script]\nstate_dir = "/x"\n' >>"$meas/k2/b/daemon.toml"
expect 1 "測定の config に [script] があれば fail" pre

fresh_tree
printf '\n[llm]\nbase = "http://127.0.0.1:18100"\n' >>"$meas/k2/b/daemon.toml"
expect 1 "測定が本番の llm proxy を指せば fail" pre

fresh_tree
printf '\n[selfdeploy]\nreleases_dir = "%s/releases"\n' "$state" >>"$meas/k2/b/daemon.toml"
expect 1 "動いている測定が本番の releases を指せば fail" pre
rm -rf "$proc/200"
expect 3 "止まった測定の config の参照は human" pre
has '"id": "stopped_measure_configs_refs"'

fresh_tree
printf '\n[x]\npath = "%s/k2/out"\n' "$meas" >>"$prod/config.toml"
expect 1 "本番が測定の dir を指せば fail" pre

fresh_tree
sed -i "s|^state_dir = .*|state_dir = \"$meas/k2/state\"|" "$prod/config.toml"
expect 1 "[script] state_dir が測定の下なら fail" pre

fresh_tree
daemon_proc 200 "$state/releases/new/bin/celeris" "$meas/k2/b/daemon.toml" new
expect 1 "測定の daemon が本番の release の binary なら fail" pre

fresh_tree
rm -rf "$mbin/rel1"; ln -s "$state/releases/new" "$mbin/rel1"
expect 1 "測定の binary が本番の release の symlink なら fail" pre

fresh_tree
rm -rf "$proc/100"
expect 1 "対象の daemon が動いていなければ fail" pre

expect 2 "読めない config は exit 2" run preflight --config "$work/none.toml" --state-dir "$state"
echo "script-compare: ok"
