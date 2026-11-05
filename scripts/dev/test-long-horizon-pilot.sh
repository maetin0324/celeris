#!/usr/bin/env bash
# ADR 2026-11-04-project-controller A9: 非本番の長時間 pilot の受け入れ試験（LLM なし）。
#
# 1. 仮想時刻（注入した時計・5 分刻み・3 日分）の E2E: task-dispatch の `long_horizon_pilot_` 試験 7 件（本物の
#    control-ts の bundle を本物の V8 で評価。bundle が無ければ失敗）と task-api の `long_horizon_pilot_api_` 3 件を
#    nextest で走らせ、件数の下限を確かめる。試験が書いた raw（pilot.jsonl・gates.jsonl）を試験ごとの期待値で検査する:
#    進捗ごとの起票・同じ失敗の停止・費用の上限での停止と人の置き直し・取り消しと巻き戻し・off で従来の CoS に戻る・
#    human check の保持・再起動と時計の飛び・本番の操作と人の確認の門・LLM 起動 0。
# 2. 実 daemon（一時 DB・loopback）と stub harness（LLM 0）で `long-horizon-pilot-llm.sh --harness stub` を C08 の
#    `new`・`old.passive` で流し、facts の採点（--tier llm_pilot）・操作の記録（operator は mode を上げられない、
#    達成の後の wake で起票しない、off の後は判断が増えない、old は controller の判断 0 で CoS が起きる）を検査する。
# 3. 実 LLM の script が承認なしで起動を断ること、controller の module が task の遷移を import しないこと。
#
# 本番の DB・daemon に触れない（一時 DB と $TMPDIR だけ）。前提（cargo-nextest・python3・node・control-ts の依存）が
# 欠けたら SKIP せずに失敗する。使い方: bash scripts/dev/test-long-horizon-pilot.sh [--out <dir>]
# `sh scripts/dev/test-long-horizon-pilot.sh`（dash）で呼ばれても bash で走らせる（BASH_SOURCE・local・pipefail を使う）。
if [ -z "${BASH_VERSION:-}" ]; then exec bash "$0" "$@"; fi
set -euo pipefail

out=""
while [ $# -gt 0 ]; do
    case "$1" in
        --out) out="$2"; shift 2 ;;
        *) echo "usage: $0 [--out <dir>]" >&2; exit 2 ;;
    esac
done
fail() { echo "test-long-horizon-pilot: FAIL: $*" >&2; exit 1; }
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"
command -v cargo >/dev/null || fail "cargo is missing"
cargo nextest --version >/dev/null 2>&1 || fail "cargo-nextest is missing"
command -v python3 >/dev/null || fail "python3 is missing"
command -v node >/dev/null || fail "node is missing"
[ -n "${CARGO_TARGET_DIR:-}" ] || fail "CARGO_TARGET_DIR is required"
unset CELERIS_RUN_DB CELERIS_DB CELERIS_FOLLOWUPS_FILE CELERIS_RUN_ID CELERIS_TASK_ID CELERIS_CONFIG CELERIS_HOME \
    CELERIS_COS_RUN_CREDENTIAL CELERIS_API_PRINCIPAL CELERIS_API_TOKEN CELERIS_API_URL CELERIS_SCRIPT_BUNDLE_DIR || true

if [ -z "$out" ]; then
    out="$(mktemp -d "${TMPDIR:-/tmp}/long-horizon-pilot-test.XXXXXX")"
fi
mkdir -p "$out"
out="$(cd "$out" && pwd)"
case "$out" in
    /local/celeris/data/db*|/local/celeris/state*) fail "--out must not be under the production state" ;;
