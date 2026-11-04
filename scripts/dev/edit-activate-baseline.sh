#!/usr/bin/env bash
# edit→検査→test→反映（activate）の baseline を計る（task 01M6DCPKPJZZ9F9J4QJ47FH8KK、S0 (2)）。
#
#   bash scripts/dev/edit-activate-baseline.sh --scenario <policy|cos|policy-ts|cos-ts|gate-ts|cos-digest> --mode <warm|cold> --reps <N> --out <file.jsonl> [--rep-start <N>] [--series <name>]
#   bash scripts/dev/edit-activate-baseline.sh --dry-run --scenario <policy|cos|policy-ts|cos-ts|gate-ts|cos-digest>   build せず手順を出して exit 0
#   bash scripts/dev/edit-activate-baseline.sh --validate <file.jsonl>             記録の欄を検査（欠けがあれば exit 1）
#   bash scripts/dev/edit-activate-baseline.sh --check-targets <file.jsonl>        policy-ts・cos-ts を目標と照合、cos-digest は集計（外れたら exit 1）
#
# 1 試行 = 代表的な編集を 1 件当てる → 段（steps）を順に計る → 編集を戻す。編集は試行ごとに違う nonce を入れるので
# sccache・cargo の fingerprint が前の試行の成果を使い回さない。編集は trap（EXIT/INT/TERM/HUP）で必ず戻し、戻した後に
# 元の内容と cmp で一致を確かめる（一致しなければ exit 3）。
#   policy: crates/task-core/src/model_policy.rs の RULES に、実際には当たらない規則を 1 件足す（ADR-0069 D3 の規則表）
#   cos:    crates/task-api/src/cos/basis/job_wait_extend.rs の validate に、実際には当たらない制御（CoS 定型判断の
#           数十行の分岐。集計 → match → 拒否）を足す。編集中に test-affected.sh --list（基準 HEAD = この編集だけ）を
#           kind=affected の行に残す
#   cos-digest（S6 の Rust の経路。ADR 2026-11-02-ts-control-layer-boundary 付記 S5-10）: crates/task-core/src/chat/digest.rs の
#           cos_digest_decide に「reason が nonce 入りの印を含む run には command を出さない」分岐を足す（当たらない）。
#           対象 crate は task-core と task-dispatch（-p を 2 つ。task-core の script_fixtures.rs が fixture と Rust の出力の
#           一致を、task-dispatch の cos_digest_* 試験が digest の pass と Rust/TS の比較を見る。cos-ts の検証責任に揃える）
#   cos-ts（S6 の TS の経路）: control-ts/src/domains/cos_digest.ts の decide に同じ論理の分岐を足し、policy-ts と同じく
#           ts-verify.sh を 1 回走らせる（段 activate が新 isolate への load と各領域の probe）
#   gate-ts（P2-F の領域の TS の経路）: control-ts/src/domains/execution_gate.ts の工程語の表の直後に印の comment を足す
#           （挙動は変わらない。golden に合うまま）。ts-verify.sh を 1 回走らせる
#
# S6 の欄（付記 S5-10・S5-11。全 scenario の新しい行に付く。古い記録に無くても --validate は通す）:
#   env.identity  commit・clean（開始時の git status --porcelain が空）・tree（対象 dir の tree hash）・rustc・node・pnpm・
#                 host・target_kind（warm-shared / cold-disposable / none）・series（--series、既定 unspecified）。
#                 TS の試行は bundle_sha256・host_api（ts-verify の activate の出力）・runner_sha256 も
#   trial_load    試行の前後の 1 分 load average と、自分の target 以外の cargo・rustc の数。busy = どれかが 0 でない
#   trial_secs・over_300s  1 試行の秒（Rust は rep の段の和）と 300 秒を超えた印（捨てずに集計に入れる）
# --rep-start N: 試行の rep を N から数える（前後半に分けた測定の後半は 11）。prime は常に rep 0。
#
# 段（1 行 1 段の JSONL）:
#   check         cargo check -p <対象 crate>
#   build         cargo build -p celeris（daemon binary。反映に要る）
#   clippy        cargo clippy -p <対象 crate> -- -D warnings
#   test-build    cargo nextest run -p <対象 crate> --no-run
#   test-run      cargo nextest run -p <対象 crate>（build 済みの実行だけ）
#   release-build cargo build --release -p celeris（release gate の cargo-build 段と同じ profile）
# 既定に入らない段（BASELINE_STEPS で指定）:
#   affected-test   CELERIS_WU_BASE=HEAD の test-affected.sh（編集だけを基準に、逆依存 crate の試験 build + 実行 + web）
#   affected-rerun  同じものをもう 1 回（build 済みなので差が test build の分）
# compile と link は、linker を時間を記録する wrapper（中身は host の linker のまま）に差し替えて分ける
# （-Z も nightly も使わない）。link_secs は段の間に走った link の秒の和（並列なら壁時計を超える）、
# link_union_secs は link が 1 つでも走っていた壁時計、compile_secs = secs - link_union_secs。
# 反映（activate）は本番で promote できないので計らない。release 記録（$RELEASES_DIR/*/gate.json、読むだけ）の
# 段の秒を "estimate" 行として別に出す（--mode の計測と混ぜない。kind 欄で分ける）。
#
# warm: 渡された CARGO_TARGET_DIR を使う。最初に編集なしで全段を 1 回回して（rep 0、kind=prime）温める。
# cold: $TMPDIR の下の使い捨て target を試行ごとに作り、終わったら消す。/local の空きが 60GB 未満なら cold を
#       飛ばし、理由を kind=skipped の行に残す。sccache は host 設定のまま（cold でも cache hit はありうるので数を記録）。
# CPU を焼く負荷はかけない。本番の daemon・DB・release 置き場には書かない（release 記録は読むだけ）。
#
# policy-ts（TS だけの変更の経路。ADR 2026-11-02-ts-fast-verify (7)）: control-ts/src/domains/lane_rules.ts の RULES に
# 当たらない規則を 1 件足し、CELERIS_WU_BASE=HEAD の scripts/dev/ts-verify.sh --record を 1 回走らせて戻す。1 試行 = kind=trial の 1 行で、
#   secs              編集を書いた時点から ts-verify（最後の段 activate）が終わるまでの壁時計。編集を戻す時間は入らない
#   stages            ts-verify の段（classify・runner-cache・install・typecheck・bundle・unit・golden・safety・integration・activate）の秒と exit
#   typecheck_secs・bundle_secs  型検査と bundle の秒（別の欄）
#   cargo_invocations ts-verify の PATH の wrapper が数えた cargo の起動回数
#   link_count        linker の wrapper が数えた link の回数
# prime は kind=prime の行で、step=runner-ensure（script-runner-cache.sh ensure。秒は runner_build_secs にも書く）と、
# warm だけ step=ts-verify（編集を当てて 1 回。編集なしでは ts-verify が分類 none で抜けて温まらないため）。集計に入れない。
# warm: control-ts/node_modules と runner の cache が在る状態。 cold: 試行ごとに編集の前に node_modules を消し、
#   ts-verify の段 install（pnpm install --offline --frozen-lockfile）から測る。runner の cache は当たる状態で測る。
#   node_modules が無いと import グラフを計算できないので、cold の単体試験は常に全部になる。
# 開始時に基準 HEAD の分類が none（作業ツリーに文書以外の変更が無い）でなければ exit 2。空でない --out も exit 2（試行を混ぜない）。
# --check-targets: warm は p50 <= 30 秒かつ p95 <= 60 秒、cold は p95 <= 120 秒（最近傍順位。n=20 なら 10 番目と 19 番目）。
#   全試行で exit 0・class ts-only・10 段すべて exit 0・cargo 起動 0・link_count 0・編集を戻せた、のときだけ exit 0。
#   ファイルに在る scenario・mode をすべて判定する（cos-ts も同じ目標。cos-digest は上限なしの集計）。試行が 1 件も無ければ exit 1。読めなければ exit 2。
#
# 環境変数: BASELINE_STEPS（既定: 上の 6 段を空白区切り）、BASELINE_JOBS（cargo -j。既定は cargo の既定）、
#           RELEASES_DIR（既定 /local/celeris/state/releases）、BASELINE_COLD_MIN_GB（既定 60）。
set -euo pipefail

