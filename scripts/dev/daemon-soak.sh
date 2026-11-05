#!/usr/bin/env bash
# P4-F（第 4 フェーズ）: 実 daemon 全体の長時間の資源の計測を 4 条件（off・v8_load・shadow・enabled）で揃えて走らせ、集計する。
#
# 1. `celeris`・`script-deploy` を build し、control-ts の bundle を作る（--celeris で別の binary を渡すと before/after を測れる）。
# 2. `scripts/dev/daemon-soak/daemon_soak.py run` を条件ごとに別の一時 home・別 port で同時に走らせる（fake harness、LLM 0）。
# 3. `daemon_soak.py summary` で集計し、<out>/summary.{json,md} を書く。
#
# 1 時間を超える --secs は人の承認の file（--approval、`daemon_soak.py plan` の approval_template の形）が要る。
# Celeris の worker の run の中では 1 時間を超える run を起こさない（daemon_soak.py の門が exit 2 で断る）。
# 本番の DB・config・systemd に触れない。外部ネットワークに出ない。CPU を焼かない（負荷は task の投入の間隔で決める）。
#
# 使い方: bash scripts/dev/daemon-soak.sh --out <dir> --secs <N> [--label main] [--celeris <bin>] [--approval <file>]
#         [--conds off,v8_load,shadow,enabled] [--kill-at <secs>] [その他は daemon_soak.py run に渡す]
set -euo pipefail

out="" secs="" label="main" celeris="" approval="" conds="off,v8_load,shadow,enabled" extra=()
while [ $# -gt 0 ]; do
    case "$1" in
        --out) out="$2"; shift 2 ;;
        --secs) secs="$2"; shift 2 ;;
        --label) label="$2"; shift 2 ;;
        --celeris) celeris="$2"; shift 2 ;;
        --approval) approval="$2"; shift 2 ;;
        --conds) conds="$2"; shift 2 ;;
        *) extra+=("$1"); shift ;;
    esac
done
fail() { echo "daemon-soak: FAIL: $*" >&2; exit 1; }
[ -n "$out" ] && [ -n "$secs" ] || { echo "usage: $0 --out <dir> --secs <N> [...]" >&2; exit 2; }
unset CELERIS_RUN_DB CELERIS_DB CELERIS_CONFIG CELERIS_HOME CELERIS_API_TOKEN CELERIS_API_URL CELERIS_SCRIPT_BUNDLE_DIR || true
root="$(cd "$(dirname "$0")/../.." && pwd)"
mkdir -p "$out"
out="$(cd "$out" && pwd)"

(cd "$root" && cargo build -q -p celeris --bin celeris -p celeris-script --bin script-deploy) || fail "build failed"
bin="${CARGO_TARGET_DIR:-$root/target}/debug"
celeris="${celeris:-$bin/celeris}"
work="$(mktemp -d "${TMPDIR:-/tmp}/daemon-soak.XXXXXX")"
cleanup() { chmod -R u+w "${work:?}" 2>/dev/null || true; rm -rf "${work:?}"; }
trap cleanup EXIT
if [ ! -d "$root/control-ts/node_modules" ]; then
    (cd "$root/control-ts" && pnpm install --offline --frozen-lockfile >/dev/null) || fail "control-ts dependencies are missing"
fi
(cd "$root/control-ts" && SC_BUILD="$root/control-ts/scripts/build-bundle.mjs" SC_OUT="$work/bundle" \
    node --input-type=module -e 'const { buildBundle } = await import(process.env.SC_BUILD); await buildBundle({ outDir: process.env.SC_OUT });' >/dev/null) \
    || fail "control-ts bundle failed"

approval_args=()
[ -n "$approval" ] && approval_args=(--approval "$approval")
pids=() dirs=()
IFS=, read -r -a list <<<"$conds"
for cond in "${list[@]}"; do
    d="$out/$label-$cond"
    dirs+=("$d")
    python3 -I "$root/scripts/dev/daemon-soak/daemon_soak.py" run --cond "$cond" --label "$label" --out "$d" \
        --state "$work/$cond" --celeris "$celeris" --script-deploy "$bin/script-deploy" --bundle "$work/bundle" \
        --secs "$secs" "${approval_args[@]}" "${extra[@]}" >"$out/$label-$cond.log" 2>&1 &
    pids+=($!)
done
rc=0
for p in "${pids[@]}"; do wait "$p" || rc=$?; done
python3 -I "$root/scripts/dev/daemon-soak/daemon_soak.py" summary "${dirs[@]}" \
    --json "$out/summary.json" --markdown "$out/summary.md" || rc=1
exit "$rc"
