#!/usr/bin/env bash
# A4（ADR 2026-11-04-project-controller §A4・付記 A4）: 統括 CoS の相談・小規模作業の受け入れの検査。
#   bash scripts/dev/test-general-cos-consult-e2e.sh [--out <dir>]
# 1. 接頭辞 `general_cos_consult_` の試験を task-core・task-worker・task-api・task-dispatch で nextest で走らせる
#    （13 件未満・skip 1 件でも失敗）。fake harness（試験が CoS run の credential で API を呼ぶ、偽の claude）と
#    試験用の時計だけで、LLM を呼ばない。本番の DB・daemon・release 置き場に触れない（一時 DB と $TMPDIR だけ）。
# 2. 試験が書いた raw（celeris.general-cos-consult/1）を読んで受け入れ条件を検査する:
#    「どうするべき？」→ task 0・返事 1、「直近 3 件を比較して」→ task 0・run 内の読取 ≥ 1・返事 1、
#    「これを修正して」→ task 1・完了の要約が元の統括 thread に戻り task id が読める（review の判定が無ければ「合格」と書かない）、
#    読取の上限を超えると run が止まる、CoS run の LLM CLI 起動で run が止まる、proposal は role=system で instructed_by に
#    引くと 422、relay_forward に本文を入れると 422、統括と案件の run・session が別。走っただけでは通さない。
# 前提（cargo-nextest・python3）が欠けたら SKIP でなく exit 1。CPU を焼く負荷はかけない。
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
# 本番・測定の config・DB・token を読ませない（ADR D11）。
unset CELERIS_CONFIG CELERIS_HOME CELERIS_DB CELERIS_API_TOKEN CELERIS_API_URL CELERIS_SCRIPT_BUNDLE_DIR || true

fail() { echo "test-general-cos-consult-e2e: FAIL: $*" >&2; exit 1; }
command -v python3 >/dev/null || fail "python3 is required"
cargo nextest --version >/dev/null 2>&1 || fail "cargo-nextest is required"

[ -n "$out" ] || out="$(mktemp -d "${TMPDIR:-/tmp}/gcc-e2e.XXXXXX")"
mkdir -p "$out"
case "$out" in /*) ;; *) out="$PWD/$out" ;; esac
rm -f "$out"/general_cos_consult_*.jsonl

echo "== 1. nextest general_cos_consult_"
log="$out/nextest.log"
set +e
CELERIS_GCC_E2E_OUT="$out" cargo nextest run -p task-core -p task-worker -p task-api -p task-dispatch \
  -E 'test(/general_cos_consult_/)' --no-fail-fast 2>&1 | tee "$log"
rc=${PIPESTATUS[0]}
set -e
[ "$rc" -eq 0 ] || fail "nextest exited $rc"
summary="$(grep -E 'Summary .* tests? run' "$log" | tail -1)"
passed="$(printf '%s\n' "$summary" | sed -nE 's/.* ([0-9]+) passed.*/\1/p')"
[ -n "$passed" ] || fail "no nextest summary"
[ "$passed" -ge 13 ] || fail "only $passed general_cos_consult_ tests passed (need >= 13)"
if printf '%s\n' "$summary" | grep -qE ' [0-9]+ (failed|timed out)'; then fail "failures: $summary"; fi
# nextest の skipped は filter で外れた試験の数。対象の試験の skip（#[ignore]）は「N passed」の不足で落ちる。

echo "== 2. raw records"
python3 -I - "$out" <<'PY'
import glob, json, os, sys
out = sys.argv[1]
recs = {}
for f in sorted(glob.glob(os.path.join(out, "general_cos_consult_*.jsonl"))):
    rows = [json.loads(l) for l in open(f) if l.strip()]
    recs[os.path.basename(f)[:-6]] = rows[-1] if rows else None
problems = []
def need(cond, msg):
    if not cond:
        problems.append(msg)
def rec(name):
    r = recs.get("general_cos_consult_" + name)
    need(r is not None, f"raw record missing: {name}")
    return r or {}
for name, r in recs.items():
    need(r and r.get("schema") == "celeris.general-cos-consult/1", f"{name}: schema")
    need(r and r.get("llm_launches") == 0, f"{name}: an LLM was launched")

c = rec("consult_answers_without_a_task")
need(c.get("intent") == "consult" and c.get("tasks_created") == 0 and c.get("replies") == 1,
     f"consult: {c}")
r = rec("read_completes_inside_the_run")
need(r.get("intent") == "read" and r.get("tasks_created") == 0 and r.get("reads", 0) >= 1
     and r.get("items_read", 0) >= 1 and r.get("replies") == 1, f"read: {r}")
d = rec("delegate_creates_one_task_traced_to_the_origin_thread")
need(d.get("intent") == "delegate" and d.get("tasks_created") == 1 and d.get("task_id")
     and d.get("origin_thread"), f"delegate: {d}")
s = rec("done_summary_returns_to_the_origin_thread")
need(s.get("summary_in_origin") is True and s.get("summary_thread") == s.get("origin_thread")
     and s.get("summary_mentions_task") is True and s.get("summary_says_pass_without_verdict") is False
     and s.get("plain_root_thread_is_project") is True and s.get("origin_replies", 0) >= 1,
     f"done summary: {s}")
w = rec("short_read_within_the_budget_completes")
need(w.get("terminal") == "done" and 1 <= w.get("reads", 0) <= w.get("max", 0), f"within budget: {w}")
o = rec("read_over_the_budget_stops_the_run")
need(o.get("terminal") == "error" and o.get("reads", 0) > o.get("max", 0)
     and str(o.get("stop", "")).startswith("cos_inline_budget_exceeded"), f"over budget: {o}")
l = rec("llm_cli_launch_stops_the_cos_run")
need(l.get("cos_terminal") == "error" and str(l.get("stop", "")).startswith("cos_llm_launch_refused")
     and l.get("violations") == 1 and l.get("worker_terminal") == "done", f"llm cli: {l}")
p = rec("proposal_is_cos_words_and_never_an_instruction")
need(p.get("proposal_role") == "system" and p.get("proposals") == 1 and p.get("instructed_by_status") == 422
     and p.get("direct_status") == 403 and p.get("from_project_status") == 422, f"proposal: {p}")
f = rec("relay_forward_with_cos_text_is_refused")
need(f.get("forged_text_status") == 422 and f.get("cos_reply_forward_status") == 422
     and f.get("project_messages") == 0, f"relay forgery: {f}")
x = rec("general_and_project_sessions_stay_separate")
need(x.get("threads_with_runs") == 2 and x.get("general_queued_after_proposal") == 0
     and x.get("cross_checkpoint_status") in (403, 409, 422), f"sessions: {x}")
print(json.dumps({"records": len(recs), "per_utterance_tasks": {
    "consult": c.get("tasks_created"), "read": r.get("tasks_created"), "delegate": d.get("tasks_created")},
    "reads": r.get("reads"), "summary_in_origin": s.get("summary_in_origin"), "problems": problems},
    ensure_ascii=False))
sys.exit(1 if problems else 0)
PY
echo "test-general-cos-consult-e2e: OK (raw in $out)"
