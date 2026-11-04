#!/usr/bin/env bash
# ADR 2026-11-04-long-horizon-bench（A8 実行）の受け入れ検査。LLM を呼ばない（台本の CoS・worker・人、本物の TS は決定的）。
#
# 1. control-ts の bundle を作り、task-dispatch の `long_horizon_bench_` 試行（10 課題 × old.passive・old.proactive・new・
#    対照 ctl.null。仮想時刻・一時 DB。new は本物の bundle を本物の V8 で評価）を走らせ、40 件以上が通ることを確かめる。
#    各試行は同じ seed を 2 回流して facts の bytes 一致を試験の中で確かめている。
# 2. 別 process（celeris-campaign の `long_horizon_bench_`）で採点・集計し、D8 の gate（同一初期条件の digest・対照 ctl.null を
#    含む）と陰性 fixture（述語ごと）、実 LLM pilot の tier の分離を確かめる。CLI `celeris-campaign long-horizon-grade --tier replay`
#    でも同じ raw を採点し、試験の集計と bytes 一致を確かめる。
# 3. 作業 tree の検査（D4）と陽性対照、facts に腕の欄が無いこと、replay の token・quota・walltime が null、凍結の path（D10）が
#    bench の source に無いこと、本番の DB・daemon を指していないことを確かめる。
# 前提（cargo-nextest・python3・node・control-ts の依存）が欠けたら SKIP せずに失敗する。
# 使い方: bash scripts/dev/test-long-horizon-bench.sh [--out <dir>]（既定は $TMPDIR の下の新しい dir）
set -euo pipefail
out=""
while [ $# -gt 0 ]; do
    case "$1" in
        --out) out="$2"; shift 2 ;;
        *) echo "usage: $0 [--out <dir>]" >&2; exit 2 ;;
    esac
done
fail() { echo "test-long-horizon-bench: FAIL: $*" >&2; exit 1; }
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
command -v cargo >/dev/null || fail "cargo is missing"
cargo nextest --version >/dev/null 2>&1 || fail "cargo-nextest is missing"
command -v python3 >/dev/null || fail "python3 is missing"
command -v node >/dev/null || fail "node is missing"
fixtures="$root/tests/fixtures/long-horizon"
[ -f "$fixtures/oracle/MANIFEST.sha256" ] || fail "fixtures are missing"

# 本番の環境変数を引き継がない（ADR Project Controller D11.3）。
unset CELERIS_RUN_DB CELERIS_DB CELERIS_FOLLOWUPS_FILE CELERIS_RUN_ID CELERIS_TASK_ID CELERIS_CONFIG CELERIS_HOME \
    CELERIS_COS_RUN_CREDENTIAL CELERIS_API_PRINCIPAL CELERIS_API_TOKEN CELERIS_API_URL CELERIS_SCRIPT_BUNDLE_DIR || true

[ -n "$out" ] || out="$(mktemp -d "${TMPDIR:-/tmp}/long-horizon-bench.XXXXXX")"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
case "$out" in
    /local/celeris/data/db*|/local/celeris/state*|"$HOME/.local/celeris"*|"$HOME/.config/celeris"*) fail "--out must not be under the production state" ;;