SCHEMA="celeris.edit-activate-baseline/1"
ALL_STEPS="check build clippy test-build test-run release-build"
root="$(git rev-parse --show-toplevel)"
cd "$root"

usage() { sed -n "2,7p" "$0" >&2; exit 2; }

scenario="" mode="" reps="" out="" dry=0 validate="" check_targets="" rep_start=1 series="unspecified"
while [ $# -gt 0 ]; do
  case "$1" in
    --scenario) scenario="${2:-}"; shift 2 ;;
    --mode) mode="${2:-}"; shift 2 ;;
    --reps) reps="${2:-}"; shift 2 ;;
    --out) out="${2:-}"; shift 2 ;;
    --rep-start) rep_start="${2:-}"; shift 2 ;;
    --series) series="${2:-}"; [ -n "$series" ] || usage; shift 2 ;;
    --dry-run) dry=1; shift ;;
    --validate) validate="${2:-}"; shift 2 ;;
    --check-targets) check_targets="${2:-}"; [ -n "$check_targets" ] || usage; shift 2 ;;
    *) usage ;;
  esac
done

# ---- --validate ----------------------------------------------------------------
if [ -n "$validate" ]; then
  exec python3 -I - "$validate" "$SCHEMA" <<'PY'
import json, sys
path, schema = sys.argv[1], sys.argv[2]
common = ["schema", "kind", "scenario", "mode", "rep", "at", "env"]
env_keys = ["head_subject", "merge_base_subject", "merge_base_expr", "rustc", "cargo", "cpu_model",
            "cpus", "loadavg", "cargo_rustc_procs", "sccache", "target_dir", "target_free_gb",
            "local_free_gb", "cargo_incremental", "profile_dev_debug"]
need = {
    "step": ["step", "cmd", "secs", "exit", "compile_secs", "link_secs", "link_count", "edit", "reverted"],
    "prime": ["step", "cmd", "secs", "exit", "compile_secs", "link_secs", "link_count"],
    "skipped": ["reason"],
    "affected": ["cmd", "lines", "edit"],
    "estimate": ["source", "release", "step", "secs"],
    "trial": ["cmd", "secs", "exit", "class", "stages", "typecheck_secs", "bundle_secs",
              "cargo_invocations", "link_count", "edit", "reverted"],
}
S6_SCENARIOS = ("cos-ts", "gate-ts", "cos-digest")
ID_KEYS = ["commit", "clean", "tree", "rustc", "node", "pnpm", "host", "target_kind", "series"]
TS_ID_KEYS = ["bundle_sha256", "host_api", "runner_sha256"]
bad, n, kinds = [], 0, {}
for i, line in enumerate(open(path, encoding="utf-8"), 1):
    if not line.strip():
        continue
    n += 1
    try:
        r = json.loads(line)
    except ValueError as e:
        bad.append(f"line {i}: not JSON ({e})"); continue
    k = r.get("kind")
    kinds[k] = kinds.get(k, 0) + 1
    if r.get("schema") != schema:
        bad.append(f"line {i}: schema != {schema}")
    keys = common + need.get(k, [])
    if k not in need:
        bad.append(f"line {i}: unknown kind {k!r}")
    for key in keys:
        if key not in r:
            bad.append(f"line {i}: missing {key}")
    if k in ("step", "prime", "skipped", "trial"):
        env = r.get("env") or {}
        for key in env_keys:
            if key not in env:
                bad.append(f"line {i}: missing env.{key}")
        sc = env.get("sccache") or {}
        for key in ("present", "reachable", "hits_delta", "misses_delta"):
            if key not in sc:
                bad.append(f"line {i}: missing env.sccache.{key}")
    if k in ("step", "trial"):
        if not isinstance(r.get("secs"), (int, float)) or not isinstance(r.get("exit"), int):
            bad.append(f"line {i}: secs/exit types")
        if r.get("reverted") is not True:
            bad.append(f"line {i}: edit was not confirmed reverted")
    if k == "trial":
        stages = r.get("stages")
        if not isinstance(stages, list) or not all(
                isinstance(st, dict) and isinstance(st.get("name"), str)
                and isinstance(st.get("secs"), (int, float)) and isinstance(st.get("exit"), int)
                for st in stages):
            bad.append(f"line {i}: stages must be a list of {{name, secs, exit}}")
    if k == "prime" and r.get("step") == "runner-ensure" and "runner_build_secs" not in r:
        bad.append(f"line {i}: missing runner_build_secs")
    # S6 の欄（付記 S5-10・S5-11）: cos-ts・cos-digest の行と、identity を持つ新しい行で必須
    env = r.get("env") if isinstance(r.get("env"), dict) else {}
    if k in ("step", "prime", "trial") and (r.get("scenario") in S6_SCENARIOS or "identity" in env):
        ident = env.get("identity")
        if not isinstance(ident, dict):
            bad.append(f"line {i}: missing env.identity")
            ident = {}
        for key in ID_KEYS:
            if key not in ident:
                bad.append(f"line {i}: missing env.identity.{key}")
        if not isinstance(ident.get("commit"), str) or not ident.get("commit"):
            bad.append(f"line {i}: env.identity.commit must be a non-empty string")
        if not isinstance(ident.get("clean"), bool):
            bad.append(f"line {i}: env.identity.clean must be a bool")
        if not isinstance(ident.get("tree"), dict) or not ident.get("tree"):
            bad.append(f"line {i}: env.identity.tree must be a non-empty object")
        if ident.get("target_kind") not in ("warm-shared", "cold-disposable", "none"):
            bad.append(f"line {i}: env.identity.target_kind {ident.get('target_kind')!r}")
        if k == "trial":
            for key in TS_ID_KEYS:
                if key not in ident:
                    bad.append(f"line {i}: missing env.identity.{key}")
            if r.get("exit") == 0 and not (isinstance(ident.get("bundle_sha256"), str) and ident.get("bundle_sha256")):
                bad.append(f"line {i}: a passed TS trial needs env.identity.bundle_sha256")
        if k in ("step", "trial"):
            for key in ("trial_secs", "over_300s", "trial_load"):
                if key not in r:
                    bad.append(f"line {i}: missing {key}")
            if not isinstance(r.get("over_300s"), bool):
                bad.append(f"line {i}: over_300s must be a bool")
            tl = r.get("trial_load")
            if not isinstance(tl, dict) or not all(
                    isinstance(tl.get(side), dict) and isinstance(tl[side].get("loadavg1"), (int, float))
                    and isinstance(tl[side].get("other_cargo_rustc"), int) for side in ("before", "after")) \
                    or not isinstance(tl.get("busy"), bool):
                bad.append(f"line {i}: trial_load must have before/after {{loadavg1, other_cargo_rustc}} and busy")
if n == 0:
    bad.append("no records")
