#!/usr/bin/env bash
# ADR 2026-11-04-project-controller A9・long-horizon-bench D9: 非本番の長時間 pilot を実 daemon で流す（1 本の script）。
#
#   bash scripts/dev/long-horizon-pilot-llm.sh --harness stub   --out <dir> [--cases C08] [--arms new,old.passive]
#   bash scripts/dev/long-horizon-pilot-llm.sh --harness config-stub --out <dir> [--cases C06] [--arms new]
#   bash scripts/dev/long-horizon-pilot-llm.sh --harness config --approval <人の承認の json> \
#       --llm-config <toml> --cos-route '<toml の配列>' [--out <dir>] [--cases C04,C06,C08,C09,C10] [--max-wall-secs N]
#
# 試行ごとに一時 home（$TMPDIR か --state-root の下）に daemon（一時 DB・loopback の port）を起こし、使い捨ての
# `pilot:<case>-<seed>` 案件を作って、`new`（controller enabled）か `old.passive`（mode off、人の発言 1 回で従来の
# CoS が起票）を流す。終わったら DB を読み取り専用で開いて facts（dataset long_horizon_llm_pilot）・arm-map・作業 tree・
# ops（操作の記録）を書き、`celeris-campaign long-horizon-grade --tier llm_pilot` で採点・集計する。
#
# - `--harness stub` は LLM を呼ばない（scripts/dev/long-horizon-pilot/stub_harness.py）。経路の確認用で、raw を
#   v2 の実 LLM raw の dir（$llm_dir）に書かない（実 LLM の raw と混ぜない）。v1 の raw（2026-11-04-long-horizon/）の下には
#   どちらの harness でも書かない。
# - `--harness config-stub` は `--harness config` と同じ driver の経路（人の書く `--llm-config` をそのまま足す）に、
#   LLM を呼ばない agent（stub_harness.py --from-objective）の TOML を渡す。agent は実 LLM と同じ入口（task の
#   objective・acceptance）だけを読み、公開の command も objective から見つける（ADR 2026-11-05-phase3-p0d）。
# - C06 の公開は driver が worker の外で起こす broker（long-horizon-pilot/pilot_effect.py）が記録し、承認は daemon の
#   approvals API で人の token が決める。`LONG_HORIZON_PILOT_STUB_TAMPER=1`（stub だけ。陰性の試験）は stub に記録への
#   書き込みを試させ、driver が observation_failed で落ちることを確かめる。`LONG_HORIZON_PILOT_STUB_FORGE=1`（stub・
#   config-stub。陰性の試験）は worker に token を探させて承認の偽造を試させ、どの token も API に通らないことを
#   driver が確かめる。API の token は試行ごとの乱数で、home の外の私的な dir に daemon・broker の起動の間だけ置く。
# - `--harness config` は実 LLM。人が書いた `--llm-config`（[llm_sources]・[harnesses]・[route_defaults]・
#   [harness_selection]・[accounts] 等）をそのまま config に足す。人の承認の記録（--approval。approval_id・
#   approved_by human:<名前>・expires_at・scope の cases/arms/environment・limits の max_usd/max_wall_secs）が要り、
#   pilot_gate.py check-approval が期限切れ・範囲外・不備を断る（ADR 2026-11-06-phase5-n1-pilot-measurement-contract）。
#   Celeris の worker の run の中（CELERIS_RUN_ID がある）では起こさない（ADR 2026-10-07 worker-no-subagents）。
#   daemon の予算（max_usd_per_day）は承認の max_usd、--max-wall-secs の既定は承認の max_wall_secs。
#   24〜72h の実稼働はこの script の範囲外（人の別途の承認）。
# - 採点の後に pilot_gate.py judge が測定の契約（usage・cache・USD の欠測、CoS の run、追記専用、拒否・漏洩・二重作用、
#   上限）を判定し、`<out>/grade/pilot-gate.json` と pc_rollout.py 用の `<out>/grade/pilot-facts.json` を書く。
#   欠測は合格にしない（verdict unknown）。stub・config-stub の G7 は pending。
# - 本番の DB・daemon・config・release に触れない（--state-root が本番の dir なら driver が断る）。
# exit: 0 = 全試行が終わり採点できた（--harness config は gate も pass）、1 = 失敗、2 = 使い方の誤り・承認が無い、
#       3 = --harness config で gate が pass でない（欠測・上限超え・拒否など。pilot-gate.json を読む）。
set -euo pipefail

usage() { sed -n '3,6p' "$0" >&2; exit 2; }
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
harness="" out="" cases="C08" arms="new,old.passive" llm_config="" cos_route="[]" state_root="" max_wall="" approval="" total_wall="" expires_at=""
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
        --total-wall-secs) total_wall="$2"; shift 2 ;;
        --approval) approval="$2"; shift 2 ;;
        *) usage ;;
    esac
