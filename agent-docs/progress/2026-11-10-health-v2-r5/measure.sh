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
#       label が head のときは続けて次の 2 段を測る（label が base でも head でもなければ skipped に理由を書く）。
#       bundles    : crates/<crate>/tests/*/main.rs の束ごとに cargo test -p <crate> --test <bundle> を 1 回（1 process の実行）。
#                    build は先に --no-run で済ませ、束ごとに exit・passed・failed・ignored・失敗した試験名・秒・loadavg を書く。
#       base_run   : $MEASURE_BASELINE（既定は同じ dir の baseline.json）の同じ crate の run1・run2 の loadavg_before・after の
#                    1 分値の最大と、この run の同じ値の最大を比べる。比が 2 を超えるか baseline に crate が無ければ、
#                    git merge-base HEAD main を $TMPDIR の git worktree（compare と同じ関数）で cargo nextest run -p <crate>
#                    --no-fail-fast を 1 回走らせる。要らなければ needed=false と比と両方の値だけを書く。
#       既存の <out.json> には crate を足していく。別の crate の記録は残し、同じ crate を測り直したらその crate の記録だけを
#       置き換える（正本は <out>-raw/steps.jsonl。<out.json> はそこから組み直す）。<out.json> の label と違う label では走らせない。
#       各段の前後と全体の前後に /proc/loadavg を記録する。crate ごとの session に git_head・label・開始時刻を書く。
#   bash <this> features <out.json> <crate,...> <crate,...>
#       2 つの組 A・B について cargo tree -e features の (package, feature) の差を出し、
#       A → B → A → B の順に cargo nextest run --no-run を走らせて各回の Compiling 行を記録する（作り直しの原因の確認）。
#   bash <this> compare <out.json> <crate,...>
#       base（git merge-base HEAD main）と HEAD を同じ run で交互に測る（試験は走らせない）。base は $TMPDIR の下に
#       git worktree add した写しで測り、終わったら git worktree remove する。crate ごとに base と HEAD の順を入れ替える。
#       targets    : cargo metadata の target の数（kind ごと。build しない）
#       build      : cargo nextest run -p <crate> --no-run で settle した後、src の mtime を進めて同じ build を測る
#                    （その crate の lib と全 test target の作り直し・link）。呼び方は両側で同じ。
#       edit_verify: run の edit_verify と同じ手順だが、build はその側の scripts/dev/test-affected.sh の nextest の行
#                    （base は -p の組、HEAD は --workspace -E の filter）を --no-run で走らせる。その後に同じ側の doc test の行も測る。
#   bash <this> --validate <out.json>
#       run・compare の出力に必要な欄があれば 0。測れなかった項目は skipped 欄（crate・item・reason）に理由があれば通す。
#       label が base でない run の出力には bundles・base_run も求める。
#   bash <this> --require <out.json> <crate,...>
#       --validate が通り、label が head で、名指した各 crate に run1・run2・bundles（見つけた全束の結果）・base_run が
#       skipped でなく揃う時だけ 0（needed=false の base_run も揃ったと数える）。
#
# 環境変数:
#   MEASURE_DEADLINE  epoch 秒。これを過ぎたら残りの段を走らせず、skipped に理由を書く（run の壁時計に収めるため）。
#                     走らせる段も残り秒数を timeout の上限にする。
#   MEASURE_BASELINE  base_run が比べる run の出力（既定は同じ dir の baseline.json）。
# 全 workspace の試験・並走 cargo の負荷はかけない（普通の build と、指定 crate の試験の実行だけ）。
# CARGO_TARGET_DIR は渡された値のまま使う。
set -u
here="$(cd "$(dirname "$0")" && pwd)"
now() { date +%s.%N; }
la() { cut -d' ' -f1-3 /proc/loadavg; }

