#!/usr/bin/env bash
# A3（ADR 2026-11-04-project-controller §A3）: 案件 CoS の運営（A1 の wake と A2 の TS の接続）の受け入れの検査。
#   bash scripts/dev/test-project-cos-shadow.sh [--out <dir>]
# 1. 接頭辞 `project_cos_` の試験を 3 crate で走らせる（nextest）。
#    - task-dispatch: A1 の tick に A3 の TsProjectDecider を入れ、fake の script で dry-run 以外の経路
#      （shadow→enable、off への rollback、off と状態行なしの一致、explanation の監査、失敗の分類、runaway の停止、
#      同じ問いを聞き直さない、worker の報告の命令を無視、TS が使えないときの fail closed、受け入れ条件を狭めた出力の拒否）。
#    - task-api: CoS の拒否（依頼文・objective/acceptance・controller の起票・本番 promote・secret・人だけの操作）と
#      受信箱の代答が既存の 7 類型の basis を要すること。
#    - celeris: 本物の bundle を本物の V8 で（dry-run の行数の差分 0、規則 1 行の変更で判定が変わる〈cargo 0〉、
#      brief・題の命令で判定が変わらない、失敗の分類つきの replan）。CELERIS_SCRIPT_REQUIRE_BUNDLE=1 で SKIP を失敗にする。
#    合計 14 件未満、失敗・SKIP が 1 件でもあれば失敗。
# 2. 試験が書いた raw（celeris.long-horizon-record/1）を読み、shadow の CoS run 数 0・injection の root 数 0・
#    拒否の code・runaway の停止（escalate 1、blocked）を検査する。
# 3. control-ts の領域の単体試験と fixture の不変条件（compare.mjs）。
# LLM を呼ばない（fake の script と注入した時計）。本番の DB・daemon に触れない（一時 DB と $TMPDIR だけ）。
# 前提（node・cargo-nextest・control-ts の依存）が欠けたら exit 1。CPU を焼く負荷はかけない。
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
# 本番・測定の config・DB・run の値を読ませない（ADR D11）。
unset CELERIS_CONFIG CELERIS_HOME CELERIS_DB CELERIS_RUN_DB CELERIS_API_TOKEN CELERIS_API_URL CELERIS_SCRIPT_BUNDLE_DIR \
  CELERIS_FOLLOWUPS_FILE CELERIS_RUN_ID CELERIS_TASK_ID CELERIS_COS_RUN_CREDENTIAL CELERIS_API_PRINCIPAL || true
[ -n "$out" ] || out="$(mktemp -d "${TMPDIR:-/tmp}/pc-cos.XXXXXX")"
mkdir -p "$out"
case "$out" in /*) ;; *) out="$PWD/$out" ;; esac
case "$out" in
  /local/celeris/data/db*|"${CELERIS_STATE_DIR:-/nonexistent}"*) echo "test-project-cos-shadow: FAIL: --out is under a production state dir" >&2; exit 1 ;;
esac
rm -f "$out"/*.jsonl "$out/skip.log" "$out"/nextest-*.log

fail() { echo "test-project-cos-shadow: FAIL: $*" >&2; exit 1; }
command -v node >/dev/null || fail "node is required"
command -v python3 >/dev/null || fail "python3 is required"
cargo nextest --version >/dev/null 2>&1 || fail "cargo-nextest is required"
if [ ! -d control-ts/node_modules ]; then
  (cd control-ts && pnpm install --offline --frozen-lockfile) || fail "control-ts dependencies are missing (pnpm install --offline failed)"
fi

total=0
run_suite() {
  local name="$1" min="$2"; shift 2
  local log="$out/nextest-$name.log"
  echo "== 1. nextest project_cos_ ($name)"
  set +e
  PROJECT_COS_RAW_DIR="$out" CELERIS_SCRIPT_REQUIRE_BUNDLE=1 CELERIS_SCRIPT_SKIP_LOG="$out/skip.log" \
    cargo nextest run "$@" --no-fail-fast 2>&1 | tee "$log"
  local rc=${PIPESTATUS[0]}
  set -e
  [ "$rc" -eq 0 ] || fail "nextest ($name) exited $rc"
  local summary passed
  summary="$(grep -E 'Summary .* tests? run' "$log" | tail -1)"
  passed="$(printf '%s\n' "$summary" | sed -nE 's/.* ([0-9]+) passed.*/\1/p')"
  [ -n "$passed" ] || fail "no nextest summary ($name)"
  [ "$passed" -ge "$min" ] || fail "only $passed project_cos_ tests passed in $name (need >= $min)"
  total=$((total + passed))
}
run_suite dispatch 11 -p task-dispatch --lib -E 'test(/(^|::)project_cos_/)'
run_suite api 7 -p task-api --test project_cos_guard -E 'test(/^project_cos_/)'
run_suite celeris 4 -p celeris --test project_cos_shadow -E 'test(/^project_cos_/)'
[ "$total" -ge 22 ] || fail "only $total project_cos_ tests (need >= 22)"
[ ! -s "$out/skip.log" ] || fail "bundle SKIP recorded: $(cat "$out/skip.log")"
echo "project_cos_ tests passed: $total"

