#!/usr/bin/env bash
# ADR 2026-11-04-project-controller A9・long-horizon-bench D9: 非本番の長時間 pilot を実 daemon で流す（1 本の script）。
#
#   bash scripts/dev/long-horizon-pilot-llm.sh --harness stub   --out <dir> [--cases C08] [--arms new,old.passive]
#   LONG_HORIZON_PILOT_LLM_APPROVED=1 bash scripts/dev/long-horizon-pilot-llm.sh --harness config \
#       --llm-config <toml> --cos-route '<toml の配列>' [--out <dir>] [--cases C04,C06,C08,C09,C10] [--max-wall-secs 14400]
#
# 試行ごとに一時 home（$TMPDIR か --state-root の下）に daemon（一時 DB・loopback の port）を起こし、使い捨ての
# `pilot:<case>-<seed>` 案件を作って、`new`（controller enabled）か `old.passive`（mode off、人の発言 1 回で従来の
# CoS が起票）を流す。終わったら DB を読み取り専用で開いて facts（dataset long_horizon_llm_pilot）・arm-map・作業 tree・
# ops（操作の記録）を書き、`celeris-campaign long-horizon-grade --tier llm_pilot` で採点・集計する。
#
# - `--harness stub` は LLM を呼ばない（scripts/dev/long-horizon-pilot/stub_harness.py）。経路の確認用で、raw を
#   v2 の実 LLM raw の dir（$llm_dir）に書かない（実 LLM の raw と混ぜない）。v1 の raw（2026-11-04-long-horizon/）の下には
#   どちらの harness でも書かない。
# - `--harness config` は実 LLM。人が書いた `--llm-config`（[llm_sources]・[harnesses]・[route_defaults]・
#   [harness_selection]・[accounts] 等）をそのまま config に足す。人の承認（LONG_HORIZON_PILOT_LLM_APPROVED=1）が
#   要り、Celeris の worker の run の中（CELERIS_RUN_ID がある）では起こさない（ADR 2026-10-07 worker-no-subagents）。
#   既定の上限は 1 試行 1,000,000 token / 4 時間（D9）。24〜72h の実稼働はこの script の範囲外（人の別途の承認）。
# - 本番の DB・daemon・config・release に触れない（--state-root が本番の dir なら driver が断る）。
# exit: 0 = 全試行が終わり採点できた、1 = 失敗、2 = 使い方の誤り・承認が無い。
set -euo pipefail

usage() { sed -n '3,6p' "$0" >&2; exit 2; }
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
harness="" out="" cases="C08" arms="new,old.passive" llm_config="" cos_route="[]" state_root="" max_wall=""
while [ $# -gt 0 ]; do
    case "$1" in
        --harness) harness="$2"; shift 2 ;;
        --out) out="$2"; shift 2 ;;
        --cases) cases="$2"; shift 2 ;;
        --arms) arms="$2"; shift 2 ;;
        --llm-config) llm_config="$2"; shift 2 ;;
        --cos-route) cos_route="$2"; shift 2 ;;
        --state-root) state_root="$2"; shift 2 ;;
        --max-wall-secs) max_wall="$2"; shift 2 ;;
        *) usage ;;
    esac
done
fail() { echo "long-horizon-pilot-llm: FAIL: $*" >&2; exit 1; }
# v1 の raw（a9/）は改変しない（ADR phase3 I6）。v2 の実 LLM raw の既定の置き場は別の dir。
llm_dir="$root/agent-docs/progress/2026-11-05-phase3/llm-v2"
v1_dir="$root/agent-docs/progress/2026-11-04-long-horizon"
case "$harness" in
    stub)
        [ -n "$out" ] || usage
        max_wall="${max_wall:-300}"
        ;;
    config)
        if [ "${LONG_HORIZON_PILOT_LLM_APPROVED:-}" != 1 ]; then
            echo "long-horizon-pilot-llm: the real LLM pilot needs a person's approval (LONG_HORIZON_PILOT_LLM_APPROVED=1)" >&2
            exit 2
        fi
        if [ -n "${CELERIS_RUN_ID:-}" ]; then
            echo "long-horizon-pilot-llm: refusing to start LLM runs inside a Celeris worker run (CELERIS_RUN_ID is set)" >&2
            exit 2
        fi
        [ -f "$llm_config" ] || { echo "long-horizon-pilot-llm: --llm-config <toml> is required" >&2; exit 2; }
        out="${out:-$llm_dir}"
        max_wall="${max_wall:-14400}"
        ;;
    *) usage ;;
