#!/usr/bin/env bash
# ADR 2026-11-04-project-controller A6: 長時間の運用の受け入れ試験（fault injection・仮想時刻の replay・資源の raw）。
#
# 1. task-dispatch の `project_controller_replay_` 試験（注入した時計・fake decider。crash の 7 境界・3 日の時計の
#    飛び・外部の応答の欠落〈TTL〉・同じ失敗 5 回・人への問いの繰り返し・1 日の予算・token の予算・paused・
#    cancelled・achieved・event が主で cron の照合が保険・TS の案の検証落ちの連続・dispatcher の loop cost）を
#    nextest で走らせ、13 件以上が通ることを確かめる（0 件・不足は失敗）。
# 2. celeris の `project_controller_replay_` 試験（本物の control-ts の bundle を本物の V8 で 1 個、100 案件の判定の
#    RSS・isolate・thread・遅延・loop cost）を `CELERIS_SCRIPT_REQUIRE_BUNDLE=1` で走らせる（bundle が無いと失敗）。
#    task-core・task-ops の `project_budget_` の単体試験も走らせる。
# 3. 試験が書いた raw（`replay.jsonl`・`resource.jsonl`、celeris.long-horizon-record/1）を検査する:
#    - 試験ごとの期待値（LLM 起動〈CoS 入力・chat run〉の数・人への問いの数・評価の数）と一致する
#    - resource: dispatcher の idle と 100 案件、TS の idle と 100 案件の 4 行が揃い、isolate は 0 か 1、
#      100 案件で thread が増えていない
#
# LLM を呼ばない（fake decider・注入した時計・本物の TS は決定的な規則）。本番の DB・daemon に触れない（試験は一時 DB
# だけを開く）。前提（cargo-nextest・python3・node・control-ts の依存）が欠けたら SKIP せずに失敗する。
#
# 使い方: bash scripts/dev/test-project-controller-replay.sh [--out <dir>]（既定は $TMPDIR の下の新しい dir）
set -euo pipefail

out=""
while [ $# -gt 0 ]; do
    case "$1" in
        --out) out="$2"; shift 2 ;;
        *) echo "usage: $0 [--out <dir>]" >&2; exit 2 ;;
    esac
done

fail() { echo "test-project-controller-replay: FAIL: $*" >&2; exit 1; }

command -v cargo >/dev/null || fail "cargo is missing"
cargo nextest --version >/dev/null 2>&1 || fail "cargo-nextest is missing"
command -v python3 >/dev/null || fail "python3 is missing"
command -v node >/dev/null || fail "node is missing"

# 本番の環境変数を引き継がない（ADR D11.3）。
unset CELERIS_RUN_DB CELERIS_DB CELERIS_FOLLOWUPS_FILE CELERIS_RUN_ID CELERIS_TASK_ID CELERIS_CONFIG CELERIS_HOME \
    CELERIS_COS_RUN_CREDENTIAL CELERIS_API_PRINCIPAL CELERIS_API_TOKEN CELERIS_API_URL CELERIS_SCRIPT_BUNDLE_DIR || true

if [ -z "$out" ]; then
    out="$(mktemp -d "${TMPDIR:-/tmp}/project-controller-replay.XXXXXX")"
fi
mkdir -p "$out"
# 試験は crate の dir で走るので、相対の --out を絶対 path にしてから渡す。
out="$(cd "$out" && pwd)"
case "$out" in
    /local/celeris/data/db*|"${CELERIS_STATE_DIR:-/nonexistent-state-dir}"*) fail "--out must not be under the production state" ;;