done
fail() { echo "long-horizon-pilot-llm: FAIL: $*" >&2; exit 1; }
# v1 の raw（a9/）は改変しない（ADR phase3 I6）。v2 の実 LLM raw の既定の置き場は別の dir。
llm_dir="$root/agent-docs/progress/2026-11-05-phase3/llm-v2"
v1_dir="$root/agent-docs/progress/2026-11-04-long-horizon"
case "$harness" in
    stub|config-stub)
        [ -n "$out" ] || usage
        max_wall="${max_wall:-300}"
        ;;
    config)
        if [ -n "${CELERIS_RUN_ID:-}" ]; then
            echo "long-horizon-pilot-llm: refusing to start LLM runs inside a Celeris worker run (CELERIS_RUN_ID is set)" >&2
            exit 2
        fi
        if [ ! -f "$approval" ]; then
            echo "long-horizon-pilot-llm: the real LLM pilot needs a person's approval record (--approval <json>)" >&2
            exit 2
        fi
        approval="$(cd "$(dirname "$approval")" && pwd)/$(basename "$approval")"
        check="$(python3 -I "$root/scripts/dev/long-horizon-pilot/pilot_gate.py" check-approval --approval "$approval" \
            --cases "$cases" --arms "$arms" ${max_wall:+--max-wall-secs "$max_wall"})" || {
            echo "long-horizon-pilot-llm: the approval does not allow this pilot: $check" >&2; exit 2; }
        [ -f "$llm_config" ] || { echo "long-horizon-pilot-llm: --llm-config <toml> is required" >&2; exit 2; }
        [ "$cos_route" != "[]" ] || { echo "long-horizon-pilot-llm: --cos-route must not be empty for --harness config" >&2; exit 2; }
        out="${out:-$llm_dir}"
        max_wall="${max_wall:-$(printf %s "$check" | python3 -I -c 'import json,sys; print(int(json.load(sys.stdin)["max_wall_secs"]))')}"
        # 承認の max_wall_secs は承認全体の総量（試行ごとにリセットしない。judge の wall_cap は全試行の wall の和と比べる）。
        total_wall="$(printf %s "$check" | python3 -I -c 'import json,sys; print(int(json.load(sys.stdin)["max_wall_secs"]))')"
        expires_at="$(printf %s "$check" | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["expires_at"])')"
        max_usd="$(printf %s "$check" | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["max_usd"])')"
        echo "long-horizon-pilot-llm: approval $(printf %s "$check" | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["approval_id"])') max_usd=$max_usd max_wall_secs=$max_wall" >&2
        ;;
    *) usage ;;
esac
command -v python3 >/dev/null || fail "python3 is missing"
command -v node >/dev/null || fail "node is missing"
: "${CARGO_TARGET_DIR:?CARGO_TARGET_DIR is required}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
case "$out" in "$v1_dir"*) fail "raw must not be written under the v1 raw ($v1_dir)" ;; esac
if [ "$harness" != config ]; then
    case "$out" in "$llm_dir"*) fail "stub raw must not be written to $llm_dir" ;; esac
fi
unset CELERIS_RUN_DB CELERIS_DB CELERIS_FOLLOWUPS_FILE CELERIS_TASK_ID CELERIS_CONFIG CELERIS_HOME \
    CELERIS_COS_RUN_CREDENTIAL CELERIS_API_PRINCIPAL CELERIS_API_TOKEN CELERIS_API_URL CELERIS_SCRIPT_BUNDLE_DIR || true

(cd "$root" && cargo build -q -p celeris --bin celeris -p celeris-script --bin script-deploy \
    -p celeris-campaign --bin celeris-campaign) || fail "build failed"
bin="$CARGO_TARGET_DIR/debug"
work="$(mktemp -d "${state_root:-${TMPDIR:-/tmp}}/long-horizon-pilot.XXXXXX")"
cleanup() { chmod -R u+w "${work:?}" 2>/dev/null || true; rm -rf "${work:?}"; }
trap cleanup EXIT
if [ ! -d "$root/control-ts/node_modules" ]; then
    (cd "$root/control-ts" && pnpm install --offline --frozen-lockfile >/dev/null) || fail "control-ts dependencies are missing"
fi
(cd "$root/control-ts" && SC_BUILD="$root/control-ts/scripts/build-bundle.mjs" SC_OUT="$work/bundle" \
    node --input-type=module -e 'const { buildBundle } = await import(process.env.SC_BUILD); await buildBundle({ outDir: process.env.SC_OUT });' >/dev/null) \
    || fail "control-ts bundle failed"

rm -rf "${out:?}/facts" "${out:?}/arm-map" "${out:?}/trials" "${out:?}/ops" "${out:?}/grade"
forge_arg=""
if [ "${LONG_HORIZON_PILOT_STUB_FORGE:-}" = 1 ]; then
    # 陰性の試験用（stub・config-stub だけ）: worker が token を探して承認を偽造しようとする（通れば試行は失敗）。
    [ "$harness" != config ] || fail "LONG_HORIZON_PILOT_STUB_FORGE is only for --harness stub|config-stub"
    forge_arg=', "--forge"'
fi
if [ "$harness" = config ]; then
    set -- --harness config --harness-label config --llm-config "$llm_config" --cos-route "$cos_route" \
        --cos-max-turns 40 --cos-max-wall 1800 --max-usd-per-day "$max_usd"
