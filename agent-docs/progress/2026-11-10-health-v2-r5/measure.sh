#!/usr/bin/env bash
# 健全性v2 R5 の計測台本。B0 の test-targets-measure.sh（agent-docs/progress/2026-11-07-health-v2-b0/）を元にした。
#
# 使い方:
#   bash <this> run <label> <out.json> <crate,...>
#       指定 crate ごとに次を測り、<out.json> に書く（1 段ごとに書き直すので、途中で止まっても測れた所までは残る）。
#       build      : cargo nextest run -p <crate> --no-run の秒・Compiling 行の数・test binary の数・link した test binary の数
#       edit_verify: src/lib.rs（無ければ src/main.rs）の末尾に 1 行足した後の
#                    CELERIS_WU_BASE=HEAD bash scripts/dev/test-affected.sh --list の秒と、対象 crate 全部の --no-run の秒。
#                    B0 と同じく settle 後に測る。settle は「編集の前に、同じ対象 crate の組で --no-run を 1 回走らせる」こと。
#                    B0 の交絡（前回と違う組・前の編集の mtime で作り直しが混ざる）を除くため、組は編集で決まる対象と同じにする。
#       run1, run2 : cargo nextest run -p <crate>（build は事前に --no-run で済ませる）を 2 回走らせた秒・件数・失敗した試験名
#       各段の前後と全体の前後に /proc/loadavg を記録する。
#   bash <this> features <out.json> <crate,...> <crate,...>
#       2 つの組 A・B について cargo tree -e features の (package, feature) の差を出し、
#       A → B → A → B の順に cargo nextest run --no-run を走らせて各回の Compiling 行を記録する（作り直しの原因の確認）。
#   bash <this> --validate <out.json>
#       run の出力に必要な欄があれば 0。測れなかった項目は skipped 欄（crate・item・reason）に理由があれば通す。
#
# 環境変数:
#   MEASURE_DEADLINE  epoch 秒。これを過ぎたら残りの段を走らせず、skipped に理由を書く（run の壁時計に収めるため）。
#                     走らせる段も残り秒数を timeout の上限にする。
# 全 workspace の試験・並走 cargo の負荷はかけない（普通の build と、指定 crate の試験の実行だけ）。
# CARGO_TARGET_DIR は渡された値のまま使う。
set -u
here="$(cd "$(dirname "$0")" && pwd)"
now() { date +%s.%N; }
la() { cut -d' ' -f1-3 /proc/loadavg; }

validate() {
  python3 -I - "$1" <<'PY'
import json, sys
p = sys.argv[1]
try:
    d = json.load(open(p))
except Exception as e:
    print(f"invalid: cannot read {p}: {e}"); sys.exit(1)
errs = []
for k in ("schema", "label", "git_head", "loadavg_before", "loadavg_after", "crates", "skipped"):
    if k not in d:
        errs.append(f"missing top-level field: {k}")
if d.get("schema") != "health-v2-r5-measure/1":
    errs.append("schema must be health-v2-r5-measure/1")
skipped = {}
for s in d.get("skipped", []):
    if not str(s.get("reason", "")).strip():
        errs.append(f"skipped entry without reason: {s}")
    skipped[(s.get("crate"), s.get("item"))] = s
req = {
    "build": ("secs", "exit", "compiled_units", "test_binaries", "linked_test_binaries", "loadavg_before", "loadavg_after"),
    "edit_verify": ("edited_file", "affected", "settle_secs", "list_secs", "rebuild_secs", "rebuild_exit",
                    "rebuild_compiled_units", "edit_to_verify_secs", "loadavg_before", "loadavg_after"),
    "run": ("secs", "exit", "tests_run", "failed_tests", "loadavg_before", "loadavg_after"),
}
crates = d.get("crates", {})
if not crates:
    errs.append("no crates measured")
for c, m in crates.items():
    for item in ("build", "edit_verify", "run1", "run2"):
        if (c, item) in skipped:
            continue
        v = m.get(item)
        if not isinstance(v, dict):
            errs.append(f"{c}.{item}: missing (and not in skipped)")
            continue
        for f in req["run" if item.startswith("run") else item]:
            if f not in v:
                errs.append(f"{c}.{item}: missing field {f}")
if errs:
    print("invalid:"); [print("  " + e) for e in errs]; sys.exit(1)
print(f"ok: {len(crates)} crates, {len(skipped)} skipped item(s)")
PY
}