esac
rm -f "$out"/replay.jsonl "$out"/resource.jsonl "$out"/skip.log "$out"/*.log
export PROJECT_CONTROLLER_REPLAY_RAW_DIR="$out"

if [ ! -d control-ts/node_modules ]; then
    (cd control-ts && pnpm install --offline --frozen-lockfile) \
        || fail "control-ts dependencies are missing (pnpm install --offline failed)"
fi

run_suite() {
    # $1 log name, $2 minimum count, rest: nextest args
    local name="$1" min="$2"; shift 2
    local log="$out/$name.log"
    set +e
    CELERIS_SCRIPT_REQUIRE_BUNDLE=1 CELERIS_SCRIPT_SKIP_LOG="$out/skip.log" \
        cargo nextest run --no-fail-fast "$@" >"$log" 2>&1
    local rc=$?
    set -e
    tail -n 3 "$log"
    [ "$rc" -eq 0 ] || fail "$name exited $rc (see $log)"
    local summary passed
    summary="$(grep -E 'Summary \[' "$log" | tail -n 1)"
    passed="$(printf '%s\n' "$summary" | sed -nE 's/.* ([0-9]+) tests? run: ([0-9]+) passed.*/\2/p')"
    [ -n "$passed" ] || fail "$name: no nextest summary"
    printf '%s\n' "$summary" | grep -qE 'failed' && fail "$name: failures in $summary"
    [ "$passed" -ge "$min" ] || fail "$name: $passed tests passed, expected at least $min"
    echo "test-project-controller-replay: $name: $passed passed"
}

run_suite dispatch 13 -p task-dispatch --lib -E 'test(/^dispatcher::tests::project_wake::replay::project_controller_replay_/)'
run_suite celeris 1 -p celeris --test project_controller_replay -E 'test(/^project_controller_replay_/)'
run_suite budget 2 -p task-ops --lib -E 'test(/project_budget_/)'
[ ! -s "$out/skip.log" ] || fail "bundle SKIP recorded: $(cat "$out/skip.log")"

python3 -I - "$out" <<'PY'
import json, sys, pathlib
out = pathlib.Path(sys.argv[1])
errors = []

def load(name):
    p = out / name
    if not p.exists():
        errors.append(f"{name} is missing")
        return []
    rows = []
    for n, line in enumerate(p.read_text().splitlines(), 1):
        try:
            v = json.loads(line)
        except ValueError as e:
            errors.append(f"{name}:{n}: {e}")
            continue
        if v.get("schema") != "celeris.long-horizon-record/1":
            errors.append(f"{name}:{n}: schema {v.get('schema')}")
        rows.append(v)
    return rows

replay = load("replay.jsonl")
by = {}
for r in replay:
    by.setdefault(r["test"], []).append(r)

def one(test):
    rows = by.get(test, [])
    if not rows:
        errors.append(f"replay: {test} has no record")
        return {}
    return rows[-1]

def expect(test, row, key, want):
    got = row.get(key)
    if got != want:
        errors.append(f"replay {test}: {key} = {got!r}, expected {want!r}")

# 全試験: LLM 起動（CoS 入力・chat run）が期待値どおり。
for r in replay:
    if r.get("cos_inputs") != r.get("expected_cos_inputs"):
        errors.append(f"replay {r['test']}: cos_inputs {r.get('cos_inputs')} != expected {r.get('expected_cos_inputs')}")
    if r.get("chat_runs") != 0:
        errors.append(f"replay {r['test']}: chat_runs {r.get('chat_runs')} (expected 0)")

crash = by.get("project_controller_replay_crash_each_stage", [])
stages = sorted(r.get("boundary") for r in crash)
want = sorted(["BeforeIngest", "AfterIngest", "AfterLease", "BeforeCommit", "AfterCommit", "AfterEffect", "AfterApplied"])
if stages != want:
    errors.append(f"crash stages {stages} != {want}")
for r in crash:
    for k, v in (("escalations", 1), ("roots", 0), ("decides", 0), ("phase", "blocked")):
        expect(f"crash {r.get('boundary')}", r, k, v)

t = "project_controller_replay_suspend_clock_jump_3days"; r = one(t)
expect(t, r, "decides_after_jump", 1)
if r and r.get("wakes_folded", 0) < 6:
    errors.append(f"{t}: wakes_folded {r.get('wakes_folded')} < 6")