esac
raw="$out/raw"; rm -rf "$raw" "$out"/*.log "$out/summary"; mkdir -p "$raw"

# 0. oracle の凍結: MANIFEST の hash が今の file と一致する（D5）。
(cd "$fixtures" && python3 -I - <<'PY') || fail "MANIFEST.sha256 does not match the fixtures"
import hashlib, pathlib, sys
lines = pathlib.Path("oracle/MANIFEST.sha256").read_text().splitlines()
want = {l.split("  ", 1)[1]: l.split("  ", 1)[0] for l in lines if "  " in l}
have = {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in pathlib.Path(".").rglob("*") if p.is_file() and p.name != "MANIFEST.sha256"}
sys.exit(0 if want == have else 1)
PY

if [ ! -f control-ts/node_modules/.modules.yaml ]; then
    (cd control-ts && pnpm install --offline --frozen-lockfile) >/dev/null 2>&1 || fail "control-ts dependencies are missing (pnpm install --offline failed)"
fi
bundle="$out/bundle"; rm -rf "$bundle"
TEST_BUILD="$root/control-ts/scripts/build-bundle.mjs" TEST_OUT="$bundle" node --input-type=module -e '
  const { buildBundle } = await import(process.env.TEST_BUILD);
  await buildBundle({ outDir: process.env.TEST_OUT });' >"$out/bundle.log" 2>&1 || fail "build-bundle.mjs failed (see $out/bundle.log)"

run_suite() { # name min args...
    local name="$1" min="$2"; shift 2
    local log="$out/$name.log" rc
    set +e
    LONG_HORIZON_BENCH_RAW_DIR="$raw" LONG_HORIZON_BENCH_SUMMARY_OUT="$out/summary" CELERIS_SCRIPT_BUNDLE_DIR="$bundle" \
        CELERIS_SCRIPT_REQUIRE_BUNDLE=1 CELERIS_SCRIPT_SKIP_LOG="$out/skip.log" cargo nextest run --no-fail-fast "$@" >"$log" 2>&1
    rc=$?
    set -e
    tail -n 2 "$log"
    [ "$rc" -eq 0 ] || fail "$name exited $rc (see $log)"
    local passed
    passed="$(grep -E 'Summary \[' "$log" | tail -n 1 | sed -nE 's/.* ([0-9]+) tests? run: ([0-9]+) passed.*/\2/p')"
    [ -n "$passed" ] || fail "$name: no nextest summary"
    [ "$passed" -ge "$min" ] || fail "$name: $passed tests passed, expected at least $min"
    echo "test-long-horizon-bench: $name: $passed passed"
}
run_suite replay 40 -p task-dispatch --lib -E 'test(/^dispatcher::tests::project_wake::long_horizon_bench::long_horizon_bench_/)'
[ ! -s "$out/skip.log" ] || fail "bundle SKIP recorded: $(cat "$out/skip.log")"
run_suite grade 17 -p celeris-campaign --test long_horizon_bench -E 'test(/^long_horizon_bench_/)'
[ -s "$out/summary/summary.json" ] || fail "summary.json was not written"
cargo run -q -p celeris-campaign --bin celeris-campaign -- long-horizon-grade --tier replay --raw "$raw" --fixtures "$fixtures" \
    --out "$out/cli" >"$out/cli.log" 2>&1 || fail "long-horizon-grade --tier replay failed (see $out/cli.log)"
cmp -s "$out/cli/grades.jsonl" "$out/summary/grades.jsonl" || fail "CLI grades.jsonl differs from the test's"
if cargo run -q -p celeris-campaign --bin celeris-campaign -- long-horizon-grade --tier llm_pilot --raw "$raw" \
    --fixtures "$fixtures" --out "$out/cli-pilot" >"$out/cli-pilot.log" 2>&1; then
    fail "long-horizon-grade --tier llm_pilot accepted replay raw"
fi

python3 -I - "$root" "$raw" "$out" <<'PY'
import json, pathlib, sys, hashlib, shutil, subprocess
root, raw, out = map(pathlib.Path, sys.argv[1:4])
fx = root / "tests/fixtures/long-horizon"
errors = []
facts = sorted((raw / "facts").glob("*.json"))
if len(facts) < 40:
    errors.append(f"{len(facts)} facts files (< 40)")
# D2 の同一初期条件: 課題ごとに全腕の init_digest が 1 つ（採点器とは別にここでも数える）。
init = {}
for f in facts:
    v = json.loads(f.read_text())
    init.setdefault(v["case"], set()).add(v.get("init_digest"))
for case, d in sorted(init.items()):
    if len(d) != 1 or None in d:
        errors.append(f"{case}: init_digest differs across arms {sorted(map(str, d))}")
canary = json.loads((fx / "oracle/holdout.json").read_text())["canary"]
hidden = set()
titles = set()
for case in sorted(p.name for p in (fx / "oracle").iterdir() if p.is_dir()):
    hidden |= set(json.loads((fx / "oracle" / case / "hidden/expected.json").read_text()).values())
    for o in json.loads((fx / "oracle" / case / "scenario.json").read_text())["outcomes"]:
        titles.add(o["title"].split("（")[-1].rstrip("）").split("-")[0])
