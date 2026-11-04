#!/usr/bin/env bash
# A2（ADR 2026-11-04-project-controller）: Project Controller の TS core の受け入れの検査。
#   bash scripts/dev/test-project-controller-e2e.sh [--out <dir>]
# 1. crates/celeris/tests/project_controller_e2e.rs の `project_controller_core_` を nextest で走らせる（本物の bundle を
#    本物の V8 で。CELERIS_SCRIPT_REQUIRE_BUNDLE=1 なので bundle が作れなければ SKIP でなく失敗）。11 件未満・SKIP 1 件でも失敗。
# 2. 試験が書いた raw（celeris.long-horizon-record/1）を読んで条件を検査する: root1 完了→中間目標 o1 の到達→root2 の
#    起票（root1 は done のまま）→最終目標の到達で追加の起票 0、同時 8 評価・8 適用で root 1 件、LLM 起動 0 など。
# 3. control-ts の領域の単体試験と fixture の不変条件（compare.mjs）。
# 4. TS だけの 1 行の変更で ts-verify.sh --record を通し（edit-activate-baseline.sh --scenario pc-ts）、
#    cargo_invocations = 0・link_count = 0 を確かめる（試行数は PC_TS_REPS、既定 1）。
# LLM を呼ばない。本番の DB・daemon・release 置き場に触れない（一時 DB と $TMPDIR だけ）。前提（node・cargo-nextest・
# control-ts の依存）が欠けたら exit 1。CPU を焼く負荷はかけない。
set -euo pipefail

out=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --out) [ "$#" -ge 2 ] || { echo "usage: $0 [--out <dir>]" >&2; exit 2; }; out="$2"; shift ;;
    *) echo "usage: $0 [--out <dir>]" >&2; exit 2 ;;
  esac
  shift
done
root="$(git rev-parse --show-toplevel)"
cd "$root"
# 本番・測定の config を読ませない（ADR D11）。
unset CELERIS_CONFIG CELERIS_HOME CELERIS_DB CELERIS_API_TOKEN CELERIS_API_URL CELERIS_SCRIPT_BUNDLE_DIR || true
[ -n "$out" ] || out="$(mktemp -d "${TMPDIR:-/tmp}/pc-e2e.XXXXXX")"
mkdir -p "$out"
case "$out" in /*) ;; *) out="$PWD/$out" ;; esac
rm -f "$out"/project_controller_core_*.jsonl "$out/skip.log"

fail() { echo "test-project-controller-e2e: FAIL: $*" >&2; exit 1; }
command -v node >/dev/null || fail "node is required"
cargo nextest --version >/dev/null 2>&1 || fail "cargo-nextest is required"
if [ ! -d control-ts/node_modules ]; then
  (cd control-ts && pnpm install --offline --frozen-lockfile) || fail "control-ts dependencies are missing (pnpm install --offline failed)"
fi

echo "== 1. nextest project_controller_core_"
log="$out/nextest.log"
set +e
CELERIS_SCRIPT_REQUIRE_BUNDLE=1 CELERIS_SCRIPT_SKIP_LOG="$out/skip.log" CELERIS_PC_E2E_OUT="$out" \
  cargo nextest run -p celeris --test project_controller_e2e -E 'test(/^project_controller_core_/)' --no-fail-fast 2>&1 | tee "$log"
rc=${PIPESTATUS[0]}
set -e
[ "$rc" -eq 0 ] || fail "nextest exited $rc"
summary="$(grep -E 'Summary .* tests? run' "$log" | tail -1)"
passed="$(printf '%s\n' "$summary" | sed -nE 's/.* ([0-9]+) passed.*/\1/p')"
[ -n "$passed" ] || fail "no nextest summary"
[ "$passed" -ge 11 ] || fail "only $passed project_controller_core_ tests passed (need >= 11)"
if printf '%s\n' "$summary" | grep -qE '[1-9][0-9]* skipped'; then fail "skipped tests: $summary"; fi
[ ! -s "$out/skip.log" ] || fail "bundle SKIP recorded: $(cat "$out/skip.log")"

echo "== 2. raw records"
python3 -I - "$out" <<'PY'
import glob, json, os, sys
out = sys.argv[1]
recs = {}
for f in sorted(glob.glob(os.path.join(out, "project_controller_core_*.jsonl"))):
    rows = [json.loads(l) for l in open(f) if l.strip()]
    recs[os.path.basename(f)[:-6]] = rows
problems = []
def need(cond, msg):
    if not cond:
        problems.append(msg)