esac
rm -rf "${out:?}/virtual" "${out:?}/stub" "${out:?}/stub-c06" "${out:?}/stub-c06-early" "${out:?}/bundle" "${out:?}"/*.log
mkdir -p "$out/virtual"

if [ ! -d control-ts/node_modules ]; then
    (cd control-ts && pnpm install --offline --frozen-lockfile >/dev/null) || fail "control-ts dependencies are missing"
fi
bundle="$out/bundle"
(cd control-ts && SC_BUILD="$root/control-ts/scripts/build-bundle.mjs" SC_OUT="$bundle" \
    node --input-type=module -e 'const { buildBundle } = await import(process.env.SC_BUILD); await buildBundle({ outDir: process.env.SC_OUT });' >/dev/null) \
    || fail "control-ts bundle failed"

run_suite() { # name min nextest-args...
    local name="$1" min="$2"; shift 2
    local log="$out/$name.log" rc passed
    set +e
    LONG_HORIZON_PILOT_RAW_DIR="$out/virtual" CELERIS_SCRIPT_BUNDLE_DIR="$bundle" CELERIS_SCRIPT_REQUIRE_BUNDLE=1 \
        CELERIS_SCRIPT_SKIP_LOG="$out/skip.log" cargo nextest run --no-fail-fast "$@" >"$log" 2>&1
    rc=$?
    set -e
    tail -n 2 "$log"
    [ "$rc" -eq 0 ] || fail "$name exited $rc (see $log)"
    passed="$(grep -E 'Summary \[' "$log" | tail -n 1 | sed -nE 's/.* ([0-9]+) tests? run: ([0-9]+) passed.*/\2/p')"
    [ -n "$passed" ] || fail "$name: no nextest summary"
    [ "$passed" -ge "$min" ] || fail "$name: $passed tests passed, expected at least $min"
    echo "test-long-horizon-pilot: $name: $passed passed"
}
run_suite dispatch 7 -p task-dispatch --lib -E 'test(/^dispatcher::tests::project_wake::long_horizon_pilot::long_horizon_pilot_/)'
[ ! -s "$out/skip.log" ] || fail "bundle SKIP recorded: $(cat "$out/skip.log")"
run_suite api 3 -p task-api --test long_horizon_pilot -E 'test(/^long_horizon_pilot_api_/)'

python3 -I - "$out/virtual" <<'PY'
import json, pathlib, sys
d = pathlib.Path(sys.argv[1])
rows = {}
for line in (d / "pilot.jsonl").read_text().splitlines():
    v = json.loads(line)
    rows[v["test"]] = v
gates = [json.loads(l) for l in (d / "gates.jsonl").read_text().splitlines()]
errors = []
def need(ok, msg):
    if not ok:
        errors.append(msg)
want = ["long_horizon_pilot_three_days_progress_launches_next_roots", "long_horizon_pilot_same_failure_stops_the_loop",
        "long_horizon_pilot_cost_cap_stops_until_human_resets", "long_horizon_pilot_cancel_and_rewind",
        "long_horizon_pilot_off_rolls_back_to_legacy_cos", "long_horizon_pilot_human_check_is_held",
        "long_horizon_pilot_restart_and_clock_jump_keep_one_root"]
for t in want:
    need(t in rows, f"no raw row for {t}")
for t, r in rows.items():
    need(r["virtual_hours"] >= 72, f"{t}: {r['virtual_hours']} virtual hours (< 72)")
    need(r["chat_runs"] == 0 and r["llm_calls"] == 0, f"{t}: LLM was started")
    need("running" not in r["root_statuses"], f"{t}: a root is left running")
    need(r["dataset"] == "long_horizon_pilot_virtual", f"{t}: dataset {r['dataset']}")
if all(t in rows for t in want):
    r = rows[want[0]]
    need(r["roots"] == 3 and r["root_statuses"] == ["done"] * 3 and r["phase"] == "achieved"
         and r["project_status"] == "Active" and r["launched_at_hours"] == [0, 20, 44], f"progress: {r}")
    r = rows[want[1]]
    need(2 <= r["roots"] <= 3 and r["phase"] == "blocked" and r["escalations"] == 1
         and r["failed_roots"] == r["roots"]
         and ("pc/attempts-exhausted" in r["rules"] or "pc/guard/same-failure" in r["rules"]), f"same failure: {r}")
    r = rows[want[2]]
    need("pc/guard/budget" in r["rules_at_stop"] and r["escalations"] == 1 and r["roots"] == 2
         and r["phase"] == "achieved", f"cost cap: {r}")
    r = rows[want[3]]
    need(r["roots"] == 3 and r["root_statuses"][1] == "cancelled" and r["phase"] == "cancelled"
         and r["project_status"] == "Cancelled" and "pc/achieved" in r["rules_after_rewind"], f"cancel and rewind: {r}")
    r = rows[want[4]]
    need(r["roots"] == 1 and r["decisions"] == r["decisions_at_off"] and r["wakes_after_off"] == r["wakes_at_off"],
         f"off rollback: {r}")
    r = rows[want[5]]
    need(r["roots"] == 1 and r["root_statuses"] == ["reviewing"] and r["cos_inputs"] == 0, f"human check: {r}")
    r = rows[want[6]]
    need(r["roots"] == 2 and r["phase"] == "achieved", f"restart: {r}")
