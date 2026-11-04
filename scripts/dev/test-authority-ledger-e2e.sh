#!/usr/bin/env bash
# A7（ADR 2026-11-04-project-controller §A7・付記 A7）: 人の許可範囲（authority ledger）の受け入れの検査。
#   bash scripts/dev/test-authority-ledger-e2e.sh [--out <dir>]
# 1. 接頭辞 `authority_ledger_` の試験を 3 crate で走らせる（nextest）。
#    - task-core: 規則（期限・予算・取り消し・読めない期限・範囲の包含）。
#    - task-api: `/cos/operations` と `…/authority/*` の E2E（6 分類の拒否・状態不明の拒否・期限切れ・予算切れ・取り消し・
#      根拠つきの許可と元リンク・同じ問いを聞き直さない・監査の 4 欄・偽の人の指示の拒否・human_required の解除不可・
#      人だけが許可を作る・off の案件は従来どおり・enabled にした人の操作が期限と回数つきの許可を作る・
#      途中目標の範囲は approved の Roadmap の版を読む）。
#    - task-dispatch: controller の tick（許可なしの起票は人に 1 回聞いて止まる・許可で続く・予算と期限・答えのある問いを
#      聞き直さない・判断の後の取り消しを効果の tx が拒否する・shadow は記録だけ・approved の Roadmap の Outcome の起票）。
#    合計 28 件未満、失敗が 1 件でもあれば失敗。
# 2. 試験が書いた raw（celeris.long-horizon-record/1）を読み、分類ごとの拒否の code・監査の 4 欄・偽の指示の拒否を検査する。
# LLM を呼ばない（fake の CoS run の credential と fake の script、注入した時計）。本番の DB・daemon に触れない
# （一時 DB と $TMPDIR だけ）。前提（cargo-nextest・python3）が欠けたら exit 1（SKIP にしない）。CPU を焼く負荷はかけない。
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
[ -n "$out" ] || out="$(mktemp -d "${TMPDIR:-/tmp}/authority-ledger.XXXXXX")"
mkdir -p "$out"
case "$out" in /*) ;; *) out="$PWD/$out" ;; esac
case "$out" in
  /local/celeris/data/db*|"${CELERIS_STATE_DIR:-/nonexistent}"*) echo "test-authority-ledger-e2e: FAIL: --out is under a production state dir" >&2; exit 1 ;;
esac
rm -f "$out"/*.jsonl "$out"/nextest-*.log

fail() { echo "test-authority-ledger-e2e: FAIL: $*" >&2; exit 1; }
command -v python3 >/dev/null || fail "python3 is required"
cargo nextest --version >/dev/null 2>&1 || fail "cargo-nextest is required"

total=0
run_suite() {
  local name="$1" min="$2"; shift 2
  local log="$out/nextest-$name.log"
  echo "== 1. nextest authority_ledger_ ($name)"
  set +e
  AUTHORITY_LEDGER_RAW_DIR="$out" cargo nextest run "$@" --no-fail-fast 2>&1 | tee "$log"
  local rc=${PIPESTATUS[0]}
  set -e
  [ "$rc" -eq 0 ] || fail "nextest ($name) exited $rc"
  local summary passed skipped
  summary="$(grep -E 'Summary .* tests? run' "$log" | tail -1)"
  passed="$(printf '%s\n' "$summary" | sed -nE 's/.* ([0-9]+) passed.*/\1/p')"
  [ -n "$passed" ] || fail "no nextest summary ($name)"
  [ "$passed" -ge "$min" ] || fail "only $passed authority_ledger_ tests passed in $name (need >= $min)"
  total=$((total + passed))
}
run_suite core 2 -p task-core --lib -E 'test(/(^|::)authority_ledger_/)'
run_suite api 19 -p task-api --test authority_ledger -E 'test(/^authority_ledger_/)'
run_suite dispatch 7 -p task-dispatch --lib -E 'test(/(^|::)authority_ledger_/)'
[ "$total" -ge 28 ] || fail "only $total authority_ledger_ tests (need >= 28)"
echo "authority_ledger_ tests passed: $total"

echo "== 2. raw records"
python3 -I - "$out" <<'PY'
import glob, json, os, sys
out = sys.argv[1]
rows = []
for f in sorted(glob.glob(os.path.join(out, "*.jsonl"))):
    for line in open(f):
        if line.strip():
            rows.append(json.loads(line))
problems = []
def need(cond, msg):
    if not cond:
        problems.append(msg)
for r in rows:
    need(r.get("schema") == "celeris.long-horizon-record/1", f"schema: {r}")
by = lambda t: [r for r in rows if r.get("test") == t]
# 6 分類それぞれの拒否（許可なし・状態不明）と /cos/operations の code。
refusals = by("class_refusal")
classes = {"human_required", "destructive", "production", "api_authorization", "external_publish", "opaque_external_effect"}
for c in classes:
    cases = {r["case"]: r["code"] for r in refusals if r["class"] == c}
    need("no_grant" in cases, f"{c}: no denial without a grant")
    need("prior_outcome_unknown" in cases or "state_unknown" in cases, f"{c}: no denial when the state is unknown")
    if c != "human_required":
        need(cases.get("cos_operation", "").endswith("_requires_human"), f"{c}: /cos/operations refusal code {cases.get('cos_operation')}")
