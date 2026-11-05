#!/usr/bin/env bash
# P1-G（ADR 2026-11-05-phase3-p1g、P0-A の K29〜K40）: 自律復帰の受け入れ試験。
#
# 1. task-dispatch の `project_recovery_` 試験（注入した時計・fake の script。k38 だけ本物の bundle）と task-api の
#    `project_recovery_`、task-ops・task-dispatch の `p1g_` を走らせる。
# 2. 試験が書いた raw（recovery.jsonl・human_wait_measure.jsonl）の期待値を検査する。k38 は、基準の tree で同じ台本を
#    動かした raw（`--before <file>`、既定は進捗の p1g/human_wait_before.jsonl）より評価が減っていることを確かめる。
#
# LLM を呼ばない。本番の DB・daemon に触れない（試験は一時 DB だけを開く）。前提が欠けたら SKIP せずに失敗する。
# 使い方: bash scripts/dev/test-project-recovery.sh [--out <dir>] [--before <file>]
set -euo pipefail

out=""
before="agent-docs/progress/2026-11-05-phase3/p1g/human_wait_before.jsonl"
while [ $# -gt 0 ]; do
    case "$1" in
        --out) out="$2"; shift 2 ;;
        --before) before="$2"; shift 2 ;;
        *) echo "usage: $0 [--out <dir>] [--before <file>]" >&2; exit 2 ;;
    esac
done

fail() { echo "test-project-recovery: FAIL: $*" >&2; exit 1; }

command -v cargo >/dev/null || fail "cargo is missing"
cargo nextest --version >/dev/null 2>&1 || fail "cargo-nextest is missing"
command -v python3 >/dev/null || fail "python3 is missing"
command -v node >/dev/null || fail "node is missing"
[ -f "$before" ] || fail "no before raw: $before"
before="$(cd "$(dirname "$before")" && pwd)/$(basename "$before")"

unset CELERIS_RUN_DB CELERIS_DB CELERIS_FOLLOWUPS_FILE CELERIS_RUN_ID CELERIS_TASK_ID CELERIS_CONFIG CELERIS_HOME \
    CELERIS_COS_RUN_CREDENTIAL CELERIS_API_PRINCIPAL CELERIS_API_TOKEN CELERIS_API_URL CELERIS_SCRIPT_BUNDLE_DIR || true

if [ -z "$out" ]; then
    out="$(mktemp -d "${TMPDIR:-/tmp}/project-recovery.XXXXXX")"
fi
mkdir -p "$out"
out="$(cd "$out" && pwd)"
rm -f "${out:?}/recovery.jsonl" "${out:?}/human_wait_measure.jsonl" "${out:?}/skip.log"
export PROJECT_RECOVERY_RAW_DIR="$out"

if [ ! -d control-ts/node_modules ]; then
    (cd control-ts && pnpm install --offline --frozen-lockfile) \
        || fail "control-ts dependencies are missing (pnpm install --offline failed)"
fi
bundle="$(mktemp -d "${TMPDIR:-/tmp}/project-recovery-bundle.XXXXXX")"
(cd control-ts && node --input-type=module -e "import {buildBundle} from './scripts/build-bundle.mjs'; await buildBundle({outDir: process.argv[1]});" "$bundle") \
    || fail "bundle build failed"
export CELERIS_SCRIPT_BUNDLE_DIR="$bundle" CELERIS_SCRIPT_REQUIRE_BUNDLE=1

cargo nextest run -p task-dispatch -p task-api -p task-ops -p task-core \
    -E 'test(project_recovery_) | test(p1g_)' 2>&1 | tee "$out/nextest.log" \
    || fail "nextest failed (see $out/nextest.log)"

python3 -I - "$out" "$before" <<'PY'
import json, re, sys, pathlib
out, before = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
def rows(path):
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
log = (out / "nextest.log").read_text()
m = re.search(r"Summary .*? (\d+) tests? run: (\d+) passed", log)
assert m and m.group(1) == m.group(2) and int(m.group(1)) >= 25, f"nextest summary: {m and m.group(0)}"
rec = rows(out / "recovery.jsonl")
by = {}
for r in rec:
    by.setdefault(r["test"], []).append(r)
need = ["project_recovery_k29", "project_recovery_k29_resume", "project_recovery_k30", "project_recovery_k31",
        "project_recovery_k32", "project_recovery_k32_root_done", "project_recovery_k33", "project_recovery_k34_clear",
        "project_recovery_k34_request", "project_recovery_k35_deny", "project_recovery_k36",
        "project_recovery_k37_stop", "project_recovery_k37_human_required", "project_recovery_k37_dangerous_budget",
        "project_recovery_k39", "project_recovery_k40_tokens", "project_recovery_k40_roots",
        "project_recovery_k40_limit", "project_recovery_k40_same_day_reset", "project_recovery_cos_brief_ttl"]
missing = [n for n in need if n not in by]
assert not missing, f"missing raw: {missing}"
assert len(by["project_recovery_k29"]) == 8 and all(r["brief_wakes"] == 1 and r["roots"] == 1 for r in by["project_recovery_k29"])
assert by["project_recovery_k31"][0]["evaluations_after_jump"] == 1
assert by["project_recovery_k32"][0]["questions"] == 2 and by["project_recovery_k32"][0]["roots"] == 0
assert all(r["roots_from_old_decision"] == 0 and r["roots_after_reevaluation"] == 1 for r in by["project_recovery_k33"])
assert by["project_recovery_k35_deny"][0]["roots"] == 0 and by["project_recovery_k35_deny"][0]["questions"] == 1
k36 = by["project_recovery_k36"][0]
assert k36["virtual_days"] >= 7 and k36["evaluations"] <= 20 and k36["questions"] == 3 and k36["roots"] == 0
assert all(r["evaluations"] == 0 and r["roots"] == 0 for r in by["project_recovery_k37_stop"])
assert all(r["evaluations_after_question"] == 0 and r["questions"] == 1 for r in by["project_recovery_k37_human_required"])
assert by["project_recovery_k37_dangerous_budget"][0]["evaluations_after_stop"] == 0
assert by["project_recovery_k39"][0]["chat_starts"] == 1 and by["project_recovery_k39"][0]["questions"] == 1
assert by["project_recovery_k40_tokens"][0]["evaluations_next_day"] == 1
assert by["project_recovery_k40_roots"][0]["without_grant_resumes"] == 0
assert by["project_recovery_k40_limit"][0]["auto_resumes"] == 3
assert by["project_recovery_cos_brief_ttl"][0]["ttl_stops"] == 1
after = {r["case"]: r for r in rows(out / "human_wait_measure.jsonl")}
was = {r["case"]: r for r in rows(before)}
assert set(after) == set(was) == {"open_question_real_ts", "answered_question_fake_ts"}, (after, was)
table = {}
for case in after:
    a, b = after[case]["evaluations"], was[case]["evaluations"]
    assert a < b, f"{case}: {b} -> {a}"
    table[case] = {"before": b, "after": a, "virtual_hours": after[case]["virtual_hours"]}
(out / "summary.json").write_text(json.dumps({"schema": "celeris.long-horizon-record/1", "raw_rows": len(rec),
    "k38_evaluations_in_24h": table}, ensure_ascii=False, indent=2) + "\n")
print("test-project-recovery: OK", json.dumps(table, ensure_ascii=False))
PY
