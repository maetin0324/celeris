#!/usr/bin/env bash
# 対照 pilot（ADR 2026-11-24-strategy-pilot-ablation D1・D3）: 本番の reviewer run の起動口で、事例の時点ごとに方式 a・b・c を流す。
#
#   bash scripts/dev/parent-eval-blind.sh --harness stub --arms a,b,c --case replay --points C-mid,D-done,F-done --out <dir>
#   bash scripts/dev/parent-eval-blind.sh --harness config --arms a,b,c --case replay --points C-mid,D-done,F-done --out <dir> --approval <人の承認の json> --llm-config <toml>
#   bash scripts/dev/parent-eval-blind.sh --harness stub|config --arms a,b,c --case research --research-src <c-results.md> --points 11.17,11.20 --out <dir> [...]
#
# 研究ケース（ADR D2.2）は --research-src の c-results.md から、子 0 の research_case.py（path で import）で時点の slice を切る
# （parent-eval-blind/research.py）。時点は 11.16・11.17・11.20、版 v1 の段は 11.15 節。`--harness config` では --research-src の
# sha256 が research_case.SHA と一致しなければ exit 2。承認の scope.cases の名は research-<時点>。本文は repo に写さない。
#
# 時点・方式ごとに一時 home（$TMPDIR か --state-root の下）に worktree の build の daemon（一時 DB・loopback）を起こし、slice から
# 使い捨ての案件・承認済み Roadmap・strategy root・子を seed する。方式 a は controller を mode shadow にし（evaluate は suppressed、
# reviewer run 0）、方式 b は mode enabled で evaluate が start_side_review で reviewer run を起こし、最終の記録が載るまで待つ
# （scripts/dev/parent-eval-blind/driver.py）。方式 c は、最初の時点より前の slice（replay は A-mid、研究ケースは 11.15 節）で版 v1 を書く段を事例ごとに
# 1 回流し（worker run と、Critic を兼ねる final review。出力は <out>/<case>/strategy-v1/）、各時点で b と同じ評価を、版 v1 を
# 参照する strategy root と作業 tree で流す。
# 出力は <out>/<case>/<point>/<arm>/ の output.json・reads.json・run.json・input/。
# - `--harness stub` は LLM を呼ばない（parent-eval-blind/stub_reviewer.py。経路の確認だけで、評価の証拠に数えない）。
# - `--harness config` は実 LLM（人の書いた --llm-config を config に足す）。Celeris の worker の run の中（CELERIS_RUN_ID が
#   ある）では引数の検査より前に断る。人の承認の記録（--approval）を pilot_gate.py check-approval が確かめる（scope の
#   cases に時点、arms に方式、environment は non-production）。
# - 本番の DB・daemon・config・release に触れない。
# exit: 0 = 全時点・全方式の記録を書いた、1 = 失敗、2 = 使い方の誤り・承認が無い・worker の run の中。
set -euo pipefail

for a in "$@"; do
    if [ "$a" = config ] && [ -n "${CELERIS_RUN_ID:-}" ]; then
        echo "parent-eval-blind: refusing to start LLM runs inside a Celeris worker run (CELERIS_RUN_ID is set)" >&2
        exit 2
    fi
done
usage() { sed -n '4,6p' "$0" >&2; exit 2; }
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
harness="" out="" arms="" case="" points="" llm_config="" approval="" state_root="" max_wall="600" max_usd="1" research_src=""
while [ $# -gt 0 ]; do
    case "$1" in
        --harness) harness="$2"; shift 2 ;;
        --out) out="$2"; shift 2 ;;
        --arms) arms="$2"; shift 2 ;;
        --case) case="$2"; shift 2 ;;
        --points) points="$2"; shift 2 ;;
        --research-src) research_src="$2"; shift 2 ;;
        --llm-config) llm_config="$2"; shift 2 ;;
        --approval) approval="$2"; shift 2 ;;
        --state-root) state_root="$2"; shift 2 ;;
        --max-wall-secs) max_wall="$2"; shift 2 ;;
        *) usage ;;
    esac
