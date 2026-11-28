#!/bin/sh
# shadow-bench: long-horizon bench C01〜C10 の腕 shadow（試験 `long_horizon_shadow_`）を本物の control-ts の bundle で流す（LLM 0）。
# 使い方（repo の root から）: sh agent-docs/progress/2026-11-27-strategy-v1/shadow_bench.sh record|check
#   record: raw を agent-docs/progress/2026-11-27-strategy-v1/shadow-bench/raw/ に書き、shadow_bench.py run で shadow-bench.json を書く。
#   check:  raw を $TMPDIR に書き、shadow_bench.py compare でcommit 済みの shadow-bench.json と比べる（作業 tree を変えない。exit 0/1）。
# bundle は $TMPDIR に作る（`pnpm -C control-ts bundle` と同じ build-bundle.mjs の buildBundle を、出力先だけ変えて呼ぶ。
# pnpm の script は control-ts/dist に書くので使わない）。bundle が無ければ CELERIS_SCRIPT_REQUIRE_BUNDLE=1 で試験が落ちる。
set -eu
mode="${1:-}"
case "$mode" in
    record|check) ;;
    *) echo "usage: $0 record|check" >&2; exit 2 ;;
esac
root="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$root"
here="agent-docs/progress/2026-11-27-strategy-v1"
fail() { echo "shadow_bench: FAIL: $*" >&2; exit 1; }
# 本番の環境変数を引き継がない（scripts/dev/test-long-horizon-bench.sh と同じ）。
unset CELERIS_RUN_DB CELERIS_DB CELERIS_FOLLOWUPS_FILE CELERIS_RUN_ID CELERIS_TASK_ID CELERIS_CONFIG CELERIS_HOME \
    CELERIS_COS_RUN_CREDENTIAL CELERIS_API_PRINCIPAL CELERIS_API_TOKEN CELERIS_API_URL CELERIS_SCRIPT_BUNDLE_DIR || true
tmp="$(mktemp -d "${TMPDIR:-/tmp}/shadow-bench.XXXXXX")"
if [ ! -f control-ts/node_modules/.modules.yaml ]; then
    corepack pnpm@12.6.0 -C control-ts install --offline --frozen-lockfile >"$tmp/install.log" 2>&1 \
        || fail "control-ts dependencies are missing (see $tmp/install.log)"
fi
TEST_BUILD="$root/control-ts/scripts/build-bundle.mjs" TEST_OUT="$tmp/bundle" node --input-type=module -e '
  const { buildBundle } = await import(process.env.TEST_BUILD);
  await buildBundle({ outDir: process.env.TEST_OUT });' >"$tmp/bundle.log" 2>&1 || fail "bundle failed (see $tmp/bundle.log)"
if [ "$mode" = record ]; then
    raw="$root/$here/shadow-bench/raw"
    rm -rf "$raw"
else
    raw="$tmp/raw"
fi
mkdir -p "$raw"
set +e
LONG_HORIZON_BENCH_RAW_DIR="$raw" CELERIS_SCRIPT_BUNDLE_DIR="$tmp/bundle" CELERIS_SCRIPT_REQUIRE_BUNDLE=1 \
    cargo nextest run -p task-dispatch --lib --no-fail-fast -E 'test(/long_horizon_shadow_/)' >"$tmp/nextest.log" 2>&1
rc=$?
set -e
tail -n 1 "$tmp/nextest.log"
[ "$rc" -eq 0 ] || fail "nextest exited $rc (see $tmp/nextest.log)"
grep -qE 'Summary \[.*10 tests run: 10 passed' "$tmp/nextest.log" || fail "expected 10 passed (see $tmp/nextest.log)"
[ "$(ls "$raw/shadow" | wc -l)" -eq 10 ] || fail "expected 10 raw records in $raw/shadow"
if [ "$mode" = record ]; then
    python3 -I "$here/shadow_bench.py" run
else
    python3 -I "$here/shadow_bench.py" compare "$raw"
fi