if [ "${1:-}" = "--validate" ]; then
  [ $# -eq 2 ] || { echo "usage: $0 --validate <out.json>" >&2; exit 2; }
  validate "$2"; exit $?
fi

mode="${1:-}"
case "$mode" in
  run) [ $# -eq 4 ] || { echo "usage: $0 run <label> <out.json> <crate,...>" >&2; exit 2; }
       label="$2"; out="$3"; crates_csv="$4" ;;
  features) [ $# -eq 4 ] || { echo "usage: $0 features <out.json> <crate,...> <crate,...>" >&2; exit 2; }
       out="$2"; set_a="$3"; set_b="$4" ;;
  *) echo "usage: $0 run|features|--validate ..." >&2; exit 2 ;;
esac

out="$(realpath -m "$out")"
raw="${out%.json}-raw"
mkdir -p "$raw"
steps="$raw/steps.jsonl"
: >"$steps"

# 残り秒（MEASURE_DEADLINE が無ければ大きな値）。
remaining() {
  if [ -n "${MEASURE_DEADLINE:-}" ]; then
    python3 -c "import time,sys; print(max(0,int(float(sys.argv[1])-time.time())))" "$MEASURE_DEADLINE"
  else echo 86400; fi
}
# step <crate> <item> <json>: 段の結果を 1 行足し、out を組み直す。
step() { python3 -I -c 'import json,sys; print(json.dumps({"crate":sys.argv[1],"item":sys.argv[2],"data":json.loads(sys.argv[3])}))' "$1" "$2" "$3" >>"$steps"; assemble; }
skip() { python3 -I -c 'import json,sys; print(json.dumps({"crate":sys.argv[1],"item":sys.argv[2],"skip":sys.argv[3]}))' "$1" "$2" "$3" >>"$steps"; assemble; }
compiled_names() { grep -E '^ *Compiling ' "$1" | awk '{print $2}' | python3 -I -c 'import json,sys; print(json.dumps([l.strip() for l in sys.stdin if l.strip()]))'; }

# 制限時間つきで実行する。残りが min 秒未満なら走らせず 124 を返す。
timed() {
  local min="$1"; shift
  local rem; rem="$(remaining)"
  [ "$rem" -ge "$min" ] || return 125
  timeout --kill-after=15 "$rem" "$@"
}

# list_binaries <args...> <outfile>: nextest の binaries-only の JSON（build は済んでいる前提）。
binaries_json() {
  local o="${@: -1}"; set -- "${@:1:$(($#-1))}"
  cargo nextest list "$@" --list-type binaries-only --message-format json 2>/dev/null >"$o" || true
}
# count_bins <json> <t0>: (binary 数, t0 以後に書かれた binary 数)
count_bins() {
  python3 -I - "$1" "$2" <<'PY'
import json, os, sys
try:
    b = json.load(open(sys.argv[1]))["rust-binaries"]
except Exception:
    print("null null"); sys.exit(0)
t0 = float(sys.argv[2])
linked = sum(1 for v in b.values() if os.path.exists(v["binary-path"]) and os.path.getmtime(v["binary-path"]) >= t0)
print(len(b), linked)
PY
}

host_json() {
  python3 -I -c 'import json,os,sys; print(json.dumps({"nproc":os.cpu_count(),"uname":sys.argv[1],"cargo":sys.argv[2],"nextest":sys.argv[3],"cargo_target_dir":os.environ.get("CARGO_TARGET_DIR")}))' \
    "$(uname -sr)" "$(cargo --version 2>/dev/null)" "$(cargo nextest --version 2>/dev/null | head -1)"
}