# validate <out.json> [<crate,...>]: 2 つ目の引数があれば --require の判定もする。
validate() {
  python3 -I - "$@" <<'PY'
import json, sys
p = sys.argv[1]
require = [c for c in sys.argv[2].split(",") if c] if len(sys.argv) > 2 else None
try:
    d = json.load(open(p))
except Exception as e:
    print(f"invalid: cannot read {p}: {e}"); sys.exit(1)
errs = []
if d.get("schema") == "health-v2-r5-build-compare/1":
    # compare の出力: base と HEAD の両方に、同じ crate の targets・build・edit_verify がある（無い項目は skipped に理由）。
    for k in ("label", "git_head", "merge_base_main", "loadavg_before", "loadavg_after", "order", "sides", "skipped"):
        if k not in d:
            errs.append(f"missing top-level field: {k}")
    skipped = set()
    for s in d.get("skipped", []):
        if not str(s.get("reason", "")).strip():
            errs.append(f"skipped entry without reason: {s}")
        skipped.add((s.get("side"), s.get("crate"), s.get("item")))
    req = {
        "targets": ("test_targets", "kinds"),
        "build": ("cmd", "secs", "exit", "compiled_units", "test_binaries", "linked_test_binaries",
                  "loadavg_before", "loadavg_after"),
        "edit_verify": ("edited_file", "affected", "cmd", "settle_secs", "list_secs", "rebuild_secs", "rebuild_exit",
                        "rebuild_compiled_units", "test_binaries", "linked_test_binaries", "edit_to_verify_secs",
                        "loadavg_before", "loadavg_after"),
    }
    sides = d.get("sides", {})
    crates = [o.get("crate") for o in d.get("order", [])]
    if set(sides) != {"base", "head"}:
        errs.append(f"sides must be base and head: {sorted(sides)}")
    if not crates:
        errs.append("order is empty")
    both = 0
    for c in dict.fromkeys(crates):
        built = 0
        for side in ("base", "head"):
            m = sides.get(side, {}).get("crates", {}).get(c, {})
            for item, fields in req.items():
                if (side, c, item) in skipped:
                    continue
                v = m.get(item)
                if not isinstance(v, dict):
                    errs.append(f"{side}.{c}.{item}: missing (and not in skipped)")
                    continue
                errs.extend(f"{side}.{c}.{item}: missing field {f}" for f in fields if f not in v)
            built += isinstance(m.get("build"), dict)
        both += built == 2
    if crates and both == 0:
        errs.append("no crate has build values on both base and head")
    for side in ("base", "head"):
        if not sides.get(side, {}).get("git_head"):
            errs.append(f"{side}: missing git_head")
    if errs:
        print("invalid:"); [print("  " + e) for e in errs]; sys.exit(1)
    print(f"ok: compare {len(set(crates))} crates (both sides built: {both}), {len(skipped)} skipped item(s)")
    if require is not None:
        print("not required-complete: compare output (--require needs a run output with label head)"); sys.exit(1)
    sys.exit(0)
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
    "bundles": ("found", "results"),
    "bundle": ("cmd", "exit", "passed", "failed", "ignored", "failed_tests", "build_secs", "secs",
               "loadavg_before", "loadavg_after"),
    "base_run": ("needed", "ratio", "baseline_loadavg_1m_max", "head_loadavg_1m_max", "baseline"),
    "base_run_needed": ("base_sha", "cmd", "secs", "exit", "tests_run", "failed_tests", "loadavg_before", "loadavg_after"),
}
# label が base でない run（HEAD の計測）は、束の 1 process 実行と base の実行（または要らない判定）も求める。
items = ("build", "edit_verify", "run1", "run2") + (() if d.get("label") == "base" else ("bundles", "base_run"))
crates = d.get("crates", {})
if not crates:
    errs.append("no crates measured")
for c, m in crates.items():
    for item in items:
        if (c, item) in skipped:
            continue
        v = m.get(item)
        if not isinstance(v, dict):
            errs.append(f"{c}.{item}: missing (and not in skipped)")
            continue
        for f in req["run" if item.startswith("run") else item]:
            if f not in v:
                errs.append(f"{c}.{item}: missing field {f}")
        if item == "bundles" and isinstance(v.get("results"), dict):
            for b in v.get("found", []):
                if (c, f"bundles:{b}") in skipped:
                    continue
                r = v["results"].get(b)
                if not isinstance(r, dict):
                    errs.append(f"{c}.bundles.{b}: missing (and not in skipped)")
                    continue
                errs.extend(f"{c}.bundles.{b}: missing field {f}" for f in req["bundle"] if f not in r)
        if item == "base_run" and v.get("needed") is True:
            errs.extend(f"{c}.base_run: missing field {f}" for f in req["base_run_needed"] if f not in v)
if errs:
    print("invalid:"); [print("  " + e) for e in errs]; sys.exit(1)
print(f"ok: {len(crates)} crates, {len(skipped)} skipped item(s)")
if require is None:
    sys.exit(0)
# --require: label が head で、名指した crate に run1・run2・bundles・base_run が skipped でなく揃う。
miss = []
if d.get("label") != "head":
    miss.append(f"label is {d.get('label')!r}, not 'head'")