for b in bad:
    print(b, file=sys.stderr)
print(json.dumps({"file": path, "records": n, "kinds": kinds, "problems": len(bad)}, ensure_ascii=False))
sys.exit(1 if bad else 0)
PY
fi

# ---- --check-targets -----------------------------------------------------------
# TS の経路（policy-ts・cos-ts）は目標と照合する。Rust の経路（cos-digest）は目標が無いので、rep ごとに段の秒を足して
# p50・p95 を出し、6 段がそろって exit 0・編集を戻せた、を全試行で確かめる。どちらも identity（tree・toolchain・host・
# target の種別）が違う行を同じ集計に入れない（混ざっていたら落ちる）。
if [ -n "$check_targets" ]; then
  exec python3 -I - "$check_targets" "$SCHEMA" "$ALL_STEPS" <<'PY'
import json, math, sys
path, schema, rust_steps = sys.argv[1], sys.argv[2], sys.argv[3].split()
STAGES = ["classify", "runner-cache", "install", "typecheck", "bundle", "unit", "golden",
          "safety", "integration", "activate"]
TS_SCENARIOS = ("policy-ts", "cos-ts", "gate-ts")
RUST_SCENARIOS = ("cos-digest",)
# (順位の百分位, 上限の秒)。Rust の経路は上限なし（集計だけ）
TARGETS = {"warm": [(50, 30.0), (95, 60.0)], "cold": [(95, 120.0)]}
def nearest_rank(values, p):  # 昇順で ceil(p*n/100) 番目。整数で計算して丸めの誤差を入れない
    ordered = sorted(values)
    return ordered[-(-p * len(ordered) // 100) - 1]
def is_secs(v):
    return isinstance(v, (int, float)) and not isinstance(v, bool) and math.isfinite(v) and v >= 0
def is_zero(v):
    return isinstance(v, int) and not isinstance(v, bool) and v == 0
def signature(r):
    env = r.get("env")
    ident = env.get("identity") if isinstance(env, dict) else None
    if not isinstance(ident, dict):
        return None
    return json.dumps([ident.get(k) for k in ("tree", "rustc", "node", "pnpm", "host", "target_kind")], sort_keys=True)
def count_flags(r, fl):
    fl["over_300s"] += r.get("over_300s") is True
    tl = r.get("trial_load")
    fl["busy"] += isinstance(tl, dict) and tl.get("busy") is True
try:
    lines = open(path, encoding="utf-8").read().splitlines()
except (OSError, UnicodeError) as e:
    print(f"cannot read {path}: {e}", file=sys.stderr)
    sys.exit(2)
problems, secs, reps, sigs, flags = [], {}, set(), {}, {}
rust = {}  # (scenario, mode, rep) -> [(line, row)]
for i, line in enumerate(lines, 1):
    if not line.strip():
        continue
    try:
        r = json.loads(line)
    except ValueError as e:
        problems.append(f"line {i}: not JSON ({e})")
        continue
    if not isinstance(r, dict):
        problems.append(f"line {i}: not an object")
        continue
    if r.get("schema") != schema:
        problems.append(f"line {i}: schema != {schema}")
    kind, scen = r.get("kind"), r.get("scenario")
    if kind == "prime":
        continue  # 温める行は集計に入れない
    if scen in RUST_SCENARIOS and kind in ("estimate", "skipped"):
        continue  # 反映の推定・飛ばした試行は集計に入れない
    if scen in RUST_SCENARIOS and kind == "step":
        if r.get("mode") not in TARGETS:
            problems.append(f"line {i}: unknown mode {r.get('mode')!r}")
            continue
        rust.setdefault((scen, r.get("mode"), r.get("rep")), []).append((i, r))
        continue
    if kind != "trial" or scen not in TS_SCENARIOS:
        problems.append(f"line {i}: not a trial of {'/'.join(TS_SCENARIOS + RUST_SCENARIOS)} (kind={kind!r}, scenario={scen!r})")
        continue
    mode, rep = r.get("mode"), r.get("rep")
    tag = f"line {i} ({scen} {mode} rep {rep})"
    if mode not in TARGETS:
        problems.append(f"{tag}: unknown mode")
        continue
    if (scen, mode, rep) in reps:
        problems.append(f"{tag}: duplicate rep")
    reps.add((scen, mode, rep))
    sigs.setdefault((scen, mode), set()).add(signature(r))
    count_flags(r, flags.setdefault((scen, mode), {"over_300s": 0, "busy": 0}))
    if not is_secs(r.get("secs")):
        problems.append(f"{tag}: secs is not a number")
    else:
        secs.setdefault((scen, mode), []).append(r["secs"])
    if not is_zero(r.get("exit")):
        problems.append(f"{tag}: exit {r.get('exit')!r}")
    if r.get("class") != "ts-only":
        problems.append(f"{tag}: class {r.get('class')!r} != ts-only")
    if not is_zero(r.get("cargo_invocations")):
        problems.append(f"{tag}: cargo_invocations {r.get('cargo_invocations')!r} != 0")
    if not is_zero(r.get("link_count")):
        problems.append(f"{tag}: link_count {r.get('link_count')!r} != 0")
    if r.get("reverted") is not True:
        problems.append(f"{tag}: edit was not confirmed reverted")
    stages = r.get("stages")
    stages = stages if isinstance(stages, list) else []
    names = [st.get("name") if isinstance(st, dict) else None for st in stages]
    if names != STAGES:
        problems.append(f"{tag}: stages {names} != {STAGES}")
    elif not all(is_zero(st.get("exit")) for st in stages):
        problems.append(f"{tag}: a stage failed")
for (scen, mode, rep), rows in sorted(rust.items(), key=lambda kv: (kv[0][0], kv[0][1], str(kv[0][2]))):
    tag = f"{scen} {mode} rep {rep}"
    names = [r.get("step") for _, r in rows]
    if names != rust_steps:
        problems.append(f"{tag}: steps {names} != {rust_steps}")
    bad_secs = [i for i, r in rows if not is_secs(r.get("secs"))]
    if bad_secs:
        problems.append(f"{tag}: secs is not a number (line {bad_secs[0]})")
    else:
        secs.setdefault((scen, mode), []).append(round(sum(r["secs"] for _, r in rows), 3))
    if not all(is_zero(r.get("exit")) for _, r in rows):
        problems.append(f"{tag}: a step failed")
    if not all(r.get("reverted") is True for _, r in rows):
        problems.append(f"{tag}: edit was not confirmed reverted")
    for _, r in rows:
        sigs.setdefault((scen, mode), set()).add(signature(r))
    count_flags(rows[0][1], flags.setdefault((scen, mode), {"over_300s": 0, "busy": 0}))
report = {"file": path, "scenarios": {}}
for (scen, mode), values in sorted(secs.items()):
    entry = {"trials": len(values), **flags.get((scen, mode), {})}
    if scen in TS_SCENARIOS:
        for p, limit in TARGETS[mode]:
            v = nearest_rank(values, p)
            entry[f"p{p}"] = v
            entry[f"p{p}_limit"] = limit
            if v > limit:
                problems.append(f"{scen} {mode}: p{p} {v} > {limit}")
    else:
        entry["p50"], entry["p95"] = nearest_rank(values, 50), nearest_rank(values, 95)
    if len(sigs.get((scen, mode), set())) > 1:
        problems.append(f"{scen} {mode}: rows of different identity (tree・toolchain・host・target) are mixed")
    report["scenarios"].setdefault(scen, {})[mode] = entry
if len(report["scenarios"]) == 1:
    report["modes"] = next(iter(report["scenarios"].values()))
if not secs:
    problems.append("no trials")
report["problems"] = len(problems)
for b in problems:
    print(b, file=sys.stderr)
print(json.dumps(report, ensure_ascii=False))
sys.exit(1 if problems else 0)
PY
fi

# is_ts: TS の経路（ts-verify.sh を計る）。tree_dirs: identity の tree hash を取る dir
case "$scenario" in
  policy-ts) crate=""; is_ts=1; tree_dirs="control-ts"; edit_file="control-ts/src/domains/lane_rules.ts" ;;
  cos-ts) crate=""; is_ts=1; tree_dirs="control-ts"; edit_file="control-ts/src/domains/cos_digest.ts" ;;
  gate-ts) crate=""; is_ts=1; tree_dirs="control-ts"; edit_file="control-ts/src/domains/execution_gate.ts" ;;
  policy) crate="task-core"; is_ts=0; tree_dirs="crates/task-core"; edit_file="crates/task-core/src/model_policy.rs" ;;
  cos) crate="task-api"; is_ts=0; tree_dirs="crates/task-api"; edit_file="crates/task-api/src/cos/basis/job_wait_extend.rs" ;;
  cos-digest) crate="task-core task-dispatch"; is_ts=0; tree_dirs="crates/task-core crates/task-dispatch"
    edit_file="crates/task-core/src/chat/digest.rs" ;;
  *) usage ;;