need(len(recs) >= 11, f"raw files: {len(recs)} (need >= 11)")
for name, rows in recs.items():
    need(rows, f"{name}: empty")
    for r in rows:
        need(r.get("schema") == "celeris.long-horizon-record/1", f"{name}: schema")
        need(r.get("llm_launches") == 0, f"{name}: llm launched")
# root1 → 中間目標 → root2 → 最終目標で追加の起票 0
flow = recs.get("project_controller_core_s2_s3_root1_to_root2_to_achieved", [])
acts = [(r["action"], r["applied"]) for r in flow]
launches = [a for a in acts if "launched" in a[1]]
need(len(launches) == 2 and all(a[1]["created"] for a in launches), f"flow launches: {launches}")
order = [r["action"] for r in flow]
try:
    i_mark1 = next(i for i, r in enumerate(flow) if r["action"] == "mark_milestone" and r["gap"]["reached"] == ["o1"])
    i_root2 = [i for i, r in enumerate(flow) if "launched" in r["applied"]][1]
    need(i_mark1 < i_root2, "root2 must follow the o1 milestone")
    need(flow[i_mark1]["gap"]["open"] == ["o2"], "o2 must be open at the o1 milestone (intermediate reached, final not)")
    root1 = flow[0]["applied"]["launched"]
    after = flow[i_root2]["roots"]
    need({"id": root1, "status": "done"} in after, f"root1 must stay done when root2 is created: {after}")
except (StopIteration, IndexError):
    problems.append(f"flow order: {order}")
achieved = [r for r in flow if r["action"] == "wait" and r["rule_id"] == "pc/achieved"]
need(len(achieved) >= 3, f"achieved evaluations: {len(achieved)}")
if achieved:
    i0 = flow.index(achieved[0])
    need(all("launched" not in r["applied"] for r in flow[i0:]), "a root was launched after the final goal")
    need(all(r["roots_total"] == 2 for r in flow[i0:]), "roots_total changed after the final goal")
need(flow and flow[-1]["cos_inputs"] == 0, "flow used a CoS input")
# 同時イベントで重複 0
conc = recs.get("project_controller_core_s4_concurrent_events_create_one_root", [])
need(conc and conc[-1]["extra"].get("concurrent_applies") == 8 and conc[-1]["extra"].get("created") == 1
     and conc[-1]["extra"].get("distinct_roots") == 1 and conc[-1]["roots_total"] == 1, f"concurrency: {conc[-1:] }")
# 人の条件を狭めた出力は起票しない
nar = recs.get("project_controller_core_s10_narrowed_acceptance_is_rejected", [])
need(nar and nar[0]["applied"] == {"rejected": ["root_acceptance_narrowed"]} and all(r["roots_total"] == 0 for r in nar),
     f"narrowed: {nar[:1]}")
rf = recs.get("project_controller_core_s6_review_failed_replans_once", [])
need(rf and rf[-1]["cos_inputs"] == 1 and rf[-1]["roots_total"] == 1, "review_failed must replan once and not launch")
for name in ("project_controller_core_s7_infra_failure_waits", "project_controller_core_s9_open_human_question_waits",
             "project_controller_core_s5_not_integrated_continues"):
    rows = recs.get(name, [])
    need(rows and rows[-1]["roots_total"] == 1, f"{name}: extra root")
summary = {"files": len(recs), "rows": sum(len(v) for v in recs.values()), "problems": problems}
print(json.dumps(summary, ensure_ascii=False))
sys.exit(1 if problems else 0)
PY

echo "== 3. control-ts unit and invariants"
(cd control-ts && ./node_modules/.bin/vitest run src/domains/project_controller.test.ts src/domains/domains.test.ts) || fail "vitest"
(cd control-ts && node scripts/compare.mjs project_controller) || fail "compare.mjs project_controller"

echo "== 4. TS-only change: ts-verify --record (cargo 0)"
reps="${PC_TS_REPS:-1}"
bash scripts/dev/edit-activate-baseline.sh --scenario pc-ts --mode warm --reps "$reps" --out "$out/pc-ts.jsonl" --series a2-check \
  || fail "edit-activate-baseline pc-ts"
python3 -I - "$out/pc-ts.jsonl" "$reps" <<'PY'
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
trials = [r for r in rows if r.get("kind") == "trial"]
want = int(sys.argv[2])
bad = [r for r in trials if r.get("cargo_invocations") != 0 or r.get("link_count") != 0 or r.get("exit") not in (0, None)]
print(json.dumps({"trials": len(trials), "secs": [r.get("secs") for r in trials],
                  "cargo_invocations": [r.get("cargo_invocations") for r in trials],
                  "link_count": [r.get("link_count") for r in trials]}))
sys.exit(0 if len(trials) == want and not bad else 1)
PY
echo "test-project-controller-e2e: OK (raw in $out)"