echo "== 2. raw records"
python3 -I - "$out" <<'PY'
import glob, json, os, sys
out = sys.argv[1]
rows = {}
for f in sorted(glob.glob(os.path.join(out, "*.jsonl"))):
    for line in open(f):
        if line.strip():
            r = json.loads(line)
            rows.setdefault(r.get("test"), []).append(r)
problems = []
def need(cond, msg):
    if not cond:
        problems.append(msg)
for name, rs in rows.items():
    for r in rs:
        need(r.get("schema") == "celeris.long-horizon-record/1", f"{name}: schema")
def one(name):
    rs = rows.get(name, [])
    need(rs, f"missing raw: {name}")
    return rs[-1] if rs else {}
sh = one("project_cos_shadow_then_enable")
need(sh.get("shadow_cos_runs") == 0, f"shadow launched a CoS run: {sh}")
need(sh.get("shadow_effect") == "suppressed" and sh.get("enabled_roots") == 1, f"shadow/enable: {sh}")
rb = one("project_cos_rollback_off")
need(rb.get("roots_kept") == 1 and rb.get("wakes_after_off") == 0 and rb.get("decisions_after_off") == 0, f"rollback: {rb}")
ol = one("project_cos_off_equals_legacy")
need(ol and ol.get("off") == ol.get("legacy"), f"off != legacy: {ol}")
ru = one("project_cos_runaway_stops")
need(ru.get("escalations") == 1 and ru.get("phase") == "blocked" and ru.get("ts_calls", 99) <= 6, f"runaway: {ru}")
q = one("project_cos_no_needless_question")
need(q.get("escalations") == 1, f"asked twice: {q}")
inj = one("project_cos_injection_report_ignored")
need(inj.get("roots") == 0 and inj.get("snapshot_has_injection") is False, f"injection (report): {inj}")
inj2 = one("project_cos_ts_injection_does_not_change_the_decision")
need(inj2.get("roots_created") == 0 and inj2.get("same_decision") is True, f"injection (TS): {inj2}")
cl = one("project_cos_failure_classified")
need(cl.get("classification") == "spec_gap" and cl.get("cos_inputs") == 1, f"classified: {cl}")
cl2 = one("project_cos_ts_failure_classified")
need(cl2.get("classification") == "spec_gap", f"classified (TS): {cl2}")
nar = one("project_cos_narrowed_acceptance_rejected")
need(nar.get("roots") == 0, f"narrowed: {nar}")
dr = one("project_cos_ts_dry_run_compare_domain")
need(dr.get("row_delta") == 0 and dr.get("action") == "launch_root", f"dry-run (TS): {dr}")
dm = one("project_cos_dry_run_on_request")
need(dm.get("decisions") == 2 and dm.get("effect") == "suppressed" and dm.get("roots") == 0
     and dm.get("cos_runs") == 0 and dm.get("calls_without_request") == 0, f"dry-run mode: {dm}")
tc = one("project_cos_ts_change_without_rust")
need(tc.get("before_secs") == 3600 and tc.get("after_secs") == 60 and tc.get("cargo_invocations") == 0, f"ts change: {tc}")
codes = {r.get("refusal_code") for name, rs in rows.items() for r in rs if r.get("refusal_code")}
for code in ("project_request_human_only", "task_goal_human_only", "project_controller_owns_launch",
             "production_requires_human", "cos_basis_missing"):
    need(code in codes, f"refusal code not recorded: {code}")
print(json.dumps({"tests": len(rows), "rows": sum(len(v) for v in rows.values()), "refusal_codes": sorted(codes),
                  "problems": problems}, ensure_ascii=False))
sys.exit(1 if problems else 0)
PY

echo "== 3. control-ts unit and invariants"
(cd control-ts && ./node_modules/.bin/vitest run src/domains/project_controller.test.ts) || fail "vitest"
(cd control-ts && node scripts/compare.mjs project_controller) || fail "compare.mjs project_controller"
echo "test-project-cos-shadow: OK ($total tests, raw in $out)"