esac
[[ "$rep_start" =~ ^[1-9][0-9]*$ ]] || usage
pflags=""
for c in $crate; do pflags="${pflags:+$pflags }-p $c"; done
steps="${BASELINE_STEPS:-$ALL_STEPS}"
jobs_arg=()
[ -z "${BASELINE_JOBS:-}" ] || jobs_arg=(-j "$BASELINE_JOBS")

step_cmd() { # step → command line（表示用と実行用で同じ）
  case "$1" in
    check) echo "cargo check ${jobs_arg[*]:-} $pflags" ;;
    build) echo "cargo build ${jobs_arg[*]:-} -p celeris" ;;
    clippy) echo "cargo clippy ${jobs_arg[*]:-} $pflags -- -D warnings" ;;
    test-build) echo "cargo nextest run ${jobs_arg[*]:-} $pflags --no-run" ;;
    test-run) echo "cargo nextest run ${jobs_arg[*]:-} $pflags --no-fail-fast" ;;
    release-build) echo "cargo build ${jobs_arg[*]:-} --release -p celeris" ;;
    affected-test|affected-rerun) echo "env CELERIS_WU_BASE=HEAD bash scripts/dev/test-affected.sh" ;;
    *) echo "unknown step $1" >&2; return 2 ;;
  esac
}

# 編集を当てる（python で anchor の直前に挿入。anchor が無ければ失敗）。$1 = nonce
apply_edit() {
  python3 -I - "$edit_file" "$scenario" "$1" <<'PY'
import sys
path, scenario, nonce = sys.argv[1], sys.argv[2], sys.argv[3]
src = open(path, encoding="utf-8").read()
if scenario == "policy":
    anchor = '    LaneRule {\n        id: "standard/default",\n'
    add = ('    LaneRule {\n'
           f'        id: "frontier/edit-activate-baseline-{nonce}",\n'
           '        lane: Tier::Frontier,\n'
           f'        condition: "baseline probe {nonce} (never matches)",\n'
           '        when: |_| false,\n'
           '    },\n')
elif scenario == "policy-ts":
    anchor = '  {\n    id: "standard/default",\n'
    add = ('  {\n'
           f'    id: "frontier/edit-activate-baseline-{nonce}",\n'
           '    lane: "frontier",\n'
           f'    condition: "baseline probe {nonce} (never matches)",\n'
           '    when: () => false,\n'
           '  },\n')
elif scenario == "cos-ts":
    # cos-digest と同じ論理: reason が nonce 入りの印を含む run には command を出さない（当たらない分岐）
    anchor = '  const { text, cards } = digestMessage(input.run_id, input.run_state, input.reason, input.items);\n'
    add = (f'  // edit-activate-baseline probe {nonce}: a branch that is never taken\n'
           f'  if (input.reason !== null && input.reason.includes("edit-activate-baseline-{nonce}")) {{\n'
           '    return { commands: [] };\n'
           '  }\n')
elif scenario == "gate-ts":
    # 工程語の表の直後に印を足す（P2-F の領域。S1 の表を編集する変更と同じ段を通す。挙動は変えない）
    anchor = '];\n\nconst INPUT_FIELDS'
    add = f'  // edit-activate-baseline probe {nonce}\n'
elif scenario == "cos-digest":
    anchor = '    let (text, cards) = cos_digest_message(\n'
    add = (f'    // edit-activate-baseline probe {nonce}: a branch that is never taken\n'
           '    if input\n'
           '        .reason\n'
           '        .as_deref()\n'
           f'        .is_some_and(|r| r.contains("edit-activate-baseline-{nonce}"))\n'
           '    {\n'
           '        return Vec::new();\n'
           '    }\n')
else:
    anchor = '    // D3: the limit on extensions of the same jobs.\n'
    # 数十行の制御: job の状態を集計して、順に判定して拒否する（CoS 定型判断の条件を足す変更の形）。実際には当たらない。
    add = (f'    // edit-activate-baseline probe {nonce}: a block of control that is never taken\n'
           '    // (no task has this many jobs, and a queued job counts as neither of the others).\n'
           '    let mut probe_queued = 0usize;\n'
           '    let mut probe_active = 0usize;\n'
           '    let mut probe_ended = 0usize;\n'
           '    for status in &current.jobs {\n'
           '        match status.state {\n'
           '            ClusterJobState::Queued => probe_queued += 1,\n'
           '            ClusterJobState::Held | ClusterJobState::Running | ClusterJobState::Exiting => {\n'
           '                probe_active += 1\n'
           '            }\n'
           '            _ => probe_ended += 1,\n'
           '        }\n'
           '    }\n'
           f'    let probe_limit = usize::MAX - {int(nonce, 16) % 1000 + 1};\n'
           '    let probe_reason = if probe_queued > probe_limit {\n'
           '        Some("too many queued jobs")\n'
           '    } else if probe_active > 0 {\n'
           '        Some("a job left the queue after the check above")\n'
           '    } else if probe_ended > 0 {\n'
           '        Some("a job ended after the check above")\n'
           '    } else {\n'
           '        None\n'
           '    };\n'
           '    if let Some(reason) = probe_reason {\n'
           '        return Err(problem(\n'
           '            BASIS_JOB_NOT_QUEUED,\n'
           '            format!(\n'
           f'                "baseline probe {nonce}: {{reason}} ({{probe_queued}} queued, \\\n'
           '                 {probe_active} active, {probe_ended} ended); escalate it"\n'
           '            ),\n'
           '        ));\n'
           '    }\n')
if src.count(anchor) != 1:
    sys.exit(f"anchor not found exactly once in {path}")
open(path, "w", encoding="utf-8").write(src.replace(anchor, add + anchor))
PY
}