for c in require:
    m = crates.get(c)
    if m is None:
        miss.append(f"{c}: not measured"); continue
    for item in ("run1", "run2", "bundles", "base_run"):
        if (c, item) in skipped or not isinstance(m.get(item), dict):
            miss.append(f"{c}.{item}: skipped or missing")
    b = m.get("bundles")
    if isinstance(b, dict):
        miss.extend(f"{c}.bundles.{n}: skipped or missing" for n in b.get("found", [])
                    if (c, f"bundles:{n}") in skipped or not isinstance(b.get("results", {}).get(n), dict))
if miss:
    print("not required-complete:"); [print("  " + e) for e in miss]; sys.exit(1)
print(f"required-complete: {', '.join(require)}")
PY
}

if [ "${1:-}" = "--validate" ]; then
  [ $# -eq 2 ] || { echo "usage: $0 --validate <out.json>" >&2; exit 2; }
  validate "$2"; exit $?
fi
if [ "${1:-}" = "--require" ]; then
  [ $# -eq 3 ] || { echo "usage: $0 --require <out.json> <crate,...>" >&2; exit 2; }
  validate "$2" "$3"; exit $?
fi

mode="${1:-}"
case "$mode" in
  run) [ $# -eq 4 ] || { echo "usage: $0 run <label> <out.json> <crate,...>" >&2; exit 2; }
       label="$2"; out="$3"; crates_csv="$4" ;;
  features) [ $# -eq 4 ] || { echo "usage: $0 features <out.json> <crate,...> <crate,...>" >&2; exit 2; }
       out="$2"; set_a="$3"; set_b="$4" ;;
  compare) [ $# -eq 3 ] || { echo "usage: $0 compare <out.json> <crate,...>" >&2; exit 2; }
       out="$2"; crates_csv="$3"; label=compare ;;
  *) echo "usage: $0 run|features|compare|--validate|--require ..." >&2; exit 2 ;;
esac

out="$(realpath -m "$out")"
raw="${out%.json}-raw"
mkdir -p "$raw"
steps="$raw/steps.jsonl"
if [ "$mode" = run ] && [ -e "$out" ]; then
  # 既存の out に足す: label が同じ時だけ。正本の steps.jsonl から、測り直す crate の行だけを除く。
  old_label="$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1])).get("label",""))' "$out" 2>/dev/null)"
  [ "$old_label" = "$label" ] || { echo "measure: $out の label は '$old_label' で、'$label' の記録を足さない" >&2; exit 2; }
  [ -f "$steps" ] || { echo "measure: $out はあるが正本の $steps が無い" >&2; exit 2; }
  python3 -I - "$steps" "$crates_csv" <<'PY' || exit 2
import json, sys
p, cs = sys.argv[1], set(sys.argv[2].split(","))
keep = [l for l in open(p) if l.strip() and json.loads(l).get("crate") not in cs]
open(p, "w").writelines(keep)
PY
else
  : >"$steps"
fi

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

# base（git merge-base HEAD main）の tree。compare と run の base_run が共有する（写しは作らず、終われば消す）。
head_tree="$(git rev-parse --show-toplevel)"; base_tree=""
base_tree_add() {
  [ -n "$base_sha" ] || { echo "measure: git merge-base HEAD main が無い" >&2; return 2; }
  base_tree="$(mktemp -d "${TMPDIR:?TMPDIR が要る}/r5-measure-base.XXXXXX")" && rmdir "$base_tree" || return 2
  echo "worktree add --detach $base_tree $base_sha" >>"$raw/worktree.log"
  git worktree add --detach "$base_tree" "$base_sha" >>"$raw/worktree.log" 2>&1 || { cat "$raw/worktree.log" >&2; base_tree=""; return 2; }
  trap base_tree_remove EXIT
}
base_tree_remove() {
  [ -n "$base_tree" ] || return 0
  git -C "$head_tree" worktree remove --force "$base_tree" >>"$raw/worktree.log" 2>&1; echo "worktree remove: exit $?" >>"$raw/worktree.log"
  base_tree=""
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

# ---- compare モード ------------------------------------------------------------------------------------
if [ "$mode" = compare ]; then
  : >"$raw/worktree.log"
  base_tree_add || exit 2
  tree_of() { if [ "$1" = head ]; then echo "$head_tree"; else echo "$base_tree"; fi; }
  IFS=, read -ra crates <<<"$crates_csv"
  order_json="$(python3 -I -c 'import json,sys; cs=sys.argv[1].split(","); print(json.dumps([{"crate":c,"side":s} for i,c in enumerate(cs) for s in (("base","head") if i%2==0 else ("head","base"))]))' "$crates_csv")"
  assemble_cmp() {
    python3 -I - "$steps" "$out" "$head_sha" "$base_sha" "$la_start" "$(la)" "$t_start" "$host" "$order_json" <<'PY'
import json, sys, time
steps, out, head, base, la0, la1, t0, host, order = sys.argv[1:10]
d = {"schema": "health-v2-r5-build-compare/1", "label": "compare", "git_head": head, "merge_base_main": base,
     "base_tree": "git worktree add --detach $TMPDIR/r5-measure-base.* <merge_base_main>（計測の後に git worktree remove）",
     "host": json.loads(host), "started_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(float(t0))),
     "elapsed_secs": round(time.time() - float(t0), 1), "loadavg_before": la0, "loadavg_after": la1,
     "order": json.loads(order), "skipped": [],
     "sides": {"base": {"git_head": base, "crates": {}}, "head": {"git_head": head, "crates": {}}}}
for line in open(steps):
    s = json.loads(line)
    if "skip" in s:
        d["skipped"].append({"side": s["side"], "crate": s["crate"], "item": s["item"], "reason": s["skip"]})
    elif s["crate"] == "_side":
        d["sides"][s["side"]][s["item"]] = s["data"]
    else:
        d["sides"][s["side"]]["crates"].setdefault(s["crate"], {})[s["item"]] = s["data"]
json.dump(d, open(out, "w"), indent=1, ensure_ascii=False)
PY
  }
  cstep() { python3 -I -c 'import json,sys; print(json.dumps({"side":sys.argv[1],"crate":sys.argv[2],"item":sys.argv[3],"data":json.loads(sys.argv[4])}))' "$@" >>"$steps"; assemble_cmp; }
  cskip() { python3 -I -c 'import json,sys; print(json.dumps({"side":sys.argv[1],"crate":sys.argv[2],"item":sys.argv[3],"skip":sys.argv[4]}))' "$@" >>"$steps"; assemble_cmp; }
  # bins <json> <t0> <crate,...>: 一覧の binary の数、うち指定 crate の数、指定 crate で t0 以後に書かれた数、全体で t0 以後に書かれた数
  bins() {
    python3 -I - "$@" <<'PY'
import json, os, sys
try:
    b = json.load(open(sys.argv[1]))["rust-binaries"]
except Exception:
    print("null null null null"); sys.exit(0)
t0, pk = float(sys.argv[2]), set(sys.argv[3].split(","))
new = lambda v: os.path.exists(v["binary-path"]) and os.path.getmtime(v["binary-path"]) >= t0
mine = [v for v in b.values() if v["binary-id"].split("::")[0] in pk]
print(len(b), len(mine), sum(map(new, mine)), sum(map(new, b.values())))
PY
  }
  # その側の test-affected.sh から、対象 crate の引数を作る行と nextest・doc test の行を取り出す（呼び方の写しを持たない）。
  for side in base head; do
    ta="$(tree_of "$side")/scripts/dev/test-affected.sh"
    grep -E '^\s*(args=\(\)|filter=""|while IFS= read -r c; do (args|filter).*<<<"\$crates")\s*$' "$ta" >"$raw/inv-$side.prep"
    grep -E '^\s*ttl_run cargo nextest run ' "$ta" | sed -E 's/^\s*ttl_run //' >"$raw/inv-$side.nextest"
    grep -E '^\s*ttl_run cargo test --doc' "$ta" | sed -E 's/^\s*ttl_run //' >"$raw/inv-$side.doc"
    if [ "$(wc -l <"$raw/inv-$side.prep")" -ne 2 ] || [ "$(wc -l <"$raw/inv-$side.nextest")" -ne 1 ] || [ "$(wc -l <"$raw/inv-$side.doc")" -ne 1 ]; then
      echo "measure: $ta から nextest・doc test の呼び方を取り出せない（$raw/inv-$side.*）" >&2; exit 2
    fi
    cstep "$side" _side invocation "$(python3 -I -c 'import json,sys; print(json.dumps({"prep":[l.strip() for l in open(sys.argv[1])],"nextest":open(sys.argv[2]).read().strip(),"doc_test":open(sys.argv[3]).read().strip()}))' "$raw/inv-$side.prep" "$raw/inv-$side.nextest" "$raw/inv-$side.doc")"
    (cd "$(tree_of "$side")" && cargo metadata --no-deps --format-version 1 2>/dev/null) >"$TMPDIR/r5-metadata-$side.json"
    for c in "${crates[@]}"; do
      cstep "$side" "$c" targets "$(python3 -I - "$TMPDIR/r5-metadata-$side.json" "$c" <<'PY'
import json, sys, collections
m = json.load(open(sys.argv[1]))
p = next(p for p in m["packages"] if p["name"] == sys.argv[2])
kinds = collections.Counter(k for t in p["targets"] for k in t["kind"])
print(json.dumps({"kinds": dict(sorted(kinds.items())), "test_targets": kinds.get("test", 0),
                  "test_target_names": sorted(t["name"] for t in p["targets"] if "test" in t["kind"])}))
PY
)"
    done
  done
  # side_run <side> <nextest|list|doc> <crates（改行区切り）>: その側の tree で、その側の test-affected.sh の行を実行する。
  side_run() {
    local side="$1" kind="$2" crates="$3" cmd
    cmd="$(cat "$raw/inv-$side.$( [ "$kind" = doc ] && echo doc || echo nextest)")"
    case "$kind" in
      nextest) cmd="timed 30 $cmd --no-run" ;;
      list) cmd="${cmd/cargo nextest run/cargo nextest list --list-type binaries-only --message-format json}" ;;
      doc) cmd="timed 30 $cmd" ;;
    esac
    (cd "$(tree_of "$side")" && eval "$(cat "$raw/inv-$side.prep")" && eval "$cmd")
  }
  probe=$'\n// edit-verify probe (R5 measure.sh compare)\n'
  measure_one() {
    local side="$1" c="$2" tree f pre rc src lrc drc l0 l1 t0 t1 t2 t3 t4 nb nm nl na aff
    tree="$(tree_of "$side")"; pre="$raw/$side-$c"
    f="crates/$c/src/lib.rs"; git -C "$tree" ls-files --error-unmatch "$f" >/dev/null 2>&1 || f="crates/$c/src/main.rs"
    # build: 両側とも -p <crate>。settle の後に src の mtime を進め、その crate と test target の作り直しを測る。
    (cd "$tree" && timed 60 cargo nextest run -p "$c" --no-run) >"$pre-build-settle.log" 2>&1; src=$?
    if [ "$src" -ge 124 ] && [ "$src" -le 137 ]; then
      cskip "$side" "$c" build "settle の build が MEASURE_DEADLINE までに終わらない（exit $src）"
    else
      touch "$tree/$f"; l0="$(la)"; t0="$(now)"
      (cd "$tree" && timed 30 cargo nextest run -p "$c" --no-run) >"$pre-build.log" 2>&1; rc=$?; t1="$(now)"
      (cd "$tree" && cargo nextest list -p "$c" --list-type binaries-only --message-format json 2>/dev/null) >"$pre-build.binaries.json"
      read -r nb nm nl na <<<"$(bins "$pre-build.binaries.json" "$t0" "$c")"
      cstep "$side" "$c" build "$(python3 -I -c 'import json,sys; a=sys.argv; c=json.loads(a[5]); n=lambda x: None if x=="null" else int(x); print(json.dumps({"cmd":"cargo nextest run -p "+a[10]+" --no-run（settle の後に "+a[11]+" の mtime を進める）","exit":int(a[1]),"timed_out":a[1] in ("124","137"),"settle_exit":int(a[4]),"secs":round(float(a[3])-float(a[2]),2),"compiled_units":len(c),"compiled":c,"test_binaries":n(a[6]),"linked_test_binaries":n(a[7]),"loadavg_before":a[8],"loadavg_after":a[9]}))' \
        "$rc" "$t0" "$t1" "$src" "$(compiled_names "$pre-build.log")" "$nm" "$nl" "$l0" "$(la)" "$c" "$f")"
    fi
    # edit_verify: 対象 crate を決め、その組で settle してから、編集 → --list → その側の nextest の行（--no-run）→ doc test の行。
    printf '%s' "$probe" >>"$tree/$f"
    (cd "$tree" && CELERIS_WU_BASE=HEAD bash scripts/dev/test-affected.sh --list) >"$pre-edit.list" 2>"$pre-edit.list.err"
    git -C "$tree" checkout -- "$f"
    aff="$(grep -v ':' "$pre-edit.list")"
    if [ -z "$aff" ]; then cskip "$side" "$c" edit_verify "test-affected.sh --list が crate を返さなかった（$pre-edit.list.err）"; return; fi
    t0="$(now)"; side_run "$side" nextest "$aff" >"$pre-edit-settle.log" 2>&1; src=$?
    if [ "$src" -ge 124 ] && [ "$src" -le 137 ]; then
      cskip "$side" "$c" edit_verify "settle の build が MEASURE_DEADLINE までに終わらない（exit $src）"; return
    fi
    l0="$(la)"; printf '%s' "$probe" >>"$tree/$f"; t1="$(now)"
    (cd "$tree" && CELERIS_WU_BASE=HEAD bash scripts/dev/test-affected.sh --list) >"$pre-edit.list2" 2>>"$pre-edit.list.err"; lrc=$?
    t2="$(now)"
    side_run "$side" nextest "$aff" >"$pre-edit.log" 2>&1; rc=$?; t3="$(now)"
    if [ "$rc" = 125 ]; then
      git -C "$tree" checkout -- "$f"
      cskip "$side" "$c" edit_verify "MEASURE_DEADLINE までの残りが 30 秒未満で、編集の後の再 build を走らせなかった（settle は済み）"; return
    fi
    side_run "$side" doc "$aff" >"$pre-doc.log" 2>&1; drc=$?; t4="$(now)"
    side_run "$side" list "$aff" >"$pre-edit.binaries.json" 2>/dev/null
    git -C "$tree" checkout -- "$f"; l1="$(la)"
    read -r nb nm nl na <<<"$(bins "$pre-edit.binaries.json" "$t2" "$(echo "$aff" | paste -sd,)")"
    cstep "$side" "$c" edit_verify "$(python3 -I - "$f" "$pre-edit.list2" "$lrc" "$rc" "$src" "$t0" "$t1" "$t2" "$t3" "$t4" "$drc" \
      "$(compiled_names "$pre-edit-settle.log")" "$(compiled_names "$pre-edit.log")" "$(compiled_names "$pre-doc.log")" \
      "$nb" "$nm" "$nl" "$na" "$l0" "$l1" "$pre-doc.log" "$raw/inv-$side.nextest" "$raw/inv-$side.doc" "$pre-edit-settle.log" <<'PY'
import json, re, sys
a = sys.argv
lines = open(a[2]).read().splitlines()
settle_c, reb_c, doc_c = json.loads(a[12]), json.loads(a[13]), json.loads(a[14])
n = lambda x: None if x == "null" else int(x)
doc = open(a[21], errors="replace").read()
res = [tuple(map(int, m.groups())) for m in re.finditer(r"test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored", doc)]
print(json.dumps({"edited_file": a[1], "affected": [l for l in lines if ":" not in l],
  "extra_checks": [l for l in lines if ":" in l],
  "cmd": open(a[22]).read().strip() + " --no-run", "doc_test_cmd": open(a[23]).read().strip(),
  "settle_exit": int(a[5]), "settle_secs": round(float(a[7]) - float(a[6]), 2), "settle_compiled_units": len(settle_c), "settle_compiled": settle_c,
  "list_exit": int(a[3]), "list_secs": round(float(a[8]) - float(a[7]), 2),
  "rebuild_exit": int(a[4]), "rebuild_timed_out": a[4] in ("124", "137"),
  "rebuild_secs": round(float(a[9]) - float(a[8]), 2),
  "rebuild_compiled_units": len(reb_c), "rebuild_compiled": reb_c,
  "listed_binaries": n(a[15]), "test_binaries": n(a[16]), "linked_test_binaries": n(a[17]), "linked_binaries_all": n(a[18]),
  "edit_to_verify_secs": round(float(a[9]) - float(a[7]), 2),
  "doc_test_exit": int(a[11]), "doc_test_secs": round(float(a[10]) - float(a[9]), 2),
  "doc_test_compiled_units": len(doc_c), "doc_test_passed": sum(r[0] for r in res), "doc_test_failed": sum(r[1] for r in res),
  "doc_test_ignored": sum(r[2] for r in res), "edit_to_verify_with_doc_secs": round(float(a[10]) - float(a[7]), 2),
  "loadavg_before": a[19], "loadavg_after": a[20]}))
PY
)"
  }
  i=0
  for c in "${crates[@]}"; do
    if [ $((i % 2)) -eq 0 ]; then sides="base head"; else sides="head base"; fi
    for side in $sides; do measure_one "$side" "$c"; done
    i=$((i + 1))
  done
  assemble_cmp
  cat "$out"
  validate "$out"; exit $?