done
fail() { echo "parent-eval-blind: FAIL: $*" >&2; exit 1; }
[ -n "$out" ] && [ -n "$arms" ] && [ -n "$case" ] && [ -n "$points" ] || usage
case "$case" in
    replay) known_points="C-mid D-done F-done" strategy_point="A-mid" ;;
    research) known_points="11.16 11.17 11.20" strategy_point="11.15" ;;
    *) echo "parent-eval-blind: unknown case $case (replay, research)" >&2; exit 2 ;;
esac
scope_cases="$points"
if [ "$case" = research ]; then
    [ -f "$research_src" ] || { echo "parent-eval-blind: --case research needs --research-src <c-results.md>" >&2; exit 2; }
    research_src="$(cd "$(dirname "$research_src")" && pwd)/$(basename "$research_src")"
    scope_cases="$(printf %s "$points" | tr , '\n' | sed 's/^/research-/' | paste -sd, -)"
elif [ -n "$research_src" ]; then
    echo "parent-eval-blind: --research-src is only for --case research" >&2; exit 2
fi
for p in $(printf %s "$points" | tr , " "); do
    case " $known_points " in *" $p "*) ;; *) echo "parent-eval-blind: unknown point $p for case $case ($known_points)" >&2; exit 2 ;; esac
done
for m in $(printf %s "$arms" | tr , " "); do
    case "$m" in
        a|b|c) ;;
        *) echo "parent-eval-blind: unknown arm $m (a, b, c)" >&2; exit 2 ;;
    esac
done
case "$harness" in
    stub) ;;
    config)
        [ -f "$approval" ] || { echo "parent-eval-blind: the real LLM evaluation needs a person's approval record (--approval <json>)" >&2; exit 2; }
        check="$(python3 -I "$root/scripts/dev/long-horizon-pilot/pilot_gate.py" check-approval --approval "$approval" \
            --cases "$scope_cases" --arms "$arms" --max-wall-secs "$max_wall")" || {
            echo "parent-eval-blind: the approval does not allow this evaluation: $check" >&2; exit 2; }
        [ -f "$llm_config" ] || { echo "parent-eval-blind: --llm-config <toml> is required" >&2; exit 2; }
        if [ "$case" = research ]; then
            python3 -I "$root/scripts/dev/parent-eval-blind/research.py" check-sha --src "$research_src" || exit 2
        fi
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
[ "$case" = research ] && set -- "$@" --research-src "$research_src"
[ "$harness" = stub ] && set -- "$@" --stub "$root/scripts/dev/parent-eval-blind/stub_reviewer.py"
[ "$harness" = config ] && set -- "$@" --llm-config "$llm_config" --max-usd "$max_usd"
drive() {
    python3 -I "$root/scripts/dev/parent-eval-blind/driver.py" --case "$case" \
        --celeris "$bin/celeris" --script-deploy "$bin/script-deploy" --bundle "$work/bundle" --seed-bin "$seed_bin" \
        --shadow "$root/crates/task-api/tests/fixtures/parent_checkpoint_shadow" --max-wall-secs "$max_wall" "$@"
}
case ",$arms," in
    *,c,*)
        echo "parent-eval-blind: case $case strategy v1 (arm c)" >&2
        rm -rf "${out:?}/$case/strategy-v1"
        drive --stage strategy-v1 --point "$strategy_point" --arm c --skill "$root/config/skills/cos-operator/strategy-review.md" \
            --state "$work/$case-strategy-v1" --out "$out" "$@" >"$work/$case-strategy-v1.log" 2>&1 ||
            { tail -40 "$work/$case-strategy-v1.log" >&2; fail "case $case strategy v1 failed"; }
        ;;
esac
for p in $(printf %s "$points" | tr , " "); do
    for m in $(printf %s "$arms" | tr , " "); do
        echo "parent-eval-blind: case $case point $p arm $m" >&2
        rm -rf "${out:?}/$case/$p/$m"
        extra=()
        [ "$m" = c ] && extra=(--strategy-dir "$out/$case/strategy-v1")
        drive --point "$p" --arm "$m" "${extra[@]}" --state "$work/$case-$p-$m" --out "$out" "$@" \
            >"$work/$case-$p-$m.log" 2>&1 || { tail -40 "$work/$case-$p-$m.log" >&2; fail "case $case point $p arm $m failed"; }
    done
done
echo "parent-eval-blind: wrote $out (harness $harness)"