if [ "$dry" -eq 1 ] && [ "$is_ts" -eq 1 ]; then
  echo "scenario: $scenario（編集: $edit_file、cargo を起動しない TS の経路。ADR 2026-11-02-ts-fast-verify (7)）"
  if [ "$scenario" = cos-ts ]; then
    echo "   cos-ts: decide に「reason が edit-activate-baseline-<nonce> を含む run には command を出さない」分岐（当たらない）を足す"
    echo "   Rust の対は cos-digest（crates/task-core/src/chat/digest.rs の cos_digest_decide に同じ論理）。付記 S5-10"
  fi
  echo "0. 基準 HEAD の ts-verify.sh --list が class: none でなければ exit 2（文書以外の変更が在ると ts-only にならない）"
  echo "1. prime（kind=prime、集計に入れない）: bash scripts/dev/script-runner-cache.sh ensure（秒を runner_build_secs に記録）"
  echo "   warm は続けて編集を当てた ts-verify を 1 回走らせて温める"
  echo "2. 試行ごと（rep は --rep-start=$rep_start から。cold は最初に control-ts/node_modules を消す。消す時間は測らない）:"
  echo "   試行の前に trial_load.before（1 分 load average・自分以外の cargo/rustc の数）を取り、編集を当てる（nonce は試行ごとに違う）"
  echo "3. env CELERIS_WU_BASE=HEAD bash scripts/dev/ts-verify.sh --record <file> の段を計る:"
  for s in classify runner-cache install typecheck bundle unit golden safety integration activate; do echo "   - $s"; done
  echo "   cold の install は pnpm install --offline --frozen-lockfile。warm は node_modules が在るので何もしない"
  echo "   activate は候補 bundle を一時 state dir に配備して新しい isolate に load し、selftest と各領域の probe を呼ぶ"
  echo "4. 編集を戻し cmp で一致を確認。1 試行 1 行（kind=trial）: secs・stages・typecheck_secs・bundle_secs・cargo_invocations・link_count・class・env"
  echo "   trial_load（before・after・busy）、trial_secs・over_300s、env.identity（commit・clean・tree=HEAD:control-ts・rustc・node・pnpm・host・"
  echo "   target_kind・series=$series・bundle_sha256・host_api・runner_sha256）"
  echo "5. 判定: --validate <file>、--check-targets <file>（warm p50 <= 30 秒・p95 <= 60 秒、cold p95 <= 120 秒、全試行 cargo 0・link 0）"
  exit 0
fi
if [ "$dry" -eq 1 ]; then
  echo "scenario: $scenario（編集: $edit_file、対象 crate: $crate）"
  echo "1. 元の $edit_file を \$TMPDIR に写し、trap で必ず戻す（戻した後に cmp で一致を確認）"
  echo "2. 試行ごとに nonce を変えた編集を当てる（policy: RULES に当たらない規則 1 件 / cos: validate に当たらない数十行の分岐 /"
  echo "   cos-digest: cos_digest_decide に「reason が edit-activate-baseline-<nonce> を含む run には command を出さない」分岐）"
  echo "3. linker を時間記録 wrapper に差し替え（CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER）、段を順に計る:"
  for s in $steps; do echo "   - $s: $(step_cmd "$s")"; done
  echo "4. warm: CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-<unset>} を編集なしで 1 回温める（kind=prime）"
  echo "   cold: \$TMPDIR/edit-activate-cold-<rep> の使い捨て target（/local の空き < ${BASELINE_COLD_MIN_GB:-60}GB なら kind=skipped）"
  echo "   rep は --rep-start=$rep_start から数える（prime は rep 0）"
  echo "5. 反映は計らず、${RELEASES_DIR:-/local/celeris/state/releases}/*/gate.json を読んで kind=estimate の行を出す（読むだけ）"
  echo "6. 1 行 1 段の JSONL に env（commit 件名・merge-base・rustc・CPU・loadavg・cargo/rustc 数・sccache・空き容量）を書く"
  echo "   env.identity（commit・clean・tree=$tree_dirs の tree hash・rustc・node・pnpm・host・target_kind・series=$series）、"
  echo "   各段の行に trial_load（試行の前後の load・自分以外の cargo/rustc の数・busy）、trial_secs（rep の段の和）・over_300s"
  exit 0
fi

case "$mode" in warm|cold) ;; *) usage ;; esac
[[ "$reps" =~ ^[1-9][0-9]*$ ]] || usage
[ -n "$out" ] || usage
mkdir -p "$(dirname "$out")"
out="$(cd "$(dirname "$out")" && pwd)/$(basename "$out")"
if [ "$is_ts" -eq 1 ] || [ "$scenario" = cos-digest ]; then
  if [ -s "$out" ]; then
    echo "edit-activate-baseline: $out が空でない（前の試行と混ぜない。別の file を指定する）" >&2
    exit 2
  fi
fi
# identity.clean（付記 S5-11 の 2）: 編集を当てる前・記録を書く前の git status --porcelain が空か
clean_at_start=false
[ -n "$(git status --porcelain)" ] || clean_at_start=true
if [ "$is_ts" -eq 1 ]; then
  class_list="$(CELERIS_WU_BASE=HEAD bash scripts/dev/ts-verify.sh --list || true)"
  case "$class_list" in
    "class: none"*) ;;
    *)
      echo "edit-activate-baseline: 作業ツリーに文書以外の変更が在る。commit してから測る" >&2
      echo "$class_list" >&2
      exit 2
      ;;
  esac
fi

work="$(mktemp -d "${TMPDIR:?TMPDIR is required}/edit-activate.XXXXXX")"
backup="$work/original"
cp -p "$edit_file" "$backup"
cold_target=""
revert() {
  if ! cmp -s "$backup" "$edit_file"; then cp -p "$backup" "$edit_file"; fi
  cmp -s "$backup" "$edit_file"
}
cleanup() {
  local rc=$?
  revert || { echo "edit-activate-baseline: failed to restore $edit_file" >&2; rc=3; }
  [ -z "$cold_target" ] || rm -rf "$cold_target"
  rm -rf "$work"
  exit "$rc"
}
trap cleanup EXIT
trap 'exit 130' INT TERM HUP

# linker wrapper: host の linker（~/.cargo/config.toml の target.<triple>.linker、無ければ cc）を呼び、時間を記録する。
host_linker="$(cargo config get --format json-value target.x86_64-unknown-linux-gnu.linker -Z unstable-options 2>/dev/null | tr -d '"' || true)"
[ -n "$host_linker" ] || host_linker="$(sed -n '/^\[target.x86_64-unknown-linux-gnu\]/,/^\[/s/^linker *= *"\(.*\)"/\1/p' "$HOME/.cargo/config.toml" 2>/dev/null | head -1)"
[ -n "$host_linker" ] || host_linker=cc
linklog="$work/link.log"
cat >"$work/timed-linker" <<EOF
#!/bin/sh
s=\$(date +%s.%N)
"$host_linker" "\$@"
rc=\$?
e=\$(date +%s.%N)
echo "\$s \$e \$rc" >>"$linklog"
exit \$rc
EOF
chmod +x "$work/timed-linker"
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER="$work/timed-linker"

sccache_reachable() { (exec 3<>/dev/tcp/127.0.0.1/"${SCCACHE_SERVER_PORT:-4226}") 2>/dev/null; }
sccache_counts() { # "hits misses"（server に届かなければ "null null"。server を自分で起こさない）
  if command -v sccache >/dev/null 2>&1 && sccache_reachable; then
    sccache --show-stats --stats-format json 2>/dev/null | python3 -I -c '
import json,sys
s=json.load(sys.stdin)["stats"]
h=sum(v for v in s.get("cache_hits",{}).get("counts",{}).values())
m=sum(v for v in s.get("cache_misses",{}).get("counts",{}).values())
print(h,m)' 2>/dev/null || echo "null null"
  else
    echo "null null"
  fi
}