elif [ "$harness" = config-stub ]; then
    # 実 LLM の代わりの agent を config harness の TOML として書く（LLM は呼ばない）。
    agent="$root/scripts/dev/long-horizon-pilot/stub_harness.py"
    # 試験用の絞り（config-stub だけ）: CoS の返事を遅らせる秒数と、account の同時 run 数（worker・reviewer と CoS の重なりを作る）。
    delay_arg=""
    [ -z "${LONG_HORIZON_PILOT_STUB_COS_DELAY:-}" ] || delay_arg=", \"--cos-delay-secs\", \"$LONG_HORIZON_PILOT_STUB_COS_DELAY\""
    [ -z "${LONG_HORIZON_PILOT_STUB_WORKER_DELAY:-}" ] || delay_arg="$delay_arg, \"--worker-delay-secs\", \"$LONG_HORIZON_PILOT_STUB_WORKER_DELAY\""
    max_runs_line=""
    [ -z "${LONG_HORIZON_PILOT_STUB_MAX_RUNS:-}" ] || max_runs_line="max_runs = $LONG_HORIZON_PILOT_STUB_MAX_RUNS"
    cat >"$work/config-stub.toml" <<TOML
[adapters.fake]
command = ["python3", "-I", "$agent", "--from-objective"$delay_arg$forge_arg]

[llm_sources.local]
kind = "fake"
accounts = ["a1"]
models = ["model-1"]
$max_runs_line

[harnesses.claude_code]
kind = "fake"
api_formats = ["fake"]
source_kinds = ["fake"]

[route_defaults.claude_code]
candidates = [{ source = "local", model = "model-1" }]

# stub の usage を daemon の USD 予算が数えられるように（費用不明の run は予算を止める）。
[budget_rates."model-1"]
input_per_million = 1.0
output_per_million = 5.0
cache_read_per_million = 0.1
cache_write_per_million = 2.0

[harness_selection]
execution_class_defaults = {}
rules = [{ harness = "claude_code", required_capabilities = [] }]
TOML
    set -- --harness config --harness-label config-stub --llm-config "$work/config-stub.toml" \
        --cos-route '[{ source = "local", model = "model-1" }]'
else
    set -- --harness stub
    # 陰性の試験用: stub が効果の記録に偽の行を足そうとする（driver は observation_failed で落ちる）。
    [ "${LONG_HORIZON_PILOT_STUB_TAMPER:-}" != 1 ] || set -- "$@" --stub-tamper
    [ "${LONG_HORIZON_PILOT_STUB_NO_OUTPUT:-}" != 1 ] || set -- "$@" --stub-no-output
fi
[ "${LONG_HORIZON_PILOT_STUB_FORGE:-}" != 1 ] || set -- "$@" --forge
# 共有の期限（ADR 2026-11-06-cos-run-launch-accounting-and-drain D3）: 最初の試行の開始から数え、全試行が同じ総量を分け合う。
deadline_args=""
if [ -n "$total_wall" ]; then
    deadline_args="--approval-started-at $(date +%s.%N) --approval-wall-secs $total_wall"
    [ -z "$expires_at" ] || deadline_args="$deadline_args --approval-expires-at $expires_at"
fi
for case in $(printf %s "$cases" | tr , " "); do
    for arm in $(printf %s "$arms" | tr , " "); do
        echo "long-horizon-pilot-llm: case $case arm $arm" >&2
        python3 -I "$root/scripts/dev/long-horizon-pilot/driver.py" --state "$work/$case-$arm" --out "$out" \
            --fixtures "$root/tests/fixtures/long-horizon" --bundle "$work/bundle" --celeris "$bin/celeris" \
            --script-deploy "$bin/script-deploy" --case "$case" --arm "$arm" --max-wall-secs "$max_wall" $deadline_args "$@" \
            >"$work/$case-$arm.log" 2>&1 || { tail -40 "$work/$case-$arm.log" >&2; fail "trial $case $arm failed"; }
        cp "$work/$case-$arm.log" "$out/ops/$case-$arm.log"
    done
done
"$bin/celeris-campaign" long-horizon-grade --tier llm_pilot --raw "$out" --fixtures "$root/tests/fixtures/long-horizon" \
    --out "$out/grade" || fail "long-horizon-grade --tier llm_pilot failed"
set +e
python3 -I "$root/scripts/dev/long-horizon-pilot/pilot_gate.py" judge --raw "$out" ${approval:+--approval "$approval"} \
    --out "$out/grade/pilot-gate.json" --pilot-facts "$out/grade/pilot-facts.json" >&2
gate_rc=$?
set -e
case "$gate_rc" in 0|1|3) ;; *) fail "pilot_gate.py judge failed (exit $gate_rc)" ;; esac
if [ "$harness" = config ] && [ "$gate_rc" -ne 0 ]; then
    echo "long-horizon-pilot-llm: wrote $out (harness $harness), but the measurement gate is not pass: $out/grade/pilot-gate.json" >&2
    exit 3
fi
echo "long-horizon-pilot-llm: wrote $out (harness $harness)"