head_sha="$(git rev-parse HEAD)"
base_sha="$(git merge-base HEAD main 2>/dev/null || echo '')"
# 計測対象（crates/・Cargo.*・.config/nextest.toml・scripts/dev/test-affected.sh）が base と同じか。
if [ -n "$base_sha" ] && git diff --quiet "$base_sha" -- crates Cargo.toml Cargo.lock .config/nextest.toml scripts/dev/test-affected.sh; then
  src_eq=true; else src_eq=false; fi
la_start="$(la)"; t_start="$(now)"; host="$(host_json)"

assemble() {
  python3 -I - "$steps" "$out" "$mode" "${label:-features}" "$head_sha" "$base_sha" "$src_eq" "$la_start" "$(la)" "$t_start" "$host" <<'PY'
import json, sys, time
steps, out, mode, label, head, base, src_eq, la0, la1, t0, host = sys.argv[1:12]
d = {"schema": "health-v2-r5-measure/1" if mode == "run" else "health-v2-r5-features/1",
     "label": label, "git_head": head, "merge_base_main": base,
     "measured_src_equals_merge_base": src_eq == "true", "host": json.loads(host),
     "started_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(float(t0))),
     "elapsed_secs": round(time.time() - float(t0), 1),
     "loadavg_before": la0, "loadavg_after": la1, "crates": {}, "skipped": []}
for line in open(steps):
    s = json.loads(line)
    if "skip" in s:
        d["skipped"].append({"crate": s["crate"], "item": s["item"], "reason": s["skip"]})
    else:
        d["crates"].setdefault(s["crate"], {})[s["item"]] = s["data"]
json.dump(d, open(out, "w"), indent=1, ensure_ascii=False)
PY
}

# ---- features モード ---------------------------------------------------------------------------------
if [ "$mode" = features ]; then
  pargs() { local a=(); IFS=, read -ra cs <<<"$1"; for c in "${cs[@]}"; do a+=(-p "$c"); done; echo "${a[@]}"; }
  # shellcheck disable=SC2046
  for s in A B; do
    set_csv="$set_a"; [ "$s" = B ] && set_csv="$set_b"
    cargo tree $(pargs "$set_csv") -e features,normal,build,dev -f '{p} {f}' --prefix none 2>/dev/null \
      | sed -E 's/ \(\*\)$//; s/ \([^)]*\)( |$)/\1/' | sort -u >"$raw/tree-$s.txt"
  done
  python3 -I - "$raw/tree-A.txt" "$raw/tree-B.txt" "$steps" <<'PY'
import json, re, sys
def feats(p):
    m = {}
    for l in open(p):
        parts = l.split()
        if len(parts) < 2 or parts[1].startswith("feature"):
            continue
        name, ver = parts[0], parts[1]
        fs = set(parts[2].split(",")) if len(parts) > 2 else set()
        m.setdefault(f"{name} {ver}", set()).update(f for f in fs if f)
    return m
a, b = feats(sys.argv[1]), feats(sys.argv[2])
diff = {k: {"only_in_A": sorted(a[k] - b[k]), "only_in_B": sorted(b[k] - a[k])}
        for k in sorted(set(a) & set(b)) if a[k] != b[k]}
open(sys.argv[3], "a").write(json.dumps({"crate": "_tree", "item": "feature_diff",
    "data": {"packages_A": len(a), "packages_B": len(b), "differing": diff}}) + "\n")
PY
  assemble
  i=0
  for s in A B A B; do
    i=$((i+1)); set_csv="$set_a"; [ "$s" = B ] && set_csv="$set_b"
    lg="$raw/build-$i-$s.log"; l0="$(la)"; t0="$(now)"
    # shellcheck disable=SC2046
    timed 60 cargo nextest run $(pargs "$set_csv") --no-run >"$lg" 2>&1; rc=$?
    t1="$(now)"
    if [ "$rc" = 125 ]; then skip "_build" "$i-$s" "MEASURE_DEADLINE までの残りが 60 秒未満"; continue; fi
    step "_build" "$i-$s" "$(python3 -I -c 'import json,sys; print(json.dumps({"set":sys.argv[1],"exit":int(sys.argv[2]),"secs":round(float(sys.argv[4])-float(sys.argv[3]),2),"compiled":json.loads(sys.argv[5]),"loadavg_before":sys.argv[6],"loadavg_after":sys.argv[7]}))' "$set_csv" "$rc" "$t0" "$t1" "$(compiled_names "$lg")" "$l0" "$(la)")"
  done
  assemble; cat "$out"; exit 0