esac
command -v python3 >/dev/null || fail "python3 is missing"
command -v node >/dev/null || fail "node is missing"
: "${CARGO_TARGET_DIR:?CARGO_TARGET_DIR is required}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
case "$out" in "$v1_dir"*) fail "raw must not be written under the v1 raw ($v1_dir)" ;; esac
if [ "$harness" = stub ]; then
    case "$out" in "$llm_dir"*) fail "stub raw must not be written to $llm_dir" ;; esac
fi
unset CELERIS_RUN_DB CELERIS_DB CELERIS_FOLLOWUPS_FILE CELERIS_TASK_ID CELERIS_CONFIG CELERIS_HOME \
    CELERIS_COS_RUN_CREDENTIAL CELERIS_API_PRINCIPAL CELERIS_API_TOKEN CELERIS_API_URL CELERIS_SCRIPT_BUNDLE_DIR || true

(cd "$root" && cargo build -q -p celeris --bin celeris -p celeris-script --bin script-deploy \
    -p celeris-campaign --bin celeris-campaign) || fail "build failed"
bin="$CARGO_TARGET_DIR/debug"
work="$(mktemp -d "${state_root:-${TMPDIR:-/tmp}}/long-horizon-pilot.XXXXXX")"
cleanup() { rm -rf "${work:?}"; }
trap cleanup EXIT
if [ ! -d "$root/control-ts/node_modules" ]; then
    (cd "$root/control-ts" && pnpm install --offline --frozen-lockfile >/dev/null) || fail "control-ts dependencies are missing"
fi
(cd "$root/control-ts" && SC_BUILD="$root/control-ts/scripts/build-bundle.mjs" SC_OUT="$work/bundle" \
    node --input-type=module -e 'const { buildBundle } = await import(process.env.SC_BUILD); await buildBundle({ outDir: process.env.SC_OUT });' >/dev/null) \
    || fail "control-ts bundle failed"

rm -rf "${out:?}/facts" "${out:?}/arm-map" "${out:?}/trials" "${out:?}/ops" "${out:?}/grade"
if [ "$harness" = config ]; then
    extra=(--harness config --llm-config "$llm_config" --cos-route "$cos_route" --cos-max-turns 40 --cos-max-wall 1800)
else
    extra=(--harness stub)
    # 陰性の試験用: stub が承認を待たずに publish を記録する（effect_before(approval, effect) が落ちる）。
    [ "${LONG_HORIZON_PILOT_STUB_EARLY_PUBLISH:-}" != 1 ] || extra+=(--stub-early-publish)
fi
for case in ${cases//,/ }; do
    for arm in ${arms//,/ }; do
        echo "long-horizon-pilot-llm: case $case arm $arm" >&2
        python3 -I "$root/scripts/dev/long-horizon-pilot/driver.py" --state "$work/$case-$arm" --out "$out" \
            --fixtures "$root/tests/fixtures/long-horizon" --bundle "$work/bundle" --celeris "$bin/celeris" \
            --script-deploy "$bin/script-deploy" --case "$case" --arm "$arm" --max-wall-secs "$max_wall" "${extra[@]}" \
            >"$work/$case-$arm.log" 2>&1 || { tail -40 "$work/$case-$arm.log" >&2; fail "trial $case $arm failed"; }
        cp "$work/$case-$arm.log" "$out/ops/$case-$arm.log"
    done
done
"$bin/celeris-campaign" long-horizon-grade --tier llm_pilot --raw "$out" --fixtures "$root/tests/fixtures/long-horizon" \
    --out "$out/grade" || fail "long-horizon-grade --tier llm_pilot failed"
echo "long-horizon-pilot-llm: wrote $out (harness $harness)"
