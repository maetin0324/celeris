#!/usr/bin/env bash
# 境界 ADR 付記 P1-E: 本番 compare の人の手順（配備→登録→compare→集計→戻し）を、一時 DB の daemon で同じコマンドのまま通す。
# 使い方: bash scripts/dev/script-compare-rehearsal.sh --out <jsonl>
#   CARGO_TARGET_DIR が要る。control-ts/node_modules が要る（無ければ offline で pnpm install してから）。
# 本番の mode・deploy・state dir・DB・config に触れない。daemon は $TMPDIR の一時 DB・一時 state dir・loopback の port で起こし、
# 終わったら止めて消す。LLM を呼ばない（llm source は fake、CoS は無効）。1000 件を作らない: 集計は件数が足りないと答えることまで確かめる。
# 1 段 1 行の JSON を --out に書く。どれかの段の確かめが外れたら exit 1。
set -euo pipefail

out=""
while [ $# -gt 0 ]; do
  case "$1" in
    --out) out="$2"; shift 2 ;;
    *) echo "usage: script-compare-rehearsal.sh --out <jsonl>" >&2; exit 2 ;;
  esac
done
[ -n "$out" ] || { echo "usage: script-compare-rehearsal.sh --out <jsonl>" >&2; exit 2; }
: "${CARGO_TARGET_DIR:?CARGO_TARGET_DIR が要る}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
tool="$root/scripts/dev/script-compare.py"
out="$(cd "$(dirname "$out")" && pwd)/$(basename "$out")"
: >"$out"

# worker の run の中の環境（run の DB・followups）を daemon と celerisctl に渡さない。
unset CELERIS_RUN_DB CELERIS_FOLLOWUPS_FILE CELERIS_RUN_ID CELERIS_TASK_ID CELERIS_COS_RUN_CREDENTIAL CELERIS_DB CELERIS_API_PRINCIPAL

(cd "$root" && cargo build -q -p celeris --bin celeris -p celerisctl --bin celerisctl -p celeris-script --bin script-deploy)
bin="$CARGO_TARGET_DIR/debug"
work="$(mktemp -d)"
daemon_pid=""
cleanup() {
  [ -z "$daemon_pid" ] || { kill -TERM "$daemon_pid" 2>/dev/null || true; wait "$daemon_pid" 2>/dev/null || true; }
  rm -rf "$work"
}
trap cleanup EXIT