fi

# ---- run モード ----------------------------------------------------------------------------------------
IFS=, read -ra crates <<<"$crates_csv"
for c in "${crates[@]}"; do
  # build
  lg="$raw/build-$c.log"; l0="$(la)"; t0="$(now)"
  timed 60 cargo nextest run -p "$c" --no-run >"$lg" 2>&1; rc=$?
  t1="$(now)"
  if [ "$rc" = 125 ]; then
    for it in build edit_verify run1 run2; do skip "$c" "$it" "MEASURE_DEADLINE までの残りが足りず走らせなかった"; done; continue
  fi
  binaries_json -p "$c" "$raw/build-$c.binaries.json"
  read -r nb nl <<<"$(count_bins "$raw/build-$c.binaries.json" "$t0")"
  step "$c" build "$(python3 -I -c 'import json,sys; a=sys.argv; print(json.dumps({"exit":int(a[1]),"timed_out":a[1] in ("124","137"),"secs":round(float(a[3])-float(a[2]),2),"compiled_units":len(json.loads(a[4])),"compiled":json.loads(a[4]),"test_binaries":None if a[5]=="null" else int(a[5]),"linked_test_binaries":None if a[6]=="null" else int(a[6]),"loadavg_before":a[7],"loadavg_after":a[8]}))' \
    "$rc" "$t0" "$t1" "$(compiled_names "$lg")" "$nb" "$nl" "$l0" "$(la)")"

  # edit_verify（settle 後）
  f="crates/$c/src/lib.rs"; git ls-files --error-unmatch "$f" >/dev/null 2>&1 || f="crates/$c/src/main.rs"
  probe=$'\n// edit-verify probe (R5 measure.sh)\n'
  printf '%s' "$probe" >>"$f"
  CELERIS_WU_BASE=HEAD bash scripts/dev/test-affected.sh --list >"$raw/edit-$c.list" 2>"$raw/edit-$c.list.err"
  git checkout -- "$f"
  args=(); while IFS= read -r a; do [ -n "$a" ] && args+=(-p "$a"); done < <(grep -v ':' "$raw/edit-$c.list")
  if [ "${#args[@]}" -eq 0 ]; then skip "$c" edit_verify "test-affected.sh --list が crate を返さなかった（$raw/edit-$c.list.err）"
  else
    sl="$raw/settle-$c.log"; ts0="$(now)"
    timed 60 cargo nextest run "${args[@]}" --no-run >"$sl" 2>&1; src=$?; ts1="$(now)"
    if [ "$src" = 125 ] || [ "$src" = 124 ] || [ "$src" = 137 ]; then
      skip "$c" edit_verify "settle の build が MEASURE_DEADLINE までに終わらない（exit $src）"
    else
      l0="$(la)"; printf '%s' "$probe" >>"$f"; t0="$(now)"
      CELERIS_WU_BASE=HEAD bash scripts/dev/test-affected.sh --list >"$raw/edit-$c.list2" 2>>"$raw/edit-$c.list.err"; lrc=$?
      t1="$(now)"
      lg="$raw/edit-$c.log"
      timed 30 cargo nextest run "${args[@]}" --no-run >"$lg" 2>&1; rc=$?
      t3="$(now)"; l1="$(la)"
      git checkout -- "$f"
      binaries_json "${args[@]}" "$raw/edit-$c.binaries.json"
      read -r nb nl <<<"$(count_bins "$raw/edit-$c.binaries.json" "$t1")"
      step "$c" edit_verify "$(python3 -I - "$f" "$raw/edit-$c.list2" "$lrc" "$rc" "$ts0" "$ts1" "$t0" "$t1" "$t3" "$(compiled_names "$sl")" "$(compiled_names "$lg")" "$nb" "$nl" "$l0" "$l1" "$src" <<'PY'
