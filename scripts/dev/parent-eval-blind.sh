#!/usr/bin/env bash
# ADR 2026-11-22-parent-checkpoint-evaluator D8.3: 本番の reviewer run の起動口で E-mid・D-done を盲検で評価する（1 本の script）。
#
#   bash scripts/dev/parent-eval-blind.sh --harness stub --out <dir> [--points E-mid,D-done]
#   bash scripts/dev/parent-eval-blind.sh --harness config --approval <人の承認の json> --llm-config <toml> --out <dir> [--points E-mid,D-done]
#
# 時点ごとに一時 home（$TMPDIR か --state-root の下）に worktree の build の daemon（一時 DB・loopback）を起こし、slice から
# 使い捨ての案件・strategy root・子を seed して controller を enabled にする。controller の evaluate が start_side_review で
# reviewer run を起こし、最終の記録が載るまで待つ（scripts/dev/parent-eval-blind/driver.py）。出力は <out>/<point>/ の
# evaluation.json・reads.json・run.json。比べるのは scripts/dev/parent-eval-blind-compare.py。
# - `--harness stub` は LLM を呼ばない（parent-eval-blind/stub_reviewer.py。経路の確認だけで、評価の証拠に数えない）。
# - `--harness config` は実 LLM（人の書いた --llm-config を config に足す）。Celeris の worker の run の中（CELERIS_RUN_ID が
#   ある）では引数の検査より前に断る。人の承認の記録（--approval）を pilot_gate.py check-approval が確かめる（scope の
#   cases に時点、arms に parent-eval-blind、environment は non-production）。run は時点の数で、4 以下。
# - 本番の DB・daemon・config・release に触れない。
# exit: 0 = 全時点の記録を書いた、1 = 失敗、2 = 使い方の誤り・承認が無い・worker の run の中。
set -euo pipefail

for a in "$@"; do
    if [ "$a" = config ] && [ -n "${CELERIS_RUN_ID:-}" ]; then
        echo "parent-eval-blind: refusing to start LLM runs inside a Celeris worker run (CELERIS_RUN_ID is set)" >&2
        exit 2
    fi
done
usage() { sed -n '4,5p' "$0" >&2; exit 2; }
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
harness="" out="" points="E-mid,D-done" llm_config="" approval="" state_root="" max_wall="600" max_usd="1"
while [ $# -gt 0 ]; do
    case "$1" in
        --harness) harness="$2"; shift 2 ;;
        --out) out="$2"; shift 2 ;;
        --points) points="$2"; shift 2 ;;
        --llm-config) llm_config="$2"; shift 2 ;;
        --approval) approval="$2"; shift 2 ;;
        --state-root) state_root="$2"; shift 2 ;;
        --max-wall-secs) max_wall="$2"; shift 2 ;;
        *) usage ;;
    esac
done
fail() { echo "parent-eval-blind: FAIL: $*" >&2; exit 1; }
[ -n "$out" ] || usage
n=0
for p in $(printf %s "$points" | tr , " "); do
    case "$p" in E-mid|D-done) n=$((n + 1)) ;; *) echo "parent-eval-blind: unknown point $p (E-mid, D-done)" >&2; exit 2 ;; esac
done
[ "$n" -ge 1 ] && [ "$n" -le 4 ] || { echo "parent-eval-blind: 1 to 4 points" >&2; exit 2; }
case "$harness" in
    stub) ;;
    config)
        [ -f "$approval" ] || { echo "parent-eval-blind: the real LLM evaluation needs a person's approval record (--approval <json>)" >&2; exit 2; }
        check="$(python3 -I "$root/scripts/dev/long-horizon-pilot/pilot_gate.py" check-approval --approval "$approval" \
            --cases "$points" --arms parent-eval-blind --max-wall-secs "$max_wall")" || {
            echo "parent-eval-blind: the approval does not allow this evaluation: $check" >&2; exit 2; }
        [ -f "$llm_config" ] || { echo "parent-eval-blind: --llm-config <toml> is required" >&2; exit 2; }
        max_usd="$(printf %s "$check" | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["max_usd"])')"
        ;;
    *) usage ;;
esac
command -v python3 >/dev/null || fail "python3 is missing"
command -v node >/dev/null || fail "node is missing"
: "${CARGO_TARGET_DIR:?CARGO_TARGET_DIR is required}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
unset CELERIS_RUN_DB CELERIS_DB CELERIS_FOLLOWUPS_FILE CELERIS_TASK_ID CELERIS_CONFIG CELERIS_HOME \
    CELERIS_COS_RUN_CREDENTIAL CELERIS_API_PRINCIPAL CELERIS_API_TOKEN CELERIS_API_URL CELERIS_SCRIPT_BUNDLE_DIR || true

(cd "$root" && cargo build -q -p celeris --bin celeris -p celeris-script --bin script-deploy) || fail "build failed"
seed_bin="$(cd "$root" && cargo test -q -p task-api --test parent_checkpoint_live --no-run --message-format=json 2>/dev/null \
    | python3 -I -c 'import json,sys
for l in sys.stdin:
    m = json.loads(l)
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["target"]["name"] == "parent_checkpoint_live":
        print(m["executable"])')" || fail "seed build failed"
[ -x "$seed_bin" ] || fail "seed binary not found"
bin="$CARGO_TARGET_DIR/debug"
work="$(mktemp -d "${state_root:-${TMPDIR:-/tmp}}/parent-eval-blind.XXXXXX")"
cleanup() { chmod -R u+w "${work:?}" 2>/dev/null || true; rm -rf "${work:?}"; }
trap cleanup EXIT
if [ ! -d "$root/control-ts/node_modules" ]; then
    (cd "$root/control-ts" && pnpm install --offline --frozen-lockfile >/dev/null) || fail "control-ts dependencies are missing"
fi
(cd "$root/control-ts" && SC_BUILD="$root/control-ts/scripts/build-bundle.mjs" SC_OUT="$work/bundle" \
    node --input-type=module -e 'const { buildBundle } = await import(process.env.SC_BUILD); await buildBundle({ outDir: process.env.SC_OUT });' >/dev/null) \
    || fail "control-ts bundle failed"
set -- --harness "$harness"
[ "$harness" = stub ] && set -- "$@" --stub "$root/scripts/dev/parent-eval-blind/stub_reviewer.py"
[ "$harness" = config ] && set -- "$@" --llm-config "$llm_config" --max-usd "$max_usd"
for p in $(printf %s "$points" | tr , " "); do
    echo "parent-eval-blind: point $p" >&2
    rm -rf "${out:?}/$p"
    python3 -I "$root/scripts/dev/parent-eval-blind/driver.py" --point "$p" --state "$work/$p" --out "$out" \
        --celeris "$bin/celeris" --script-deploy "$bin/script-deploy" --bundle "$work/bundle" --seed-bin "$seed_bin" \
        --shadow "$root/crates/task-api/tests/fixtures/parent_checkpoint_shadow" --max-wall-secs "$max_wall" "$@" \
        >"$work/$p.log" 2>&1 || { tail -40 "$work/$p.log" >&2; fail "point $p failed"; }
done
echo "parent-eval-blind: wrote $out (harness $harness)"