# ---- S6 の識別（付記 S5-10）と試行の前後の load（付記 S5-11 の 3）----------------------------------------
if [ "$is_ts" -eq 1 ]; then target_kind=none; elif [ "$mode" = cold ]; then target_kind=cold-disposable; else target_kind=warm-shared; fi
python3 -I - "$clean_at_start" "$target_kind" "$series" $tree_dirs >"$work/identity.json" <<'PY'
import json, socket, subprocess, sys
clean, target_kind, series, *dirs = sys.argv[1:]
def run(*a):
    try:
        p = subprocess.run(a, capture_output=True, text=True, timeout=60)
    except (OSError, subprocess.TimeoutExpired):
        return None
    return p.stdout.strip() or None if p.returncode == 0 else None
print(json.dumps({
    "commit": run("git", "rev-parse", "HEAD"), "clean": clean == "true",
    "tree": {d: run("git", "rev-parse", f"HEAD:{d}") for d in dirs},
    "rustc": run("rustc", "-V"), "node": run("node", "--version"), "pnpm": run("pnpm", "--version"),
    "host": socket.gethostname(), "target_kind": target_kind, "series": series,
}, ensure_ascii=False))
PY
export EAB_IDENTITY="$work/identity.json"
runner_sha256=null
load_probe() { # $1 自分の target dir → {"loadavg1", "other_cargo_rustc"}（CARGO_TARGET_DIR が $1 の process は自分のもの）
  python3 -I - "$1" <<'PY'
import json, os, sys
tdir = sys.argv[1]
other = 0
for pid in os.listdir("/proc"):
    if not pid.isdigit():
        continue
    try:
        if open(f"/proc/{pid}/comm").read().strip() not in ("cargo", "rustc"):
            continue
        env = open(f"/proc/{pid}/environ", "rb").read().split(b"\0")
    except OSError:
        continue
    if not tdir or f"CARGO_TARGET_DIR={tdir}".encode() not in env:
        other += 1
print(json.dumps({"loadavg1": float(open("/proc/loadavg").read().split()[0]), "other_cargo_rustc": other}))
PY
}

base_expr='${CELERIS_WU_BASE:-$(git merge-base HEAD main)}'
merge_base="${CELERIS_WU_BASE:-$(git merge-base HEAD main 2>/dev/null || true)}"
env_json() { # $1 target dir, $2 hits0 $3 miss0 $4 hits1 $5 miss1
  python3 -I - "$1" "$2" "$3" "$4" "$5" "$merge_base" "$base_expr" <<'PY'
import json, os, subprocess, sys, shutil
tdir, h0, m0, h1, m1, mb, mbexpr = sys.argv[1:8]
def run(*a):
    try:
        return subprocess.run(a, capture_output=True, text=True).stdout.strip()
    except OSError:
        return None
def free_gb(p):
    while p and not os.path.exists(p):
        p = os.path.dirname(p)
    return round(shutil.disk_usage(p).free / 2**30, 1) if p else None
cpu = next((l.split(":", 1)[1].strip() for l in open("/proc/cpuinfo") if l.startswith("model name")), None)
procs = 0
for pid in os.listdir("/proc"):
    if pid.isdigit():
        try:
            if open(f"/proc/{pid}/comm").read().strip() in ("cargo", "rustc"):
                procs += 1
        except OSError:
            pass
num = lambda v: None if v == "null" else int(v)
hd = None if num(h0) is None or num(h1) is None else num(h1) - num(h0)
md = None if num(m0) is None or num(m1) is None else num(m1) - num(m0)
print(json.dumps({
    "head_subject": run("git", "log", "-1", "--format=%s"),
    "merge_base_subject": run("git", "log", "-1", "--format=%s", mb) if mb else None,
    "merge_base_expr": mbexpr,
    "rustc": run("rustc", "-V"), "cargo": run("cargo", "-V"),
    "cpu_model": cpu, "cpus": os.cpu_count(),
    "loadavg": [float(x) for x in open("/proc/loadavg").read().split()[:3]],
    "cargo_rustc_procs": procs,
    "sccache": {"present": shutil.which("sccache") is not None, "reachable": num(h1) is not None,
                "hits_delta": hd, "misses_delta": md},
    "target_dir": tdir, "target_free_gb": free_gb(tdir), "local_free_gb": free_gb("/local"),
    "cargo_incremental": os.environ.get("CARGO_INCREMENTAL"),
    "profile_dev_debug": os.environ.get("CARGO_PROFILE_DEV_DEBUG"),
    **({"identity": json.load(open(os.environ["EAB_IDENTITY"]))} if os.environ.get("EAB_IDENTITY") else {}),
}, ensure_ascii=False))
PY
}

emit() { echo "$1" >>"$out"; echo "$1" | cut -c1-240 >&2; }

# 段の間 5 秒ごとに、この計測以外の cargo/rustc（CARGO_TARGET_DIR が $1 と違う process）の数と 1 分 loadavg の最大を $2 に書く
sample_load() {
  exec python3 -I - "$1" "$2" <<'PY'
import json, os, sys, time
tdir, path = sys.argv[1:3]
n, other_max, load_max = 0, 0, 0.0
parent = os.getppid()
while os.getppid() == parent:  # 計測が kill -9 されても残らない
    other = 0
    for pid in os.listdir("/proc"):
        if not pid.isdigit():
            continue
        try:
            if open(f"/proc/{pid}/comm").read().strip() not in ("cargo", "rustc"):
                continue
            env = open(f"/proc/{pid}/environ", "rb").read().split(b"\0")
        except OSError:
            continue
        if f"CARGO_TARGET_DIR={tdir}".encode() not in env:
            other += 1
    n += 1
    other_max = max(other_max, other)
    load_max = max(load_max, float(open("/proc/loadavg").read().split()[0]))
    with open(path + ".tmp", "w") as f:
        json.dump({"samples": n, "other_cargo_rustc_max": other_max, "loadavg1_max": load_max}, f)
    os.replace(path + ".tmp", path)
    time.sleep(5)
PY
}
now() { date -u +%Y-%m-%dT%H:%M:%SZ; }

# $1 kind $2 rep $3 step $4 target $5 edit(nonce or "")
run_step() {
  local kind="$1" rep="$2" step="$3" tdir="$4" nonce="$5" cmd s e rc envj link sampler during
  cmd="$(step_cmd "$step")"
  : >"$linklog"
  read -r h0 m0 < <(sccache_counts)
  envj_pre_load="$(cut -d' ' -f1-3 /proc/loadavg)"
  during="$work/during.json"
  rm -f "$during"
  sample_load "$tdir" "$during" </dev/null &
  sampler=$!
  s=$(date +%s.%N)
  set +e
  CARGO_TARGET_DIR="$tdir" $cmd >"$work/step.log" 2>&1
  rc=$?
  set -e
  e=$(date +%s.%N)
  kill "$sampler" 2>/dev/null || true
  wait "$sampler" 2>/dev/null || true
  read -r h1 m1 < <(sccache_counts)
  [ "$rc" -eq 0 ] || tail -n 20 "$work/step.log" >&2
  envj="$(env_json "$tdir" "$h0" "$m0" "$h1" "$m1")"
  # link は並列に走る（test binary が多い段で和が壁時計を超える）。和と、区間の和集合（壁時計に占める分）の両方を出す
  link="$(sort -n "$linklog" | awk 'BEGIN{t=0;n=0;u=0;end=0} {t+=$2-$1;n++; b=($1>end)?$1:end; if($2>b){u+=$2-b;end=$2}} END{printf "%.3f %d %.3f", t, n, u}')"
  python3 -I - "$SCHEMA" "$kind" "$scenario" "$mode" "$rep" "$step" "$cmd" "$s" "$e" "$rc" "$link" "$nonce" "$envj" "$(now)" "$envj_pre_load" "$edit_file" "$during" <<'PY' >"$work/rec.json"
import json, sys
(schema, kind, scen, mode, rep, step, cmd, s, e, rc, link, nonce, envj, at, preload, efile, during) = sys.argv[1:18]
secs = round(float(e) - float(s), 3)
lsecs, lcount, lunion = link.split()
rec = {"schema": schema, "kind": kind, "scenario": scen, "mode": mode, "rep": int(rep), "at": at,
       "step": step, "cmd": cmd, "secs": secs, "exit": int(rc),
       "link_secs": float(lsecs), "link_count": int(lcount), "link_union_secs": float(lunion),
       "compile_secs": round(secs - float(lunion), 3)}
if kind == "step":
    rec["edit"] = {"file": efile, "nonce": nonce}
    rec["reverted"] = None  # 試行の終わりに埋める
env = json.loads(envj)
env["loadavg_before"] = [float(x) for x in preload.split()]
try:
    env["during"] = json.load(open(during))
except (OSError, ValueError):
    env["during"] = None
rec["env"] = env
print(json.dumps(rec, ensure_ascii=False))
PY
  if [ "$kind" = prime ]; then emit "$(cat "$work/rec.json")"; else cat "$work/rec.json" >>"$work/pending"; fi
}