import json, sys
a = sys.argv
lines = open(a[2]).read().splitlines()
settle_c, reb_c = json.loads(a[10]), json.loads(a[11])
print(json.dumps({"edited_file": a[1], "affected": [l for l in lines if ":" not in l],
  "extra_checks": [l for l in lines if ":" in l], "settle_exit": int(a[16]),
  "settle_secs": round(float(a[6]) - float(a[5]), 2), "settle_compiled_units": len(settle_c), "settle_compiled": settle_c,
  "list_exit": int(a[3]), "list_secs": round(float(a[8]) - float(a[7]), 2),
  "rebuild_exit": int(a[4]), "rebuild_timed_out": a[4] in ("124", "137"),
  "rebuild_secs": round(float(a[9]) - float(a[8]), 2),
  "rebuild_compiled_units": len(reb_c), "rebuild_compiled": reb_c,
  "test_binaries": None if a[12] == "null" else int(a[12]),
  "linked_test_binaries": None if a[13] == "null" else int(a[13]),
  "edit_to_verify_secs": round(float(a[9]) - float(a[7]), 2),
  "loadavg_before": a[14], "loadavg_after": a[15]}))
PY
)"
    fi
  fi

  # run1・run2（build は先に済ませる。編集を戻した mtime の分の作り直しを試験の秒に混ぜない）
  pl="$raw/prerun-$c.log"
  timed 60 cargo nextest run -p "$c" --no-run >"$pl" 2>&1; prc=$?
  if [ "$prc" = 125 ] || [ "$prc" = 124 ] || [ "$prc" = 137 ]; then
    skip "$c" run1 "試験の前の build が MEASURE_DEADLINE までに終わらない（exit $prc）"
    skip "$c" run2 "試験の前の build が MEASURE_DEADLINE までに終わらない（exit $prc）"; continue
  fi
  for r in 1 2; do
    lg="$raw/run$r-$c.log"; l0="$(la)"; t0="$(now)"
    timed 120 cargo nextest run -p "$c" --no-fail-fast >"$lg" 2>&1; rc=$?
    t1="$(now)"
    if [ "$rc" = 125 ]; then skip "$c" "run$r" "MEASURE_DEADLINE までの残りが 120 秒未満で走らせなかった"; continue; fi
    step "$c" "run$r" "$(python3 -I - "$lg" "$rc" "$t0" "$t1" "$l0" "$(la)" <<'PY'
import json, re, sys
lg, rc, t0, t1, l0, l1 = sys.argv[1:7]
txt = open(lg, errors="replace").read()
failed = sorted(set(m.group(2) for m in re.finditer(r"^\s*(FAIL|TIMEOUT|SIGSEGV|SIGABRT|SIGKILL|ABORT)\s+\[[^\]]*\]\s+(?:\(\s*\d+/\d+\)\s+)?(\S+\s+\S+)", txt, re.M)))
m = re.search(r"Summary \[\s*([\d.]+)s\]\s+(\d+) tests? run: (\d+) passed(?:.*?(\d+) failed)?(?:.*?(\d+) timed out)?(?:.*?(\d+) skipped)?", txt)
d = {"exit": int(rc), "timed_out": rc in ("124", "137"), "secs": round(float(t1) - float(t0), 2),
     "nextest_summary_secs": float(m.group(1)) if m else None,
     "tests_run": int(m.group(2)) if m else None, "passed": int(m.group(3)) if m else None,
     "failed": int(m.group(4) or 0) if m else None, "timed_out_tests": int(m.group(5) or 0) if m else None,
     "skipped_tests": int(m.group(6) or 0) if m else None,
     "failed_tests": failed, "loadavg_before": l0, "loadavg_after": l1}
print(json.dumps(d))
PY
)"
  done
done
assemble
cat "$out"
validate "$out"
