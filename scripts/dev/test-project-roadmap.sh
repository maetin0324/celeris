#!/usr/bin/env bash
# A5（ADR 2026-11-04-project-controller 付記 A5）: 案件の Roadmap（版付きの途中目標）の受け入れの検査。
#   bash scripts/dev/test-project-roadmap.sh [--out <dir>]
# 1. 接頭辞 `project_roadmap_` の試験を nextest で走らせる。
#    - task-core（lib）: 検証（形・stable id・取り下げ）、版の前進は決定だけ、行の追記専用（trigger）、
#      Outcome の記録の条件、migration 0113 の後方互換（0112 の DB から起動して行を保つ）。
#    - task-api（tests/project_roadmap.rs）: 提案は版を進めない / 人の承認で版が進み旧版が superseded /
#      CoS・operator は決められない / stable id が版をまたぐ / root と Outcome の対応の履歴と Rust の evidence /
#      旧 milestones の 410 が不変 / task の milestone_id を読まない / brief の全置換で Roadmap が変わらない /
#      block → Roadmap の優先順 / 却下で版が変わらない。
#    core 7 件・api 10 件より少ない、失敗が 1 件でもあれば失敗。
# 2. 旧 milestones・project_plan の 410 の既存試験（crates/task-api/tests/project_plan.rs）を走らせる（1 件以上）。
# 3. 試験が書いた raw（celeris.long-horizon-record/1 の roadmap.jsonl）を読み、版の遷移
#    （proposed・approved・superseded・rejected）と主体（決定は human: だけ、CoS・operator の決定は拒否）、
#    reached の記録の主体と root の対応、block → Roadmap の優先を検査する。
# 4. Roadmap の実装の file が旧 milestones（milestone_id・milestones 表・auto_advance）と ExecutionPlan の
#    stage・WorkUnit の phase を読まないこと（コメントを除いた grep）。
# 5. web の panel の単体試験（vitest）。
# LLM を呼ばない（CoS は fake の run の credential）。本番の DB・daemon に触れない（一時 DB と $TMPDIR だけ）。
# 前提（cargo-nextest・python3・node・pnpm）が欠けたら exit 1。CPU を焼く負荷はかけない。
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
# 本番・測定の config・DB・run の値を読ませない。
unset CELERIS_CONFIG CELERIS_HOME CELERIS_DB CELERIS_RUN_DB CELERIS_API_TOKEN CELERIS_API_URL \
  CELERIS_FOLLOWUPS_FILE CELERIS_RUN_ID CELERIS_TASK_ID CELERIS_COS_RUN_CREDENTIAL CELERIS_API_PRINCIPAL || true
[ -n "$out" ] || out="$(mktemp -d "${TMPDIR:-/tmp}/pc-roadmap.XXXXXX")"
mkdir -p "$out"
case "$out" in /*) ;; *) out="$PWD/$out" ;; esac
case "$out" in
  /local/celeris/data/db*|"${CELERIS_STATE_DIR:-/nonexistent}"*) echo "test-project-roadmap: FAIL: --out is under a production state dir" >&2; exit 1 ;;
esac
rm -f "$out"/roadmap.jsonl "$out"/nextest-*.log

fail() { echo "test-project-roadmap: FAIL: $*" >&2; exit 1; }
command -v python3 >/dev/null || fail "python3 is required"
command -v node >/dev/null || fail "node is required"
command -v pnpm >/dev/null || fail "pnpm is required"
cargo nextest --version >/dev/null 2>&1 || fail "cargo-nextest is required"

run_suite() {
  local name="$1" min="$2"; shift 2
  local log="$out/nextest-$name.log"
  echo "== nextest ($name)"
  set +e
  PROJECT_ROADMAP_RAW_DIR="$out" cargo nextest run "$@" --no-fail-fast 2>&1 | tee "$log"
  local rc=${PIPESTATUS[0]}
  set -e
  [ "$rc" -eq 0 ] || fail "nextest ($name) exited $rc"
  local summary passed skipped
  summary="$(grep -E 'Summary .* tests? run' "$log" | tail -1)"
  passed="$(printf '%s\n' "$summary" | sed -nE 's/.* ([0-9]+) passed.*/\1/p')"
  [ -n "$passed" ] || fail "no nextest summary ($name)"
  [ "$passed" -ge "$min" ] || fail "only $passed tests passed in $name (need >= $min)"
  skipped="$(printf '%s\n' "$summary" | sed -nE 's/.* ([0-9]+) skipped.*/\1/p')"
  echo "$name: $passed passed${skipped:+ (${skipped} filtered out)}"
  eval "passed_$name=$passed"
}