need({r["code"] for r in refusals if r["case"] == "cos_operation"} == {
    "production_requires_human", "external_publication_requires_human", "irreversible_requires_human",
    "api_authorization_requires_human", "opaque_effect_requires_human"}, "the five /cos/operations refusal codes")
cases = {(r["class"], r["case"]): r["code"] for r in refusals}
need(cases.get(("destructive", "expired")) == "grant_expired", "expired grant refused")
need(cases.get(("production", "budget")) == "grant_budget_exhausted", "exhausted budget refused")
need(cases.get(("destructive", "revoked")) == "grant_revoked", "revoked grant refused")
need(cases.get(("destructive", "source_gone")) == "grant_source_unreadable", "unreadable source refused")
need(cases.get(("human_required", "grant_refused")) == "authority_human_required_not_grantable", "human_required grant refused")
need(cases.get(("human_required", "cos_answer_with_grant")) == "cos_human_required", "human_required not lifted by a grant")
# 監査の 4 欄（suggestion・observed outcome・actual actor・human decision）。
audit = by("audit_fields")
need({r.get("path") for r in audit} >= {"cos_operations", "instructed_by"}, f"audit rows: {audit}")
for r in audit:
    need(r.get("suggestion") is True and r.get("observed_outcome") is True, f"audit suggestion/outcome: {r}")
    need(str(r.get("actual_actor", "")).startswith("cos:"), f"audit actor: {r}")
    need(str(r.get("human_decision", "")).split(":")[0] in ("authority_grant", "human_message"), f"audit human decision: {r}")
launch = by("authority_ledger_launch_requires_grant_then_runs")
need(launch and launch[-1]["escalations"] == 1 and launch[-1]["roots"] == 1 and launch[-1]["consumed"] == 1, f"launch: {launch}")
need(launch and launch[-1]["audit"]["actual_actor"] == "daemon"
     and str(launch[-1]["audit"]["human_decision"]).startswith("authority_grant:")
     and launch[-1]["audit"]["suggestion"] and launch[-1]["audit"]["observed_outcome"], f"controller audit: {launch}")
# 偽の人の指示はすべて拒否。
fake = {r["case"]: r["code"] for r in by("fake_instruction")}
for case in ("cos_text", "system_input", "proposal", "operator_message", "relay", "other_project", "log_text",
             "reserved_kind", "unknown_kind", "cos_answered_decision", "not_a_person"):
    need(case in fake, f"fake instruction case missing: {case}")
need(all(code.startswith("authority_") for code in fake.values()), f"fake instruction codes: {fake}")
# 同じ問いを聞き直さない。
same = by("same_question")
need(same and same[-1]["notices"] == same[-1]["asked"], f"same question: {same}")
again = by("authority_ledger_answered_question_is_not_asked_again")
need(again and again[-1]["asked_while_answered"] == 0, f"answered question: {again}")
rev = by("authority_ledger_revoked_after_decision_creates_no_root")
need(rev and rev[-1]["roots"] == 0 and rev[-1]["effect_ref"] == "authority:grant_revoked", f"revoked after decision: {rev}")
# 途中目標の範囲は approved の Roadmap の版を読む（block・取り下げ・proposed・rejected は範囲にならない）。
scope = {r["case"]: r["code"] for r in by("roadmap_scope")}
need(scope.get("roadmap_only") == "allowed", f"roadmap only outcome: {scope}")
for case in ("block_only_check", "dropped_check", "proposed_check", "rejected_check", "withdrawn_check"):
    need(scope.get(case) == "unknown_scope", f"roadmap {case}: {scope.get(case)}")
for case in ("block_only_grant", "dropped_grant"):
    need(scope.get(case) == "authority_scope_unknown", f"roadmap {case}: {scope.get(case)}")
need(scope.get("unreadable_check") == "ledger_unreadable", f"roadmap unreadable: {scope.get('unreadable_check')}")
rl = {r["grant"]: r for r in by("authority_ledger_roadmap_launch")}
for kind in ("project", "outcome"):
    r = rl.get(kind)
    need(r and r["roots"] == 1 and r["escalations"] == 0 and r["consumed"] == 1, f"roadmap launch with a {kind} grant: {r}")
oth = by("authority_ledger_roadmap_other_outcome")
need(oth and oth[-1]["roots_denied"] == 0 and oth[-1]["escalations"] == 1 and oth[-1]["reason"] == "no_grant"
     and oth[-1]["roots_after_grant"] == 1, f"roadmap other outcome: {oth}")
print(json.dumps({"rows": len(rows), "fake_cases": sorted(fake), "problems": problems}, ensure_ascii=False))
sys.exit(1 if problems else 0)
PY
echo "test-authority-ledger-e2e: OK ($total tests, raw in $out)"
