#!/usr/bin/env bash
# P1-I（第 3 フェーズ）: 長時間の運用の資源の計測を 4 条件（off・v8_load・shadow・enabled）で揃えて走らせ、集計する。
#
# 1. control-ts の bundle を $TMPDIR に作る（CELERIS_SCRIPT_BUNDLE_DIR があればそれを使う）。
# 2. `crates/celeris/tests/long_run_soak.rs` の試験 binary を作り、4 条件を別々の process で同時に走らせる
#    （RSS を条件ごとに分けるため。nextest の 10 分の打ち切りを避けて binary を直接呼ぶ）。
#    計測の窓の中では restart しない。窓の後に run を走らせたまま持ち主の process を SIGKILL し、リース回収を測る。
# 3. `scripts/dev/long_run_soak_summary.py` で soak.jsonl を集計し、summary.json と summary.md を書く。
#
# LLM を呼ばない（fake harness の sh）。外部ネットワークに出ない。本番の DB・daemon に触れない（一時 DB）。
# CPU を焼かない（dispatch の間を --pace-ms で空ける）。実時間の 24/72 時間の実稼働はこの script の範囲外（人の承認が要る）。
#
# 使い方: bash scripts/dev/long-run-soak.sh --out <dir> [--secs 1560] [--tasks 20] [--pace-ms 250] [--seed-events 200000]
#   --secs を 0 にすると LONG_RUN_SOAK_CYCLES（既定 3）回で終わる。
set -euo pipefail

out="" secs=1560 tasks=20 pace=250 seed=200000
while [ $# -gt 0 ]; do
    case "$1" in
        --out) out="$2"; shift 2 ;;
        --secs) secs="$2"; shift 2 ;;
        --tasks) tasks="$2"; shift 2 ;;
        --pace-ms) pace="$2"; shift 2 ;;
        --seed-events) seed="$2"; shift 2 ;;
        *) echo "usage: $0 --out <dir> [--secs N] [--tasks N] [--pace-ms N] [--seed-events N]" >&2; exit 2 ;;
    esac
done
[ -n "$out" ] || { echo "long-run-soak: --out is required" >&2; exit 2; }
fail() { echo "long-run-soak: FAIL: $*" >&2; exit 1; }

unset CELERIS_RUN_DB CELERIS_DB CELERIS_CONFIG CELERIS_HOME CELERIS_API_TOKEN CELERIS_API_URL || true
repo="$(cd "$(dirname "$0")/../.." && pwd)"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
rm -f "${out:?}/soak.jsonl"

bundle="${CELERIS_SCRIPT_BUNDLE_DIR:-}"
if [ -z "$bundle" ]; then
    bundle="$(mktemp -d "${TMPDIR:-/tmp}/long-run-soak-bundle.XXXXXX")"
    (cd "$repo/control-ts" && node --input-type=module -e \
        "import {buildBundle} from './scripts/build-bundle.mjs'; await buildBundle({outDir: process.argv[1]});" "$bundle") \
        || fail "cannot build the control-ts bundle"
fi

bin="$(cd "$repo" && cargo test -p celeris --test long_run_soak --no-run --message-format=json 2>/dev/null \
    | python3 -c 'import json,sys
for l in sys.stdin:
    m=json.loads(l)
    if m.get("reason")=="compiler-artifact" and m.get("executable") and m["target"]["name"]=="long_run_soak":
        print(m["executable"])' | tail -1)"
[ -x "$bin" ] || fail "test binary not found"

export CELERIS_SCRIPT_REQUIRE_BUNDLE=1 CELERIS_SCRIPT_BUNDLE_DIR="$bundle" LONG_RUN_SOAK_RAW_DIR="$out" \
    LONG_RUN_SOAK_TASKS="$tasks" LONG_RUN_SOAK_PACE_MS="$pace" LONG_RUN_SOAK_SEED_EVENTS="$seed"
if [ "$secs" != 0 ]; then export LONG_RUN_SOAK_SECS="$secs"; fi
pids=()
for cond in off v8_load shadow enabled; do
    (cd "$repo/crates/celeris" && "$bin" --exact "long_run_soak_$cond" --nocapture >"$out/$cond.log" 2>&1) &
    pids+=($!)
done
rc=0
for p in "${pids[@]}"; do wait "$p" || rc=1; done
for cond in off v8_load shadow enabled; do
    grep -q "test result: ok. 1 passed" "$out/$cond.log" || { echo "long-run-soak: $cond failed (see $out/$cond.log)" >&2; rc=1; }
done
python3 -I "$repo/scripts/dev/long_run_soak_summary.py" "$out/soak.jsonl" \
    --json "$out/summary.json" --markdown "$out/summary.md" >/dev/null || rc=1
cat "$out/summary.md"
exit "$rc"