echo "== 1. project_roadmap_ tests"
run_suite core 7 -p task-core --lib -E 'test(/(^|::)project_roadmap_/)'
run_suite api 10 -p task-api --test project_roadmap -E 'test(/^project_roadmap_/)'
total=$((passed_core + passed_api))
[ "$total" -ge 17 ] || fail "only $total project_roadmap_ tests (need >= 17)"
echo "project_roadmap_ tests passed: $total"

echo "== 2. legacy milestones / project_plan 410 (unchanged)"
run_suite plan410 1 -p task-api --test project_plan

echo "== 3. raw records"
python3 -I - "$out/roadmap.jsonl" <<'PY'
import json, sys
path = sys.argv[1]
try:
    rows = [json.loads(l) for l in open(path) if l.strip()]
except FileNotFoundError:
    sys.exit(f"test-project-roadmap: FAIL: no raw records at {path}")
def fail(msg):
    sys.exit(f"test-project-roadmap: FAIL: raw: {msg}")
if not rows:
    fail("empty")
for r in rows:
    if r.get("schema") != "celeris.long-horizon-record/1":
        fail(f"bad schema {r}")
by = {}
for r in rows:
    by.setdefault(r["test"], []).append(r)
need = [
    "project_roadmap_proposal_does_not_advance_the_version",
    "project_roadmap_human_approval_advances_and_supersedes",
    "project_roadmap_cos_and_operator_cannot_decide",
    "project_roadmap_root_links_and_reached_need_rust_evidence",
    "project_roadmap_takes_precedence_over_the_request_block",
    "project_roadmap_reject_keeps_the_current_version",
]
for t in need:
    if t not in by:
        fail(f"missing records of {t}")
p = by["project_roadmap_proposal_does_not_advance_the_version"]
if [r["to"] for r in p] != ["proposed"]:
    fail(f"a proposal must stay proposed: {p}")
a = by["project_roadmap_human_approval_advances_and_supersedes"]
if [r["to"] for r in a] != ["superseded", "approved"]:
    fail(f"versions must be superseded then approved: {a}")
for r in a:
    if not str(r.get("decided_by") or "").startswith("human:"):
        fail(f"a version advanced without a person: {r}")
c = by["project_roadmap_cos_and_operator_cannot_decide"]
refused = {r["actor"]: r["decide_status"] for r in c}
if refused != {"cos": 422, "cos-direct": 422, "operator": 403}:
    fail(f"non-human decisions must be refused: {refused}")
e = by["project_roadmap_root_links_and_reached_need_rust_evidence"]
kinds = [(r["kind"], r["status"]) for r in e]
if kinds != [("root_linked", None), ("status", "in_progress"), ("root_linked", None), ("status", "reached")]:
    fail(f"outcome history: {kinds}")
if any(not str(r["actor"]).startswith("human:") for r in e):
    fail(f"outcome status by a non-human: {e}")
if len({r["root"] for r in e if r["kind"] == "root_linked"}) != 2:
    fail("two roots must be linked to the outcome")
pr = by["project_roadmap_takes_precedence_over_the_request_block"][0]
if pr["source"] != "roadmap" or pr["block_outcomes"] != 2 or pr["roadmap_outcomes"] != 2:
    fail(f"precedence: {pr}")
rj = by["project_roadmap_reject_keeps_the_current_version"][0]
if rj["to"] != "rejected" or not str(rj["actor"]).startswith("human:"):
    fail(f"reject: {rj}")
print(f"raw ok: {len(rows)} records, transitions proposed/approved/superseded/rejected, non-human decide refused {refused}")
PY

echo "== 4. no legacy milestones / plan phases in the roadmap code"
files="crates/task-core/src/roadmap.rs crates/task-core/src/store/roadmap.rs crates/task-ops/src/roadmap.rs crates/task-api/src/handlers/roadmap.rs crates/task-core/migrations/0113_project_roadmap.sql"
for f in $files; do
  [ -s "$f" ] || fail "missing $f"
done
# shellcheck disable=SC2086
hits="$(grep -nE 'milestone_id|MilestoneId|FROM milestones|auto_advance|work_unit|WorkUnit|stage_id|\.phase\b' $files | grep -vE ':[0-9]+:\s*(//|--)' || true)"
[ -z "$hits" ] || fail "the roadmap code reads legacy milestones or plan phases: $hits"
echo "ok: roadmap code does not read milestones, auto_advance or plan phases"

echo "== 5. web panel"
if [ ! -d web/node_modules ]; then
  (cd web && pnpm install --offline --frozen-lockfile) || fail "web dependencies are missing (pnpm install --offline failed)"
fi
(cd web && pnpm exec vitest run features/projects/project-roadmap-panel.test.tsx) || fail "web panel tests failed"

echo "test-project-roadmap: OK ($total project_roadmap_ tests, raw in $out)"