flush_trial() { # $1 試行の前の load_probe。編集を戻せたか・試行の秒（段の和）・前後の load を各行に書いてから out に移す
  local ok=false after
  after="$(load_probe "$tdir")"
  if revert; then ok=true; fi
  python3 -I -c '
import json,sys
ok = sys.argv[2] == "true"
rows = [json.loads(l) for l in open(sys.argv[1])]
total = round(sum(r["secs"] for r in rows), 3)
before, after = json.loads(sys.argv[3]), json.loads(sys.argv[4])
during = max([((r.get("env") or {}).get("during") or {}).get("other_cargo_rustc_max", 0) for r in rows] or [0])
load = {"before": before, "after": after, "during_other_cargo_rustc_max": during,
        "busy": before["other_cargo_rustc"] > 0 or after["other_cargo_rustc"] > 0 or during > 0}
for r in rows:
    r["reverted"] = ok; r["trial_secs"] = total; r["over_300s"] = total > 300; r["trial_load"] = load
    print(json.dumps(r, ensure_ascii=False))' "$work/pending" "$ok" "$1" "$after" >"$work/pending.done"
  while IFS= read -r l; do emit "$l"; done <"$work/pending.done"
  rm -f "$work/pending" "$work/pending.done"
  [ "$ok" = true ] || { echo "edit-activate-baseline: edit not reverted" >&2; exit 3; }
}

record_affected() { # 編集中の test-affected.sh --list（基準 HEAD = この編集だけ）を残す。何も走らせない
  local lines
  lines="$(CELERIS_WU_BASE=HEAD bash scripts/dev/test-affected.sh --list)"
  emit "$(python3 -I -c 'import json,sys;print(json.dumps({"schema":sys.argv[1],"kind":"affected","scenario":sys.argv[2],"mode":sys.argv[3],"rep":1,"at":sys.argv[4],"env":None,"cmd":"CELERIS_WU_BASE=HEAD bash scripts/dev/test-affected.sh --list","lines":sys.argv[5].splitlines(),"edit":{"file":sys.argv[6],"nonce":sys.argv[7]}},ensure_ascii=False))' "$SCHEMA" "$scenario" "$mode" "$(now)" "$lines" "$edit_file" "$1")"
}

estimate() { # release 記録（gate.json）を読むだけ。反映の推定の材料
  local rdir="${RELEASES_DIR:-/local/celeris/state/releases}"
  [ -d "$rdir" ] || return 0
  python3 -I - "$rdir" "$SCHEMA" "$scenario" "$mode" "$(now)" <<'PY' >>"$out"
import glob, json, os, sys
rdir, schema, scen, mode, at = sys.argv[1:6]
files = sorted(glob.glob(os.path.join(rdir, "*", "gate.json")), key=os.path.getmtime)[-5:]
for f in files:
    try:
        g = json.load(open(f))
    except (OSError, ValueError):
        continue
    for s in g.get("steps", []):
        if s.get("skipped"):
            continue
        print(json.dumps({"schema": schema, "kind": "estimate", "scenario": scen, "mode": mode, "rep": 0,
                          "at": at, "env": None, "source": "gate.json (read-only)",
                          "release": {"at": g.get("at"), "ok": g.get("ok")},
                          "step": s.get("step"), "secs": s.get("secs")}, ensure_ascii=False))
PY
}

# ---- policy-ts・cos-ts（ts-verify.sh を計る。cargo の段を使わない）--------------------------------------------
# 試行の前後の load は $work/trial-load.json、候補 bundle の識別は $work/bundle-id.json（ts_run が書く）から行に入れる。
# $1 kind $2 rep $3 step（trial では空） $4 cmd $5 開始 $6 終了 $7 exit $8 ts-verify の記録 $9 nonce ${10} reverted
ts_emit() {
  local envj link
  envj="$(env_json "${CARGO_TARGET_DIR:-}" null null null null)"
  link="$(sort -n "$linklog" | awk 'BEGIN{t=0;n=0;u=0;end=0} {t+=$2-$1;n++; b=($1>end)?$1:end; if($2>b){u+=$2-b;end=$2}} END{printf "%.3f %d %.3f", t, n, u}')"
  emit "$(python3 -I - "$SCHEMA" "$scenario" "$mode" "$(now)" "$envj" "$edit_file" "$link" "$work/during.json" "$@" <<'PY'
import json, os, sys
(schema, scen, mode, at, envj, efile, link, during, kind, rep, step, cmd, s, e, rc, record, nonce, reverted) = sys.argv[1:19]
secs = round(float(e) - float(s), 3)
lsecs, lcount, lunion = link.split()
try:
    v = json.load(open(record))
except (OSError, ValueError):
    v = {}  # ts-verify が記録を書く前に落ちた。cargo 起動回数は null のまま（--check-targets で落ちる）
stages = v.get("stages", [])
by_name = {st.get("name"): st.get("secs") for st in stages}
rec = {"schema": schema, "kind": kind, "scenario": scen, "mode": mode, "rep": int(rep), "at": at,
       "cmd": cmd, "secs": secs, "exit": int(rc)}
rec["link_count"] = max(int(lcount), v.get("link_count", 0))
if step:
    rec.update({"step": step, "link_secs": float(lsecs), "link_union_secs": float(lunion),
                "compile_secs": round(secs - float(lunion), 3)})
    if step == "runner-ensure":
        rec["runner_build_secs"] = secs
if kind == "trial" or step == "ts-verify":
    rec.update({"class": v.get("class"), "stages": stages,
                "typecheck_secs": by_name.get("typecheck"), "bundle_secs": by_name.get("bundle"),
                "cargo_invocations": v.get("cargo_invocations"),
                "cargo_compile_invocations": v.get("cargo_compile_invocations"),
                "cargo_calls": v.get("cargo_calls"), "units": v.get("units"),
                "verify_total_secs": v.get("total_secs")})
if nonce:
    rec["edit"] = {"file": efile, "nonce": nonce}
    rec["reverted"] = reverted == "true"
env = json.loads(envj)
try:
    env["during"] = json.load(open(during))
except (OSError, ValueError):
    env["during"] = None
work = os.path.dirname(during)
if kind == "trial" or step == "ts-verify":
    try:
        bid = json.load(open(os.path.join(work, "bundle-id.json")))
    except (OSError, ValueError):
        bid = {"bundle_sha256": None, "host_api": None, "runner_sha256": None}
    if isinstance(env.get("identity"), dict):
        env["identity"].update(bid)
if kind == "trial":
    rec["trial_secs"] = secs
    rec["over_300s"] = secs > 300
    try:
        rec["trial_load"] = json.load(open(os.path.join(work, "trial-load.json")))
    except (OSError, ValueError):
        rec["trial_load"] = None
rec["env"] = env
print(json.dumps(rec, ensure_ascii=False))
PY
)"
}