# 候補 bundle は使い捨ての dir に作る（作業ツリーの control-ts/dist を使わない）。
dist="$work/dist"
(cd "$root/control-ts" && SC_BUILD="$root/control-ts/scripts/build-bundle.mjs" SC_OUT="$dist" node --input-type=module -e '
    const { buildBundle } = await import(process.env.SC_BUILD);
    await buildBundle({ outDir: process.env.SC_OUT });
  ' >/dev/null)

home="$work/daemon"
mkdir -p "$home/ws"
printf 'rehearsal-human-token\n' >"$home/human.token"
printf 'rehearsal-operator-token\n' >"$home/operator.token"
port="$(python3 -I -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
cat >"$home/config.toml" <<CONF
db = "celeris.sqlite3"
workspace_root = "ws"
tick_ms = 100

[llm_sources.local]
kind = "fake"
accounts = ["a1"]
models = ["fake"]

[harnesses.fake]
kind = "fake"
api_formats = ["fake"]
source_kinds = ["fake"]
command = "sh"
args = ["-c", "cat >/dev/null; echo '{\"type\":\"question\",\"text\":\"not needed\"}'"]

[route_defaults.fake]
candidates = [{ source = "local", model = "fake" }]

[harness_selection]
execution_class_defaults = {}
rules = [{ harness = "fake", required_capabilities = [] }]

[cos]
enabled = false

[workspace]
build_cache_dir = "$home/build-cache"

[scratch]
dir = "$home/scratch"

[api]
listen = "127.0.0.1:$port"

[[api.tokens]]
principal = "human:rehearsal"
token_file = "human.token"

[[api.tokens]]
principal = "operator:rehearsal"
token_file = "operator.token"
CONF
db="$home/celeris.sqlite3"
api="http://127.0.0.1:$port"
ctl() { CELERIS_API_PRINCIPAL=human:rehearsal "$bin/celerisctl" script --config "$home/config.toml" "$@"; }
now() { python3 -I -c 'import time; print(f"{time.time():.3f}")'; }
rec() { # rec <step> <exit> <t0> <detail json>
  python3 -I - "$out" "$1" "$2" "$3" "$4" <<'PY'
import json, sys, time
out, step, code, t0, detail = sys.argv[1:6]
with open(out, "a", encoding="utf-8") as f:
    f.write(json.dumps({"schema": "celeris.script-compare-rehearsal/1", "step": step, "exit": int(code),
                        "secs": round(time.time() - float(t0), 3), "detail": json.loads(detail)},
                       ensure_ascii=False, sort_keys=True) + "\n")
PY
}
fail() { echo "script-compare-rehearsal: $*" >&2; exit 1; }
field() { python3 -I -c 'import json,sys; v=json.load(sys.stdin)
for k in sys.argv[1].split("."): v=v[int(k)] if k.isdigit() else v[k]
print(v)' "$1"; }
start_daemon() {
  "$bin/celeris" --config "$home/config.toml" --log-format text --release rehearsal >>"$work/daemon.log" 2>&1 &
  daemon_pid=$!
  local i
  for i in $(seq 1 600); do
    kill -0 "$daemon_pid" 2>/dev/null || { tail -30 "$work/daemon.log" >&2; fail "daemon が終わった"; }
    curl -fs -o /dev/null -H "Authorization: Bearer rehearsal-human-token" "$api/api/v1/script/domains" && return 0
    sleep 0.2
  done
  fail "daemon の API が 120 秒で答えない"
}
stop_daemon() { kill -TERM "$daemon_pid"; wait "$daemon_pid" 2>/dev/null || true; daemon_pid=""; }
add_task() { # POST /tasks（人の依頼の代わり。fake harness が 1 run だけ走る）
  curl -fsS -X POST "$api/api/v1/tasks" -H "Authorization: Bearer rehearsal-human-token" \
    -H 'Content-Type: application/json' \
    -d "{\"title\":\"rehearsal $1\",\"objective\":\"rehearsal task $1\",\"acceptance\":[{\"type\":\"artifact_exists\",\"name\":\"r.md\"}]}" >/dev/null
}
domains_json() { ctl domains --json; }
version_of() { domains_json | python3 -I -c 'import json,sys
d=json.load(sys.stdin); rows=d["domains"] if isinstance(d,dict) else d
print([r for r in rows if r["domain"]==sys.argv[1]][0]["version"])' "$1"; }

# 1. config の [script] が無い daemon: 登録は 503（本番の今の状態の手前と同じ）。
t="$(now)"; start_daemon
code="$(curl -s -o /dev/null -w '%{http_code}' -X POST "$api/api/v1/script/generations" \
  -H "Authorization: Bearer rehearsal-human-token" -H 'Content-Type: application/json' \
  -d "{\"bundle_sha256\":\"$(printf 'a%.0s' $(seq 1 64))\"}")"
[ "$code" = 503 ] || fail "[script] 無しの登録が $code"
stop_daemon
rec no_script_section 0 "$t" "{\"register_http\": $code}"

# 2. config に [script] を足して再起動（人の手順の 3.）。mode は全部 rust。
t="$(now)"
printf '\n[script]\nstate_dir = "state"\n' >>"$home/config.toml"
start_daemon
modes="$(sqlite3 -readonly -batch "$db" "SELECT group_concat(domain||'='||mode) FROM script_domain_modes")"
case "$modes" in *compare*|*canary*|*=ts*) fail "起動直後の mode: $modes" ;; esac
rec start_with_script_section 0 "$t" "{\"modes\": \"$modes\"}"

# 3. 配備より先の登録は 409 で hash を失わない（手順の順序の確かめ）。
t="$(now)"
sha="$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["bundle_sha256"])' "$dist/manifest.json")"
set +e; early="$(ctl generations register "$sha" 2>&1)"; rc=$?; set -e
[ "$rc" -ne 0 ] && printf '%s' "$early" | grep -q script_bundle_not_deployed || fail "配備前の登録: rc=$rc $early"
rec register_before_deploy "$rc" "$t" '{"refused": "script_bundle_not_deployed"}'

# 4. 配備と登録。
t="$(now)"
deployed="$("$bin/script-deploy" --dist "$dist" --state-dir "$home/state")"
[ "$deployed" = "$sha" ] || fail "script-deploy の hash: $deployed"
ctl generations register "$sha" >/dev/null
state="$(ctl generations list --json | python3 -I -c 'import json,sys
d=json.load(sys.stdin); rows=d["generations"] if isinstance(d,dict) else d
print(rows[0]["state"])')"
[ "$state" = loaded ] || fail "登録後の state: $state"
rec deploy_and_register 0 "$t" "{\"bundle_sha256\": \"$sha\", \"state\": \"$state\"}"

# 5. 件数の基準を残してから lane_rules を compare にする。
t="$(now)"
base_id="$(sqlite3 -readonly -batch "$db" 'SELECT coalesce(max(id),0) FROM events')"
ctl mode lane_rules compare --expected-version "$(version_of lane_rules)" --generation "$sha" --mode-reason "rehearsal" >/dev/null
[ "$(sqlite3 -readonly -batch "$db" "SELECT mode FROM script_domain_modes WHERE domain='lane_rules'")" = compare ] || fail "compare になっていない"
for other in route_priority cos_digest; do
  [ "$(sqlite3 -readonly -batch "$db" "SELECT mode FROM script_domain_modes WHERE domain='$other'")" = rust ] || fail "$other が rust でない"
done
rec mode_compare 0 "$t" "{\"baseline_event_id\": $base_id}"

# 6. 本物の dispatch で記録を作る（件数を書き足さない）。
t="$(now)"
want=3
for n in $(seq 1 "$want"); do add_task "$n"; done
got=0
for _ in $(seq 1 300); do
  got="$(sqlite3 -readonly -batch "$db" "SELECT count(*) FROM events WHERE json_extract(json,'\$.type')='script_compare_recorded' AND json_extract(json,'\$.domain')='lane_rules'")"
  [ "$got" -ge "$want" ] && break
  sleep 0.2
done
[ "$got" -ge "$want" ] || { tail -30 "$work/daemon.log" >&2; fail "記録が $got 件"; }
rec dispatch_records 0 "$t" "{\"lane_rules_records\": $got}"

# 7. 標本（実 daemon の RSS と GET /daemon）と集計。件数が足りないので ready でなく exit 4。
t="$(now)"
python3 -I "$tool" sample --pid "$daemon_pid" --api "$api" --token-file "$home/human.token" >"$work/samples.jsonl"
set +e
python3 -I "$tool" report --db "$db" --samples "$work/samples.jsonl" --require-ready lane_rules >"$work/report.json"
rc=$?
set -e
[ "$rc" -eq 4 ] || { cat "$work/report.json" >&2; fail "report の exit: $rc"; }
python3 -I - "$work/report.json" "$out" "$rc" "$t" <<'PY'
import json, sys, time
r = json.load(open(sys.argv[1]))
d = [x for x in r["domains"] if x["domain"] == "lane_rules"][0]
assert d["outcomes"]["match"] >= 3 and d["outcomes"]["mismatch"] == 0, d
assert d["guard_state"]["calls"] == d["calls"] and not d["ready"], d
assert r["samples"]["rss_kb"]["max"] > 0, r["samples"]
assert not r["problems"], r["problems"]
detail = {"outcomes": d["outcomes"], "latency": d["latency"], "clean_streak": d["clean_streak"], "required": d["required"],
          "ready": d["ready"], "samples": r["samples"]}
with open(sys.argv[2], "a", encoding="utf-8") as f:
    f.write(json.dumps({"schema": "celeris.script-compare-rehearsal/1", "step": "report_not_ready", "exit": int(sys.argv[3]),
                        "secs": round(time.time() - float(sys.argv[4]), 3), "detail": detail},
                       ensure_ascii=False, sort_keys=True) + "\n")
PY

# 8. 人でない主体は canary に上げられない（昇格の権限の確かめ。operator の token）。
t="$(now)"
ver="$(version_of lane_rules)"
code="$(curl -s -o /dev/null -w '%{http_code}' -X PUT "$api/api/v1/script/domains/lane_rules/mode" \
  -H "Authorization: Bearer rehearsal-operator-token" -H 'Content-Type: application/json' \
  -d "{\"mode\":\"canary\",\"expected_version\":$ver,\"generation\":\"$sha\",\"canary_bps\":100,\"reason\":\"rehearsal\"}")"
[ "$code" = 403 ] || fail "operator の canary が $code"
# 人の token でも件数が足りなければ 422。
code2="$(curl -s -o /dev/null -w '%{http_code}' -X PUT "$api/api/v1/script/domains/lane_rules/mode" \
  -H "Authorization: Bearer rehearsal-human-token" -H 'Content-Type: application/json' \
  -d "{\"mode\":\"canary\",\"expected_version\":$ver,\"generation\":\"$sha\",\"canary_bps\":100,\"reason\":\"rehearsal\"}")"
[ "$code2" = 422 ] || fail "件数不足の canary が $code2"
rec canary_refused 0 "$t" "{\"operator_http\": $code, \"human_not_ready_http\": $code2}"

# 9. 戻し。rust に戻した後の task は記録を増やさない。
t="$(now)"
ctl mode lane_rules rust --expected-version "$(version_of lane_rules)" --mode-reason "rehearsal rollback" >/dev/null
[ "$(sqlite3 -readonly -batch "$db" "SELECT mode FROM script_domain_modes WHERE domain='lane_rules'")" = rust ] || fail "rust に戻っていない"
sleep 0.5
before="$(sqlite3 -readonly -batch "$db" "SELECT count(*) FROM events WHERE json_extract(json,'\$.type')='script_compare_recorded'")"
add_task after-rollback
sent=0
for _ in $(seq 1 150); do
  sent="$(sqlite3 -readonly -batch "$db" "SELECT count(*) FROM events WHERE json_extract(json,'\$.type')='routing_decided' AND json_extract(json,'\$.record.decision.source') IN ('hint','default')")"
  [ "$sent" -gt "$got" ] && break
  sleep 0.2
done
after="$(sqlite3 -readonly -batch "$db" "SELECT count(*) FROM events WHERE json_extract(json,'\$.type')='script_compare_recorded'")"
[ "$before" = "$after" ] || fail "rust に戻した後に記録が増えた: $before → $after"
rec rollback_to_rust 0 "$t" "{\"records_before\": $before, \"records_after\": $after, \"routing_decided_policy\": $sent}"

# 10. 再起動しても rust のまま。集計の戻しの欄に人の戻しが出る。
t="$(now)"
stop_daemon; start_daemon
mode="$(sqlite3 -readonly -batch "$db" "SELECT mode FROM script_domain_modes WHERE domain='lane_rules'")"
[ "$mode" = rust ] || fail "再起動後の mode: $mode"
python3 -I "$tool" report --db "$db" >"$work/report2.json"
actor="$(field rollback.to_rust_by_actor.human:rehearsal <"$work/report2.json")"
[ "$actor" = 1 ] || fail "集計の戻しの数: $actor"
stop_daemon
rec restart_stays_rust 0 "$t" "{\"mode\": \"$mode\", \"to_rust_by_human\": $actor}"
echo "script-compare-rehearsal: ok ($out)"