t = "project_controller_replay_external_never_replies"; r = one(t)
expect(t, r, "escalations", 1); expect(t, r, "decides_after_stop", 0); expect(t, r, "phase", "blocked")
t = "project_controller_replay_worker_fails_same_way_5x"; r = one(t)
expect(t, r, "failures_seen", 3); expect(t, r, "roots", 3); expect(t, r, "escalations", 1)
t = "project_controller_replay_human_escalation_repeat"; r = one(t)
expect(t, r, "escalations", 1); expect(t, r, "decides", 2)
t = "project_controller_replay_budget_day_cap"; r = one(t)
expect(t, r, "decides", 2); expect(t, r, "escalations", 1)
t = "project_controller_replay_usd_cap"; r = one(t)
expect(t, r, "decides", 1); expect(t, r, "escalations", 1)
t = "project_controller_replay_paused_project"; r = one(t)
expect(t, r, "decides_while_paused", 0); expect(t, r, "decides_after_resume", 1)
t = "project_controller_replay_cancelled_project"; r = one(t)
expect(t, r, "decides", 0)
t = "project_controller_replay_achieved_stays_quiet"; r = one(t)
expect(t, r, "decides_while_achieved", 1); expect(t, r, "roots_while_achieved", 0); expect(t, r, "decides_after_human", 2)
t = "project_controller_replay_event_primary_cron_fallback"; r = one(t)
if r and not (r.get("event_path_latency_secs", 10**9) <= 60 < r.get("cron_recovery_latency_secs", 0) <= 3700):
    errors.append(f"{t}: latencies event {r.get('event_path_latency_secs')} cron {r.get('cron_recovery_latency_secs')}")
t = "project_controller_replay_invalid_proposals_stop"; r = one(t)
expect(t, r, "decides", 3); expect(t, r, "escalations", 1)

resource = load("resource.jsonl")
seen = {(r.get("kind"), r.get("scenario")) for r in resource}
for need in [("dispatcher_loop", "idle"), ("dispatcher_loop", "projects_100"), ("ts_decider", "idle"), ("ts_decider", "projects_100")]:
    if need not in seen:
        errors.append(f"resource: {need} is missing")
for r in resource:
    for k in ("rss_kb", "isolates", "llm_launches"):
        if k not in r:
            errors.append(f"resource {r.get('kind')}/{r.get('scenario')}: {k} missing")
    if r.get("llm_launches") != 0:
        errors.append(f"resource {r.get('kind')}/{r.get('scenario')}: llm_launches {r.get('llm_launches')}")
    if r.get("isolates") not in (0, 1):
        errors.append(f"resource {r.get('kind')}/{r.get('scenario')}: isolates {r.get('isolates')}")
    if r.get("scenario") == "projects_100":
        keys = ("loop_micros_p50", "decide_micros_p95") if r.get("kind") == "dispatcher_loop" else (
            "decide_micros_p50", "decide_micros_p95", "native_snapshot_micros_p95", "loop_micros_per_100_projects_p50")
        for k in keys:
            if not isinstance(r.get(k), int) or r.get(k) <= 0:
                errors.append(f"resource {r.get('kind')}: {k} = {r.get(k)!r}")
idle = {r.get("kind"): r for r in resource if r.get("scenario") == "idle"}
busy = {r.get("kind"): r for r in resource if r.get("scenario") == "projects_100"}
if "ts_decider" in idle and "ts_decider" in busy and busy["ts_decider"].get("threads", 10**6) > idle["ts_decider"].get("threads", 0) + 1:
    errors.append("resource ts_decider: threads grew with 100 projects")

if errors:
    for e in errors:
        print("test-project-controller-replay: FAIL:", e, file=sys.stderr)
    sys.exit(1)
print(f"test-project-controller-replay: raw ok ({len(replay)} replay rows, {len(resource)} resource rows) in {out}")
PY
echo "test-project-controller-replay: OK"
