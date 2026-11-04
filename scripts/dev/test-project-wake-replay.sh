#!/usr/bin/env bash
# ADR 2026-11-04-project-controller A1: 案件の durable wake の受け入れ試験。
#
# 1. task-dispatch の `project_wake_` 試験（再配送・kind 9 種・crash B1〜B6・待機・coalesce・CAS・lease・
#    backoff・backpressure・mode）を nextest で走らせ、27 件以上が通ることを確かめる（0 件・不足は失敗）。
# 2. task-api の `project_wake_api_` 試験（API・CoS op・人の操作の human_epoch）と task-core の
#    `project_wake_store_` 試験を走らせる（それぞれ 1 件以上）。
# 3. 試験が書いた raw（celeris.long-horizon-record/1 の JSONL）を検査する:
#    - redelivery: 10 回の再配送で root 1・wake 1・冪等キー 1・未処理 0・LLM 起動 0
#    - crash: B1〜B6 の 6 境界それぞれで root 1・未処理の wake 0・pending の効果 0・CoS 入力 ≤ 1・LLM 起動 0
#    - idle: 仮想 7 日（wake なし）で decide 0・LLM 起動 0・chat message 0、cron 168 回で decide 168・LLM 起動 0
#
# LLM を呼ばない（fake decider・fake harness・注入した時計）。本番の DB・daemon に触れない（試験は一時 DB だけを
# 開く）。前提（cargo-nextest・python3）が欠けたら SKIP せずに失敗する。
#
# 使い方: bash scripts/dev/test-project-wake-replay.sh [--out <dir>]（既定は $TMPDIR の下の新しい dir）
set -euo pipefail

out=""
while [ $# -gt 0 ]; do
    case "$1" in
        --out) out="$2"; shift 2 ;;
        *) echo "usage: $0 [--out <dir>]" >&2; exit 2 ;;
    esac
done

fail() { echo "test-project-wake-replay: FAIL: $*" >&2; exit 1; }

command -v cargo >/dev/null || fail "cargo is missing"
cargo nextest --version >/dev/null 2>&1 || fail "cargo-nextest is missing"
command -v python3 >/dev/null || fail "python3 is missing"

# 本番の環境変数を引き継がない（ADR D11.3）。
unset CELERIS_RUN_DB CELERIS_DB CELERIS_FOLLOWUPS_FILE CELERIS_RUN_ID CELERIS_TASK_ID \
    CELERIS_COS_RUN_CREDENTIAL CELERIS_API_PRINCIPAL CELERIS_CONFIG || true

if [ -z "$out" ]; then
    out="$(mktemp -d "${TMPDIR:-/tmp}/project-wake-replay.XXXXXX")"
fi
mkdir -p "$out"
case "$(cd "$out" && pwd)" in
    /local/celeris/data/db*|"${CELERIS_STATE_DIR:-/nonexistent-state-dir}"*) fail "--out must not be under the production state" ;;
esac
rm -f "$out"/redelivery.jsonl "$out"/crash.jsonl "$out"/idle.jsonl
export PROJECT_WAKE_RAW_DIR="$out"

run_suite() {
    # $1 package, $2 filter, $3 minimum count, $4 log name
    local log="$out/$4.log"
    set +e
    cargo nextest run -p "$1" --no-fail-fast -E "$2" >"$log" 2>&1
    local rc=$?
    set -e
    tail -n 3 "$log"
    [ "$rc" -eq 0 ] || fail "$1 $2 exited $rc (see $log)"
    local summary
    summary="$(grep -E 'Summary \[' "$log" | tail -n 1)"
    local passed
    passed="$(printf '%s\n' "$summary" | sed -nE 's/.* ([0-9]+) tests? run: ([0-9]+) passed.*/\2/p')"
    [ -n "$passed" ] || fail "$1: no nextest summary"
    printf '%s\n' "$summary" | grep -qE 'failed' && fail "$1: failures in $summary"
    [ "$passed" -ge "$3" ] || fail "$1 $2: $passed tests passed, expected at least $3"
    echo "test-project-wake-replay: $1 $2: $passed passed"
}

run_suite task-dispatch 'test(/^dispatcher::tests::project_wake::project_wake_/)' 27 dispatch
run_suite task-api 'test(/project_wake_api_/)' 28 api
run_suite task-core 'test(/project_wake_store_/)' 2 store

python3 - "$out" <<'PY'
import json, sys, pathlib
out = pathlib.Path(sys.argv[1])
errors = []
def load(name):
    p = out / name
    if not p.exists():
        errors.append(f"{name} is missing (raw records were not written)")
        return []
    rows = [json.loads(l) for l in p.read_text().splitlines() if l.strip()]
    for r in rows:
        if r.get("schema") != "celeris.long-horizon-record/1":
            errors.append(f"{name}: bad schema {r.get('schema')}")
    return rows
def expect(row, key, want, op="=="):
    got = row.get(key)
    ok = {"==": got == want, "<=": got is not None and got <= want}[op]
    if not ok:
        errors.append(f"{row.get('test')}: {key}={got!r}, expected {op} {want!r}")

red = load("redelivery.jsonl")
if len(red) != 1:
    errors.append(f"redelivery: {len(red)} records, expected 1")
for r in red:
    expect(r, "deliveries", 10)
    for k, v in [("roots", 1), ("wakes", 1), ("idempotency_keys", 1), ("decides", 1),
                 ("unprocessed_wakes", 0), ("pending_effects", 0), ("llm_launches", 0)]:
        expect(r, k, v)

crash = load("crash.jsonl")
names = sorted(r.get("test") for r in crash)
want = [f"project_wake_crash_b{i}" for i in range(1, 7)]
if names != want:
    errors.append(f"crash: records {names}, expected {want}")
for r in crash:
    for k, v in [("roots", 1), ("unprocessed_wakes", 0), ("pending_effects", 0),
                 ("idempotency_keys", 1), ("llm_launches", 0)]:
        expect(r, k, v)
    expect(r, "cos_inputs", 1, "<=")

idle = {r.get("test"): r for r in load("idle.jsonl")}
for name, decides in [("project_wake_idle_7days", 0), ("project_wake_cron_idle", 168)]:
    r = idle.get(name)
    if r is None:
        errors.append(f"idle: {name} record is missing")
        continue
    expect(r, "virtual_hours", 168)
    expect(r, "decides", decides)
    expect(r, "llm_launches", 0)
    expect(r, "chat_messages", 0)
    expect(r, "unprocessed_wakes", 0)

if errors:
    for e in errors:
        print("test-project-wake-replay: FAIL:", e, file=sys.stderr)
    sys.exit(1)
print(f"test-project-wake-replay: raw ok ({len(red)} redelivery, {len(crash)} crash, {len(idle)} idle) in {out}")
PY
echo "test-project-wake-replay: OK"