fi

# ---- run モード ----------------------------------------------------------------------------------------
IFS=, read -ra crates <<<"$crates_csv"
baseline_in="${MEASURE_BASELINE:-$here/baseline.json}"
# label が base でない run の後段（bundles・base_run）。head だけが走らせ、ほかの label は skipped に理由を書く。
post_items=""; [ "$label" != base ] && post_items="bundles base_run"
skip_post() { local it; for it in $post_items; do skip "$1" "$it" "$2"; done; }

# nextest_result <log> <exit> <t0> <t1> <loadavg_before> <loadavg_after>: nextest の log から秒・件数・失敗した試験名の JSON。
nextest_result() {
  python3 -I - "$@" <<'PY'
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
}

# bundles <crate>: crates/<crate>/tests/*/main.rs の束ごとに cargo test -p <crate> --test <bundle> を 1 回（1 process）。
measure_bundles() {
  local c="$1" m b bl lg brc rc t0 t1 t2 l0 la0 found=() res="$raw/bundles-$c.jsonl"
  for m in crates/"$c"/tests/*/main.rs; do [ -f "$m" ] && found+=("$(basename "$(dirname "$m")")"); done
  : >"$res"; la0="$(la)"
  for b in "${found[@]}"; do
    bl="$raw/bundle-$c-$b.build.log"; lg="$raw/bundle-$c-$b.log"
    # build は先に済ませ、試験の秒に混ぜない。
    t0="$(now)"; timed 60 cargo test -p "$c" --test "$b" --no-run >"$bl" 2>&1; brc=$?; t1="$(now)"
    if [ "$brc" = 125 ] || [ "$brc" = 124 ] || [ "$brc" = 137 ]; then
      skip "$c" "bundles:$b" "束の build が MEASURE_DEADLINE までに終わらない（exit $brc）"; continue
    fi
    l0="$(la)"; t1="$(now)"
    if [ "$brc" = 0 ]; then timed 30 cargo test -p "$c" --test "$b" >"$lg" 2>&1; rc=$?; else rc="$brc"; : >"$lg"; fi
    t2="$(now)"
    if [ "$rc" = 125 ]; then skip "$c" "bundles:$b" "MEASURE_DEADLINE までの残りが 30 秒未満で走らせなかった（build は済み）"; continue; fi
    python3 -I - "$b" "$c" "$lg" "$rc" "$brc" "$t0" "$t1" "$t2" "$l0" "$(la)" >>"$res" <<'PY'
import json, re, sys
b, c, lg, rc, brc, t0, t1, t2, l0, l1 = sys.argv[1:11]
txt = open(lg, errors="replace").read()
res = [tuple(map(int, m.groups())) for m in re.finditer(r"^test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored", txt, re.M)]
print(json.dumps({"bundle": b, "data": {
  "cmd": f"cargo test -p {c} --test {b}", "build_exit": int(brc), "build_secs": round(float(t1) - float(t0), 2),
  "exit": int(rc), "timed_out": rc in ("124", "137"), "harness_finished": bool(res),
  "passed": res[-1][0] if res else None, "failed": res[-1][1] if res else None, "ignored": res[-1][2] if res else None,
  "failed_tests": sorted(set(re.findall(r"^test (\S+) \.\.\. FAILED$", txt, re.M))),
  "secs": round(float(t2) - float(t1), 2), "loadavg_before": l0, "loadavg_after": l1}}))
PY
  done
  step "$c" bundles "$(python3 -I - "$res" "$c" "$la0" "$(la)" "${found[@]}" <<'PY'
import json, sys
res, c, l0, l1, found = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5:]
r = {}
for line in open(res):
    s = json.loads(line); r[s["bundle"]] = s["data"]
print(json.dumps({"glob": f"crates/{c}/tests/*/main.rs", "cmd": f"cargo test -p {c} --test <bundle>",
                  "found": found, "results": r, "loadavg_before": l0, "loadavg_after": l1}))