# $1 kind $2 rep $3 step。編集を当てて ts-verify を 1 回走らせ、戻してから 1 行を書く。ts-verify の exit を $ts_rc に残す。
ts_run() {
  local kind="$1" rep="$2" step="$3" nonce s e sampler reverted=false record="$work/verify-record.json"
  local cmd="env CELERIS_WU_BASE=HEAD bash scripts/dev/ts-verify.sh --record <file>"
  nonce="$(od -An -N4 -tx4 /dev/urandom | tr -d ' \n')"
  local before after
  rm -f "$record" "$work/during.json" "$work/trial-load.json" "$work/bundle-id.json"
  rm -rf "$work/tsv"
  mkdir -p "$work/tsv"
  : >"$linklog"
  before="$(load_probe "${CARGO_TARGET_DIR:-}")"
  sample_load "${CARGO_TARGET_DIR:-}" "$work/during.json" </dev/null &
  sampler=$!
  s=$(date +%s.%N)
  apply_edit "$nonce"
  ts_rc=0
  # ts-verify の作業 dir（$TMPDIR/ts-verify.*）を残させ、段 activate の出力から候補 bundle の sha256 と host_api を読む
  TS_VERIFY_KEEP_WORK=1 TMPDIR="$work/tsv" CELERIS_WU_BASE=HEAD bash scripts/dev/ts-verify.sh --record "$record" >"$work/verify.log" 2>&1 || ts_rc=$?
  e=$(date +%s.%N)
  kill "$sampler" 2>/dev/null || true
  wait "$sampler" 2>/dev/null || true
  after="$(load_probe "${CARGO_TARGET_DIR:-}")"
  if revert; then reverted=true; fi
  python3 -I - "$work/tsv" "$runner_sha256" "$before" "$after" "$work/during.json" "$work/bundle-id.json" "$work/trial-load.json" <<'PY'
import glob, json, sys
tsv, runner_sha, before, after, during, bid_out, load_out = sys.argv[1:8]
act = {}
for f in sorted(glob.glob(f"{tsv}/ts-verify.*/activate.json")):
    try:
        act = json.load(open(f))
    except (OSError, ValueError):
        pass
json.dump({"bundle_sha256": act.get("bundle_sha256"), "host_api": act.get("host_api"),
           "runner_sha256": None if runner_sha == "null" else runner_sha}, open(bid_out, "w"))
before, after = json.loads(before), json.loads(after)
try:
    d = json.load(open(during)).get("other_cargo_rustc_max", 0)
except (OSError, ValueError):
    d = 0
json.dump({"before": before, "after": after, "during_other_cargo_rustc_max": d,
           "busy": before["other_cargo_rustc"] > 0 or after["other_cargo_rustc"] > 0 or d > 0}, open(load_out, "w"))
PY
  rm -rf "$work/tsv"
  [ "$ts_rc" -eq 0 ] || tail -n 30 "$work/verify.log" >&2
  ts_emit "$kind" "$rep" "$step" "$cmd" "$s" "$e" "$ts_rc" "$record" "$nonce" "$reverted"
  [ "$reverted" = true ] || { echo "edit-activate-baseline: edit not reverted" >&2; exit 3; }
}

if [ "$is_ts" -eq 1 ]; then
  echo "edit-activate-baseline: scenario=$scenario mode=$mode reps=$reps rep_start=$rep_start out=$out" >&2
  : >"$linklog"
  rm -f "$work/during.json"
  s=$(date +%s.%N)
  rc=0
  bash scripts/dev/script-runner-cache.sh ensure >"$work/ensure.log" 2>&1 || rc=$?
  e=$(date +%s.%N)
  [ "$rc" -eq 0 ] || tail -n 30 "$work/ensure.log" >&2
  ts_emit prime 0 runner-ensure "bash scripts/dev/script-runner-cache.sh ensure" "$s" "$e" "$rc" /dev/null "" ""
  [ "$rc" -eq 0 ] || { echo "edit-activate-baseline: runner の ensure が失敗した" >&2; exit 1; }
  runner_path="$(bash scripts/dev/script-runner-cache.sh path 2>/dev/null || true)"
  if [ -n "$runner_path" ] && [ -f "$runner_path" ]; then runner_sha256="$(sha256sum "$runner_path" | cut -d' ' -f1)"; fi
  if [ "$mode" = warm ]; then
    ts_run prime 0 ts-verify
    [ "$ts_rc" -eq 0 ] || { echo "edit-activate-baseline: 温める ts-verify が失敗した" >&2; exit 1; }
  fi
  for rep in $(seq "$rep_start" $((rep_start + reps - 1))); do
    [ "$mode" != cold ] || rm -rf control-ts/node_modules
    ts_run trial "$rep" ""
  done
  echo "edit-activate-baseline: wrote $out" >&2
  exit 0
fi

echo "edit-activate-baseline: scenario=$scenario mode=$mode reps=$reps steps=[$steps] out=$out" >&2

if [ "$mode" = warm ]; then
  tdir="${CARGO_TARGET_DIR:?warm needs CARGO_TARGET_DIR}"
  for st in $steps; do run_step prime 0 "$st" "$tdir" ""; done
fi

min_gb="${BASELINE_COLD_MIN_GB:-60}"
for rep in $(seq "$rep_start" $((rep_start + reps - 1))); do
  if [ "$mode" = cold ]; then
    free="$(df -BG --output=avail /local | tail -1 | tr -dc 0-9)"
    if [ "$free" -lt "$min_gb" ]; then
      envj="$(env_json "$TMPDIR" null null null null)"
      emit "$(python3 -I -c 'import json,sys;print(json.dumps({"schema":sys.argv[1],"kind":"skipped","scenario":sys.argv[2],"mode":"cold","rep":int(sys.argv[3]),"at":sys.argv[4],"reason":f"/local free {sys.argv[5]}GB < {sys.argv[6]}GB","env":json.loads(sys.argv[7])},ensure_ascii=False))' "$SCHEMA" "$scenario" "$rep" "$(now)" "$free" "$min_gb" "$envj")"
      continue
    fi
    cold_target="$TMPDIR/edit-activate-cold-$rep"
    rm -rf "$cold_target"
    tdir="$cold_target"
  fi
  nonce="$(od -An -N4 -tx4 /dev/urandom | tr -d ' \n')"
  before_load="$(load_probe "$tdir")"
  apply_edit "$nonce"
  if [ "$scenario" = cos ] && [ "$rep" -eq "$rep_start" ]; then record_affected "$nonce"; fi
  for st in $steps; do run_step step "$rep" "$st" "$tdir" "$nonce"; done
  flush_trial "$before_load"
  if [ -n "$cold_target" ]; then rm -rf "$cold_target"; cold_target=""; fi
done
estimate
echo "edit-activate-baseline: wrote $out" >&2