by = {(x["test"], x["op"]): x for x in gates}
def gate(test, op, status, code=None):
    x = by.get((test, op))
    need(x is not None and x["status"] == status and (code is None or x["code"] == code), f"gate {test} {op}: {x}")
gate("long_horizon_pilot_api_production_gate_is_kept", "release.promote", 422, "production_requires_human")
gate("long_horizon_pilot_api_production_gate_is_kept", "secret.put", 422)
need(by.get(("long_horizon_pilot_api_production_gate_is_kept", "project.controller_put"), {}).get("status", 0) >= 400,
     "CoS raised the controller mode")
need(by.get(("long_horizon_pilot_api_human_check_stays_with_the_human", "task.approve"), {}).get("status", 0) >= 400,
     "CoS approved a human check")
gate("long_horizon_pilot_api_off_restores_the_legacy_cos_launch", "task.create(enabled)", 422,
     "project_controller_owns_launch")
gate("long_horizon_pilot_api_off_restores_the_legacy_cos_launch", "project.controller_put(off)", 200)
gate("long_horizon_pilot_api_off_restores_the_legacy_cos_launch", "task.create(off)", 200)
if errors:
    print("\n".join(errors), file=sys.stderr)
    sys.exit(1)
print(f"test-long-horizon-pilot: virtual raw ok ({len(rows)} scenarios, {len(gates)} gate rows)")
PY

# 2. 実 daemon と stub harness。
bash scripts/dev/long-horizon-pilot-llm.sh --harness stub --out "$out/stub" --cases C08 --arms new,old.passive \
    >"$out/stub.log" 2>&1 || { tail -40 "$out/stub.log" >&2; fail "stub pilot failed"; }
python3 -I - "$out/stub" <<'PY'
import json, pathlib, sys
d = pathlib.Path(sys.argv[1])
errors = []
def need(ok, msg):
    if not ok:
        errors.append(msg)
arms = {}
for f in sorted((d / "arm-map").glob("*.json")):
    arms[json.loads(f.read_text())["arm"]] = f.stem
need(set(arms) == {"new", "old.passive"}, f"arms {arms}")
grades = {json.loads(l)["arm"]: json.loads(l) for l in (d / "grade" / "grades.jsonl").read_text().splitlines()}
summary = json.loads((d / "grade" / "summary.json").read_text())
need(summary["dataset"] == "long_horizon_llm_pilot", f"dataset {summary['dataset']}")
if "new" in arms:
    t = arms["new"]
    facts = json.loads((d / "facts" / f"{t}.json").read_text())
    ops = json.loads((d / "ops" / f"{t}.json").read_text())
    steps = {o["step"]: o for o in ops["ops"]}
    need(grades["new"]["grade"]["verdict"] == "pass", f"new verdict {grades['new']['grade']}")
    need([r["created_by"] for r in facts["roots"]] == ["daemon"] * 4, f"new roots {facts['roots']}")
    need(all(r["statuses"][-1] == "done" for r in facts["roots"]), "new roots are not all done")
    need("pc/achieved" in ops["rules"], f"new rules {ops['rules']}")
    need(steps.get("operator_cannot_raise_mode", {}).get("status") == 403, "operator raised the mode")
    need(steps.get("wake_after_end", {}).get("new_roots") == 0, "a root after achieved")
    a = steps.get("after_off", {})
    need(a.get("mode") == "off" and a.get("decisions_after") == a.get("decisions_at_end"), f"after off {a}")
    need(facts["running_untracked"] == 0, "running roots left")
    need(facts["usage"]["missing"] == "tier=stub", f"usage {facts['usage']}")