PY
)"
}

# base_run <crate>: baseline と HEAD の run1・run2 の loadavg（1 分値）の最大を比べ、要る時だけ base で 1 回走らせる。
measure_base_run() {
  local c="$1" cmp prc rc l0 t0 t1 lg pl
  cmp="$(python3 -I - "$baseline_in" "$out" "$c" "$head_tree" <<'PY'
import json, os, sys
bp, op, c, top = sys.argv[1:5]
def la_max(m):
    v = [float(m[r][k].split()[0]) for r in ("run1", "run2") if isinstance(m.get(r), dict)
         for k in ("loadavg_before", "loadavg_after") if m[r].get(k)]
    return max(v) if v else None
try:
    bm, why = json.load(open(bp)).get("crates", {}).get(c), None
except Exception as e:
    bm, why = None, f"baseline を読めない: {e}"
b = la_max(bm) if bm else None
h = la_max(json.load(open(op))["crates"].get(c, {}))
ratio = round(h / b, 3) if (b and h is not None) else None
needed = b is None or (ratio is not None and ratio > 2)
reason = why or ("baseline に crate の run1・run2 の loadavg が無い" if b is None
                 else f"HEAD の 1 分値の最大 {h} は baseline の {b} の {ratio} 倍" + ("（2 倍を超える）" if needed else "（2 倍以下）"))
rel = os.path.relpath(os.path.realpath(bp), top)
print(json.dumps({"baseline": bp if rel.startswith("..") else rel, "baseline_loadavg_1m_max": b, "head_loadavg_1m_max": h, "ratio": ratio,
                  "threshold": 2, "needed": needed, "reason": reason}))
PY
)"
  if [ "$(python3 -I -c 'import json,sys; print(json.loads(sys.argv[1])["needed"])' "$cmp")" != True ]; then
    step "$c" base_run "$cmp"; return
  fi
  base_tree_add || { skip "$c" base_run "base の git worktree add に失敗した（$raw/worktree.log）"; return; }
  pl="$raw/base-prerun-$c.log"; lg="$raw/base-run-$c.log"
  t0="$(now)"; (cd "$base_tree" && timed 60 cargo nextest run -p "$c" --no-run) >"$pl" 2>&1; prc=$?; t1="$(now)"
  if [ "$prc" = 125 ] || [ "$prc" = 124 ] || [ "$prc" = 137 ]; then
    base_tree_remove; skip "$c" base_run "base の試験の前の build が MEASURE_DEADLINE までに終わらない（exit $prc）"; return
  fi
  local bsecs; bsecs="$(python3 -c 'import sys; print(round(float(sys.argv[2])-float(sys.argv[1]),2))' "$t0" "$t1")"
  l0="$(la)"; t0="$(now)"
  (cd "$base_tree" && timed 120 cargo nextest run -p "$c" --no-fail-fast) >"$lg" 2>&1; rc=$?; t1="$(now)"
  base_tree_remove
  if [ "$rc" = 125 ]; then skip "$c" base_run "MEASURE_DEADLINE までの残りが 120 秒未満で base を走らせなかった（build は済み）"; return; fi
  step "$c" base_run "$(python3 -I -c 'import json,sys; d=json.loads(sys.argv[1]); d.update(json.loads(sys.argv[2])); d.update({"base_sha":sys.argv[3],"cmd":"cargo nextest run -p "+sys.argv[4]+" --no-fail-fast（base の git worktree で。build は先に --no-run）","build_exit":int(sys.argv[5]),"build_secs":float(sys.argv[6])}); print(json.dumps(d))' \
    "$cmp" "$(nextest_result "$lg" "$rc" "$t0" "$t1" "$l0" "$(la)")" "$base_sha" "$c" "$prc" "$bsecs")"
}

