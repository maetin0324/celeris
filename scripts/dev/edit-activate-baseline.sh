#!/usr/bin/env bash
# edit→検査→test→反映（activate）の baseline を計る（task 01M6DCPKPJZZ9F9J4QJ47FH8KK、S0 (2)）。
#
#   bash scripts/dev/edit-activate-baseline.sh --scenario <policy|cos> --mode <warm|cold> --reps <N> --out <file.jsonl>
#   bash scripts/dev/edit-activate-baseline.sh --dry-run --scenario <policy|cos>   build せず手順を出して exit 0
#   bash scripts/dev/edit-activate-baseline.sh --validate <file.jsonl>             記録の欄を検査（欠けがあれば exit 1）
#
# 1 試行 = 代表的な編集を 1 件当てる → 段（steps）を順に計る → 編集を戻す。編集は試行ごとに違う nonce を入れるので
# sccache・cargo の fingerprint が前の試行の成果を使い回さない。編集は trap（EXIT/INT/TERM/HUP）で必ず戻し、戻した後に
# 元の内容と cmp で一致を確かめる（一致しなければ exit 3）。
#   policy: crates/task-core/src/model_policy.rs の RULES に、実際には当たらない規則を 1 件足す（ADR-0069 D3 の規則表）
#   cos:    crates/task-api/src/cos/basis/job_wait_extend.rs の validate に、実際には当たらない制御（CoS 定型判断の
#           数十行の分岐。集計 → match → 拒否）を足す。編集中に test-affected.sh --list（基準 HEAD = この編集だけ）を
#           kind=affected の行に残す
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
# 環境変数: BASELINE_STEPS（既定: 上の 6 段を空白区切り）、BASELINE_JOBS（cargo -j。既定は cargo の既定）、
#           RELEASES_DIR（既定 /local/celeris/state/releases）、BASELINE_COLD_MIN_GB（既定 60）。
set -euo pipefail

SCHEMA="celeris.edit-activate-baseline/1"
ALL_STEPS="check build clippy test-build test-run release-build"
root="$(git rev-parse --show-toplevel)"
cd "$root"

usage() { sed -n "2,6p" "$0" >&2; exit 2; }

scenario="" mode="" reps="" out="" dry=0 validate=""
while [ $# -gt 0 ]; do
  case "$1" in
    --scenario) scenario="${2:-}"; shift 2 ;;
    --mode) mode="${2:-}"; shift 2 ;;
    --reps) reps="${2:-}"; shift 2 ;;
    --out) out="${2:-}"; shift 2 ;;
    --dry-run) dry=1; shift ;;
    --validate) validate="${2:-}"; shift 2 ;;
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
}
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
    if k in ("step", "prime", "skipped"):
        env = r.get("env") or {}
        for key in env_keys:
            if key not in env:
                bad.append(f"line {i}: missing env.{key}")
        sc = env.get("sccache") or {}
        for key in ("present", "reachable", "hits_delta", "misses_delta"):
            if key not in sc:
                bad.append(f"line {i}: missing env.sccache.{key}")
    if k == "step":
        if not isinstance(r.get("secs"), (int, float)) or not isinstance(r.get("exit"), int):
            bad.append(f"line {i}: secs/exit types")
        if r.get("reverted") is not True:
            bad.append(f"line {i}: edit was not confirmed reverted")
if n == 0:
    bad.append("no records")
for b in bad:
    print(b, file=sys.stderr)
print(json.dumps({"file": path, "records": n, "kinds": kinds, "problems": len(bad)}, ensure_ascii=False))
sys.exit(1 if bad else 0)
PY
fi

case "$scenario" in
  policy) crate="task-core"; edit_file="crates/task-core/src/model_policy.rs" ;;
  cos) crate="task-api"; edit_file="crates/task-api/src/cos/basis/job_wait_extend.rs" ;;
  *) usage ;;
esac
steps="${BASELINE_STEPS:-$ALL_STEPS}"
jobs_arg=()
[ -z "${BASELINE_JOBS:-}" ] || jobs_arg=(-j "$BASELINE_JOBS")

step_cmd() { # step → command line（表示用と実行用で同じ）
  case "$1" in
    check) echo "cargo check ${jobs_arg[*]:-} -p $crate" ;;
    build) echo "cargo build ${jobs_arg[*]:-} -p celeris" ;;
    clippy) echo "cargo clippy ${jobs_arg[*]:-} -p $crate -- -D warnings" ;;
    test-build) echo "cargo nextest run ${jobs_arg[*]:-} -p $crate --no-run" ;;
    test-run) echo "cargo nextest run ${jobs_arg[*]:-} -p $crate --no-fail-fast" ;;
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

if [ "$dry" -eq 1 ]; then
  echo "scenario: $scenario（編集: $edit_file、対象 crate: $crate）"
  echo "1. 元の $edit_file を \$TMPDIR に写し、trap で必ず戻す（戻した後に cmp で一致を確認）"
  echo "2. 試行ごとに nonce を変えた編集を当てる（policy: RULES に当たらない規則 1 件 / cos: validate に当たらない数十行の分岐）"
  echo "3. linker を時間記録 wrapper に差し替え（CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER）、段を順に計る:"
  for s in $steps; do echo "   - $s: $(step_cmd "$s")"; done
  echo "4. warm: CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-<unset>} を編集なしで 1 回温める（kind=prime）"
  echo "   cold: \$TMPDIR/edit-activate-cold-<rep> の使い捨て target（/local の空き < ${BASELINE_COLD_MIN_GB:-60}GB なら kind=skipped）"
  echo "5. 反映は計らず、${RELEASES_DIR:-/local/celeris/state/releases}/*/gate.json を読んで kind=estimate の行を出す（読むだけ）"
  echo "6. 1 行 1 段の JSONL に env（commit 件名・merge-base・rustc・CPU・loadavg・cargo/rustc 数・sccache・空き容量）を書く"
  exit 0
fi

case "$mode" in warm|cold) ;; *) usage ;; esac
[[ "$reps" =~ ^[1-9][0-9]*$ ]] || usage
[ -n "$out" ] || usage
mkdir -p "$(dirname "$out")"
out="$(cd "$(dirname "$out")" && pwd)/$(basename "$out")"

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

flush_trial() { # 編集を戻せたかを各行に書いてから out に移す
  local ok=false
  if revert; then ok=true; fi
  python3 -I -c '
import json,sys
ok = sys.argv[2] == "true"
for l in open(sys.argv[1]):
    r = json.loads(l); r["reverted"] = ok; print(json.dumps(r, ensure_ascii=False))' "$work/pending" "$ok" >"$work/pending.done"
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

echo "edit-activate-baseline: scenario=$scenario mode=$mode reps=$reps steps=[$steps] out=$out" >&2

if [ "$mode" = warm ]; then
  tdir="${CARGO_TARGET_DIR:?warm needs CARGO_TARGET_DIR}"
  for st in $steps; do run_step prime 0 "$st" "$tdir" ""; done
fi

min_gb="${BASELINE_COLD_MIN_GB:-60}"
for rep in $(seq 1 "$reps"); do
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
  apply_edit "$nonce"
  if [ "$scenario" = cos ] && [ "$rep" -eq 1 ]; then record_affected "$nonce"; fi
  for st in $steps; do run_step step "$rep" "$st" "$tdir" "$nonce"; done
  flush_trial
  if [ -n "$cold_target" ]; then rm -rf "$cold_target"; cold_target=""; fi
done
estimate
echo "edit-activate-baseline: wrote $out" >&2