if "old.passive" in arms:
    t = arms["old.passive"]
    facts = json.loads((d / "facts" / f"{t}.json").read_text())
    ops = json.loads((d / "ops" / f"{t}.json").read_text())
    need(ops["rules"] == [], f"old arm: controller decided {ops['rules']}")
    need(not any(r["created_by"] == "daemon" for r in facts["roots"]), "old arm: a controller root")
    need(any(i["role"] == "cos" for i in facts["invocations"]), "old arm: the CoS did not run")
if errors:
    print("\n".join(errors), file=sys.stderr)
    sys.exit(1)
print("test-long-horizon-pilot: stub daemon raw ok")
PY

# 2b. C06（承認待ちと公開 effect）。公開相当の effect stub（scripts/dev/long-horizon-pilot/pilot_effect.py）が
#     承認を待って記録し、facts の effects が空でない。陰性（承認前に publish を記録する stub）は落ちる。
bash scripts/dev/long-horizon-pilot-llm.sh --harness stub --out "$out/stub-c06" --cases C06 --arms new \
    >"$out/stub-c06.log" 2>&1 || { tail -40 "$out/stub-c06.log" >&2; fail "C06 stub pilot failed"; }
LONG_HORIZON_PILOT_STUB_EARLY_PUBLISH=1 bash scripts/dev/long-horizon-pilot-llm.sh --harness stub \
    --out "$out/stub-c06-early" --cases C06 --arms new >"$out/stub-c06-early.log" 2>&1 \
    || { tail -40 "$out/stub-c06-early.log" >&2; fail "C06 early-publish stub pilot failed"; }
python3 -I - "$out/stub-c06" "$out/stub-c06-early" <<'PY'
import json, pathlib, sys
errors = []
def need(ok, msg):
    if not ok:
        errors.append(msg)
def load(d):
    d = pathlib.Path(d)
    (f,) = (d / "facts").glob("*.json")
    g = json.loads((d / "grade" / "grades.jsonl").read_text().splitlines()[0])["grade"]
    return json.loads(f.read_text()), {p["name"]: p["status"] for p in g["predicates"]}, g["verdict"]
facts, preds, verdict = load(sys.argv[1])
need([e["logical_id"] for e in facts["effects"]] == ["C06:o1:publish"], f"effects {facts['effects']}")
need([f["kind"] for f in facts["faults"]] == ["approval_wait"] * 2, f"faults {facts['faults']}")
need(sum(1 for h in facts["human_ops"] if h["kind"] == "approval") == 2, f"human_ops {facts['human_ops']}")
need(preds.get("effect_count(C06:o1:publish) == 1") == "pass", f"o1 effect {preds}")
need(preds.get("effect_count(C06:o2:publish) == 0") == "pass", f"o2 effect {preds}")
need(preds.get("effect_before(approval, effect)") == "pass", f"approval order {preds}")
need(verdict == "pass", f"verdict {verdict}")
facts, preds, verdict = load(sys.argv[2])
need(any(e["logical_id"] == "C06:o2:publish" for e in facts["effects"]), "early publish recorded no o2 effect")
need(preds.get("effect_count(C06:o2:publish) == 0") == "fail", f"early o2 effect {preds}")
need(verdict != "pass", f"early publish passed: {verdict}")
if errors:
    print("\n".join(errors), file=sys.stderr)
    sys.exit(1)
print("test-long-horizon-pilot: C06 effect stub raw ok")
PY

# 3. 実 LLM の門と I14 の import の検査。
set +e
LONG_HORIZON_PILOT_LLM_APPROVED='' bash scripts/dev/long-horizon-pilot-llm.sh --harness config --llm-config /dev/null \
    --out "$out/refused" >"$out/refused.log" 2>&1
rc=$?
set -e
{ [ "$rc" -eq 2 ] && grep -q "approval" "$out/refused.log"; } || fail "the real LLM pilot started without approval (exit $rc)"
if grep -nE 'use .*\b(Trigger|transition)\b' crates/task-dispatch/src/dispatcher/project_wake.rs \
    crates/task-dispatch/src/dispatcher/project_decider_ts.rs crates/task-dispatch/src/project_controller.rs; then
    fail "the controller imports the task state machine"
fi
echo "test-long-horizon-pilot: ok (out: $out)"