for c in "${crates[@]}"; do
  step "$c" session "$(python3 -I -c 'import json,sys,time; print(json.dumps({"label":sys.argv[1],"git_head":sys.argv[2],"merge_base_main":sys.argv[3],"measured_src_equals_merge_base":sys.argv[4]=="true","started_at":time.strftime("%Y-%m-%dT%H:%M:%SZ",time.gmtime()),"loadavg_start":sys.argv[5],"cargo_target_dir":sys.argv[6]}))' "$label" "$head_sha" "$base_sha" "$src_eq" "$(la)" "${CARGO_TARGET_DIR:-}")"
  # build
  lg="$raw/build-$c.log"; l0="$(la)"; t0="$(now)"
  timed 60 cargo nextest run -p "$c" --no-run >"$lg" 2>&1; rc=$?
  t1="$(now)"
  if [ "$rc" = 125 ]; then
    for it in build edit_verify run1 run2 $post_items; do skip "$c" "$it" "MEASURE_DEADLINE までの残りが足りず走らせなかった"; done; continue
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
    for it in run1 run2 $post_items; do skip "$c" "$it" "試験の前の build が MEASURE_DEADLINE までに終わらない（exit $prc）"; done; continue
  fi
  for r in 1 2; do
    lg="$raw/run$r-$c.log"; l0="$(la)"; t0="$(now)"
    timed 120 cargo nextest run -p "$c" --no-fail-fast >"$lg" 2>&1; rc=$?
    t1="$(now)"
    if [ "$rc" = 125 ]; then skip "$c" "run$r" "MEASURE_DEADLINE までの残りが 120 秒未満で走らせなかった"; continue; fi
    step "$c" "run$r" "$(nextest_result "$lg" "$rc" "$t0" "$t1" "$l0" "$(la)")"
  done
  if [ "$label" = head ]; then
    measure_bundles "$c"
    measure_base_run "$c"
  elif [ -n "$post_items" ]; then
    skip_post "$c" "label が head でない（'$label'）ので、束の 1 process 実行と base の実行を走らせなかった"
  fi
done
assemble
cat "$out"
validate "$out"