for f in facts:
    v = json.loads(f.read_text())
    if "arm" in v:
        errors.append(f"{f.name}: facts carry the arm")
    if v.get("dataset") != "long_horizon_replay":
        errors.append(f"{f.name}: dataset {v.get('dataset')}")
    u = v.get("usage", {})
    if any(u.get(k) is not None for k in ("tokens", "quota", "wall_secs")) or u.get("missing") != "tier=replay":
        errors.append(f"{f.name}: replay usage must be null with missing=tier=replay, got {u}")

def leaks(path, needles):
    hits = []
    for p in pathlib.Path(path).rglob("*"):
        if p.is_file():
            b = p.read_bytes()
            for n in needles:
                if n.encode() in b:
                    hits.append(f"{p}: {n[:16]}")
    return hits

# D4: 試行の作業 tree と public に canary・隠し受入の hash・oracle の file が無い。
needles = [canary] + sorted(hidden)
for d in [raw / "trials", fx / "public"]:
    for h in leaks(d, needles):
        errors.append(f"leak: {h}")
for t in (raw / "trials").iterdir():
    names = {p.name for p in t.rglob("*")}
    for bad in ("oracle.json", "scenario.json", "expected.json", "holdout.json", ".git"):
        if bad in names:
            errors.append(f"trial {t.name} contains {bad}")
# 陽性対照: canary を public の写しに入れると検査が当たる（空振りでない）。
ctl = out / "positive-control"
shutil.rmtree(ctl, ignore_errors=True)
shutil.copytree(fx / "public/C01", ctl)
(ctl / "repo-seed/notes.txt").write_text("copied " + canary + "\n")
if not leaks(ctl, needles):
    errors.append("positive control: the canary planted in a public copy was not detected")
shutil.rmtree(ctl)
# 方針が課題を名指ししない: 課題の固有語が control-ts/src と crates/task-*/src に無い。
src = subprocess.run(["git", "ls-files", "control-ts/src", "crates/task-*/src"], cwd=root, capture_output=True, text=True, check=True).stdout.split()
src = [s for s in src if "long_horizon_bench" not in s]
for s in src:
    text = (root / s).read_text(errors="ignore")
    for w in titles:
        if w in text:
            errors.append(f"{s} names the case word {w}")
# D10: bench の source・script・fixture が凍結の path を参照しない。
frozen = ["cases/final", "grade-cases/final", "grader-private/final", "frozen-manifests", "/var/lib/celeris-eval", "/local/celeris/eval-", "EVAL_ROOT"]
bench = [root / "crates/task-dispatch/src/dispatcher/tests/long_horizon_bench.rs", root / "crates/campaign/src/long_horizon.rs",
         root / "crates/campaign/tests/long_horizon_bench.rs"] + [p for p in fx.rglob("*") if p.is_file()]
for p in bench:
    text = p.read_text(errors="ignore")
    for w in frozen:
        if w in text:
            errors.append(f"{p}: references frozen path {w}")
s = json.loads((out / "summary/summary.json").read_text())
if s["invalid"]["invalid_init"] != 0:
    errors.append(f"summary: invalid_init {s['invalid']}")
for arm in ("old.passive", "old.proactive", "new", "ctl.null"):
    if s["arms"].get(arm, {}).get("trials") != 10:
        errors.append(f"summary: arm {arm} has {s['arms'].get(arm, {}).get('trials')} trials")
if errors:
    for e in errors:
        print("test-long-horizon-bench: FAIL:", e, file=sys.stderr)
    sys.exit(1)
a = s["arms"]
print("test-long-horizon-bench: summary " + "; ".join(
    f"{k}: reach {v['reach_rate']:.2f} autonomous {v['autonomous_reach']:.2f} avoidable {v['human_intervention']['avoidable']} "
    f"wasteful {v['wasteful_roots']} double {v['double_effect']} recovery {v['recovery_rate']} "
    f"unprompted {v['reached_without_instruction']}" for k, v in sorted(a.items())))
PY
echo "test-long-horizon-bench: OK (raw and summary in $out)"
