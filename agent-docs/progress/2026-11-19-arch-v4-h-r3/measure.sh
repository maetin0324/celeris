#!/bin/sh
# アーキv4 H R3（task 01M7THYAPKESR1BG6BQZPJ749P）の測定の台本。
#
# 数え方は agent-docs/progress/2026-11-18-arch-v4-e/measure-commands.md の C3・E1・E2（dispatcher の self.store・self.config・
# SqliteStore）と A の C12（source-size-report。script と toml は base.txt の commit から git show で出す）をそのまま使う。
# 規則（除外の正規表現・論理 scope の 8 file・perl の正規表現）を変えない。
#
#   sh measure.sh rows <rev|.>
#       metric<TAB>scope<TAB>value<TAB>checker を出す。`.` は作業ツリー（一時の index で tree を作って同じ command に当てる）。
#       C12（repo/C12）の行は rev のときだけ出す。
#   sh measure.sh verify <tsv> <before-rev> <after-rev|->
#       rows を数え直し、tsv の before（と after。`-` なら見ない）と違う行・tsv に無い行・数え直しに無い tsv の行を出して exit 1。
#   sh measure.sh shrink [--allow-same] <path>...
#       base.txt の sha と作業ツリーを比べる。crates/task-dispatch/src の非試験 file の集合が変わった、またはどれかの file の
#       E1 の self.store か self.config の数が増えたら exit 1。--allow-same が無ければ、与えた path（file か dir）の
#       E1 の store＋config の合計が減っていないときも exit 1。
#   sh measure.sh gain <tsv>
#       tsv の before と after が R3 の成果の条件（dispatcher_files は同じ、store・config の出現は減る、*_files は増えない、
#       crate 全体の減りが request/C3 の減り以上、loc_product_lines は増えない）を満たせば exit 0。満たさない行を出して exit 1。
#
# exit: 0 = 条件を満たす、1 = 満たさない、2 = 使い方・引数の誤り、3 = 論理 scope を選び直す必要がある（#[path] の集合が base と違う）。
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(git -C "$HERE" rev-parse --show-toplevel)
cd "$ROOT"
BASE_FILE="$HERE/base.txt"

T=$(mktemp -d "${TMPDIR:-/tmp}/r3-measure.XXXXXX")
trap 'rm -rf "$T"' EXIT HUP INT TERM

# E の measure-commands.md の X3（path の除外）と C3 の git grep の出力に当てる除外
X3='(^|/)(tests?)(/|\.rs$)|_tests?\.rs$'
XG=':[^:]*((^|/)(tests?)(/|\.rs:)|_tests?\.rs:)'
C=crates/task-dispatch/src
D=$C/dispatcher
HR=$C/health_review
# 論理 scope（A の baseline.md §6 の 1）= dispatcher/ の非試験 file − lib.rs:47 の project_decider_ts.rs ＋ #[path] の 8 file
LOGICAL_DROP=$D/project_decider_ts.rs
LOGICAL_EXTRA='design_gate design_verdict plan_adoption policy_review worker_profile check_item review_limits inventory'

die() { echo "measure.sh: $*" >&2; exit 2; }

base_sha() {
    [ -f "$BASE_FILE" ] || die "base.txt が無い: $BASE_FILE"
    b=$(tr -d ' \n' < "$BASE_FILE")
    git rev-parse --verify --quiet "$b^{commit}" >/dev/null || die "base.txt の sha が commit でない: $b"
    echo "$b"
}

# rev を object id にする。`.` は作業ツリーの crates/task-dispatch を一時の index に add して write-tree した tree の id。
resolve() {
    if [ "$1" = . ]; then
        idx="$T/index"
        rm -f "$idx"
        src=$(git rev-parse --path-format=absolute --git-path index)
        if [ -f "$src" ]; then cp "$src" "$idx"; fi
        if [ ! -f "$idx" ]; then GIT_INDEX_FILE="$idx" git read-tree HEAD; fi
        GIT_INDEX_FILE="$idx" git add -A -- crates/task-dispatch
        GIT_INDEX_FILE="$idx" git write-tree
    else
        git rev-parse --verify --quiet "$1^{commit}" || die "rev が commit でない: $1"
    fi
}

# 非試験の #[path] の属性（file と行の中身。行番号は除く）。論理 scope の 8 file が選び直しなしで使えるかの guard。
path_attrs() {
    { git grep -o '#\[path = "[^"]*"\]' "$1" -- "$C" || true; } | { grep -vE "$XG" || true; } | sed "s|^$1:||" | LC_ALL=C sort
}

# rev（object id）の file を $T/f/<id>/<path> に git show で写す（git show $R:<path> と同じ内容）。
blob() {
    out="$T/f/$1/$2"
    if [ ! -f "$out" ]; then
        mkdir -p "$(dirname "$out")"
        git show "$1:$2" > "$out" || { echo "measure.sh: $1:$2 を読めない" >&2; exit 3; }
    fi
    echo "$out"
}

# E1 の perl（file 全体に self\s*\.\s*(store|config)\b）。空の file は perl が何も出さないので 0 0 にする。
e1_counts() {
    r=$(perl -0777 -ne '$s=()=/self\s*\.\s*store\b/g; $c=()=/self\s*\.\s*config\b/g; print "$s $c\n"' "$1")
    echo "${r:-0 0}"
}

row() { printf '%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4"; }

# scope.txt（dispatcher/ の非試験 file）と logical.txt を作る。#[path] の集合が base と違えば exit 3。
make_scopes() {
    R=$1
    git ls-tree -r --name-only "$R" "$D/" | { grep -vE "$X3" || true; } > "$T/scope.txt"
    { grep -v "^$LOGICAL_DROP\$" "$T/scope.txt" || true
      for f in $LOGICAL_EXTRA; do echo "$HR/$f.rs"; done; } > "$T/logical.txt"
    while IFS= read -r x; do
        git cat-file -e "$R:$x" 2>/dev/null || { echo "measure.sh: 論理 scope の file が $R に無い: $x（選び直す）" >&2; exit 3; }
    done < "$T/logical.txt"
    B=$(base_sha)
    if [ "$R" != "$B" ]; then
        path_attrs "$B" > "$T/attrs.base"
        path_attrs "$R" > "$T/attrs.rev"
        if ! cmp -s "$T/attrs.base" "$T/attrs.rev"; then
            echo "measure.sh: 非試験の #[path] の集合が base と違う。論理 scope を git grep -n '#\\[path' <rev> -- $C で選び直す:" >&2
            diff "$T/attrs.base" "$T/attrs.rev" >&2 || true
            exit 3
        fi
    fi
}

rows_dispatch() {
    R=$1
    make_scopes "$R"
    n_req=$(($(wc -l < "$T/scope.txt")))
    n_log=$(($(wc -l < "$T/logical.txt")))
    row dispatcher_files request/C3 "$n_req" a-c3
    row dispatcher_files request/E1 "$n_req" e-e1
    row dispatcher_files logical/C3 "$n_log" a-c3
    row dispatcher_files logical/E1 "$n_log" e-e1
    for p in store config; do
        # C3 依頼 scope: git grep -o（1 行の中の self.<p>）
        { git grep -o "self\\.$p" "$R" -- "$D/" || true; } | { grep -vE "$XG" || true; } > "$T/m"
        f=$(($(cut -d: -f2 "$T/m" | sort -u | wc -l))); o=$(($(wc -l < "$T/m")))
        row "dispatcher_${p}_files" request/C3 "$f" a-c3
        row "dispatcher_${p}_hits" request/C3 "$o" a-c3
        # C3 論理 scope: file ごとに git show | grep -o | wc -l
        while IFS= read -r x; do grep -o "self\\.$p" "$(blob "$R" "$x")" | wc -l; done < "$T/logical.txt" |
            awk -v p="$p" '{o+=$1; if($1>0)f++} END{printf "dispatcher_%s_files\tlogical/C3\t%d\ta-c3\ndispatcher_%s_hits\tlogical/C3\t%d\ta-c3\n", p, f, p, o}'
    done
    # E1（改行をまたぐ参照も数える）
    for L in scope logical; do
        case $L in scope) sc=request/E1 ;; *) sc=logical/E1 ;; esac
        while IFS= read -r x; do e1_counts "$(blob "$R" "$x")"; done < "$T/$L.txt" |
            awk -v sc="$sc" '{if($1>0)sf++; so+=$1; if($2>0)cf++; co+=$2}
                END{printf "dispatcher_store_files\t%s\t%d\te-e1\ndispatcher_store_hits\t%s\t%d\te-e1\n", sc, sf, sc, so;
                    printf "dispatcher_config_files\t%s\t%d\te-e1\ndispatcher_config_hits\t%s\t%d\te-e1\n", sc, cf, sc, co}'
    done
    # E2 crate 全体（task-dispatch/src の非試験）
    n_crate=$(($(git ls-tree -r --name-only "$R" "$C/" | { grep -vcE "$X3" || true; })))
    row dispatch_crate_files crate/E2 "$n_crate" e-e2
    for p in store config; do
        o=$(($({ git grep -o "self\\.$p" "$R" -- "$C/" || true; } | { grep -vE "$XG" || true; } | wc -l)))
        row "dispatch_crate_${p}_hits" crate/E2 "$o" e-e2
    done
    # E2 SqliteStore を書く file（依頼 scope・論理 scope）
    s_req=$(($({ git grep -l 'SqliteStore' "$R" -- "$D/" || true; } | sed "s|^$R:||" | { grep -vcE "$X3" || true; })))
    row dispatcher_sqlitestore_files request/E2 "$s_req" e-e2
    s_log=$(while IFS= read -r x; do grep -c SqliteStore "$(blob "$R" "$x")" || true; done < "$T/logical.txt" | awk '$1>0{f++} END{print f+0}')
    row dispatcher_sqlitestore_files logical/E2 "$s_log" e-e2
}

# A の C12（分類規則は base.txt の commit に固定）
rows_c12() {
    R=$1
    B=$(base_sha)
    git show "$B:scripts/dev/source-size-report.py" > "$T/ssr.py"
    git show "$B:scripts/dev/source-size-report.toml" > "$T/ssr.toml"
    python3 -I "$T/ssr.py" --root . --config "$T/ssr.toml" --exclusive-classes --rev "$R" --format json --out "$T/ssr.json" >/dev/null
    python3 -I -c '
import json, sys
v = json.load(open(sys.argv[1]))["views"]
for m, view, k in (("loc_product_lines", "product_only", "lines"), ("loc_product_rust_lines", "product_only", "rust_lines"),
                   ("loc_own_total_with_tests", "own_total_with_tests", "lines")):
    print(f"{m}\trepo/C12\t{int(v[view][k])}\ta-c12")
' "$T/ssr.json"
}

rows() {
    R=$(resolve "$1")
    rows_dispatch "$R"
    if [ "$1" != . ]; then rows_c12 "$R"; fi
}

# 非試験 file ごとの E1 の数（path<TAB>store<TAB>config）を crate 全体で出す
per_file_e1() {
    R=$1
    git ls-tree -r --name-only "$R" "$C/" | { grep -vE "$X3" || true; } | while IFS= read -r x; do
        set -- $(e1_counts "$(blob "$R" "$x")")
        printf '%s\t%s\t%s\n' "$x" "$1" "$2"
    done
}

cmd_shrink() {
    allow_same=0
    if [ "${1:-}" = --allow-same ]; then allow_same=1; shift; fi
    B=$(base_sha)
    W=$(resolve .)
    per_file_e1 "$B" > "$T/e1.base"
    per_file_e1 "$W" > "$T/e1.work"
    python3 -I -c '
import sys
def load(p):
    out = {}
    for line in open(p, encoding="utf-8"):
        f, s, c = line.rstrip("\n").split("\t")
        out[f] = (int(s), int(c))
    return out
base, work = load(sys.argv[1]), load(sys.argv[2])
allow_same = sys.argv[3] == "1"
paths = sys.argv[4:]
bad = []
for f in sorted(set(base) - set(work)):
    bad.append(f"removed\t{f}\t非試験 file が消えた")
for f in sorted(set(work) - set(base)):
    bad.append(f"added\t{f}\t非試験 file が増えた")
for f in sorted(set(base) & set(work)):
    (bs, bc), (ws, wc) = base[f], work[f]
    if ws > bs:
        bad.append(f"increased\t{f}\tself.store {bs} -> {ws}")
    if wc > bc:
        bad.append(f"increased\t{f}\tself.config {bc} -> {wc}")
def pick(p):
    p = p.rstrip("/")
    return [f for f in base if f == p or f.startswith(p + "/")]
sel = []
for p in paths:
    hit = pick(p)
    if not hit:
        print(f"measure.sh: path が base の crates/task-dispatch/src の非試験 file に当たらない: {p}", file=sys.stderr)
        sys.exit(2)
    sel.extend(h for h in hit if h not in sel)
bsum = sum(sum(base[f]) for f in sel)
wsum = sum(sum(work.get(f, (0, 0))) for f in sel)
if not allow_same and not wsum < bsum:
    bad.append(f"not-decreased\t{len(sel)} file\tE1 の store+config {bsum} -> {wsum}")
for b in bad:
    print(b)
status = "ok" if not bad else "fail"
print(f"shrink: {status} files {len(base)} -> {len(work)} paths {len(sel)} file store+config {bsum} -> {wsum}", file=sys.stderr)
sys.exit(1 if bad else 0)
' "$T/e1.base" "$T/e1.work" "$allow_same" "$@"
}

cmd_verify() {
    [ $# -eq 3 ] || die "使い方: sh measure.sh verify <tsv> <before-rev> <after-rev|->"
    tsv=$1; before=$2; after=$3
    [ -f "$tsv" ] || die "tsv が無い: $tsv"
    rows "$before" > "$T/rows.before"
    if [ "$after" = - ]; then : > "$T/rows.after"; else rows "$after" > "$T/rows.after"; fi
    python3 -I -c '
import sys
tsv, rb, ra, before_rev, after_rev = sys.argv[1:6]
def load_rows(p):
    out = {}
    for line in open(p, encoding="utf-8"):
        m, s, v, c = line.rstrip("\n").split("\t")
        out[(m, s)] = (v, c)
    return out
lines = open(tsv, encoding="utf-8").read().splitlines()
head = lines[0].split("\t")
want = ["metric", "scope", "before", "after", "checker", "note"]
bad = []
if head != want:
    bad.append("header\t" + "\t".join(head) + "\t期待: " + " ".join(want))
t = {}
for n, line in enumerate(lines[1:], 2):
    cells = line.split("\t")
    if len(cells) != 6:
        bad.append(f"malformed\tline {n}\t列が {len(cells)} 個")
        continue
    key = (cells[0], cells[1])
    if key in t:
        bad.append(f"duplicate\t{cells[0]}\t{cells[1]}")
    t[key] = cells
b, a = load_rows(rb), load_rows(ra)
for key, (v, c) in b.items():
    if key not in t:
        bad.append(f"missing\t{key[0]}\t{key[1]}\tcount={v}")
        continue
    if t[key][2] != v:
        bad.append(f"before\t{key[0]}\t{key[1]}\ttsv={t[key][2]}\tcount={v}")
    if t[key][4] != c:
        bad.append(f"checker\t{key[0]}\t{key[1]}\ttsv={t[key][4]}\tcount={c}")
if before_rev != ".":
    for key in t:
        if key not in b:
            bad.append(f"extra\t{key[0]}\t{key[1]}\t数え直しに無い行")
for key, (v, c) in a.items():
    if key in t and t[key][3] != v:
        bad.append(f"after\t{key[0]}\t{key[1]}\ttsv={t[key][3]}\tcount={v}")
for x in bad:
    print(x)
status = "ok" if not bad else "fail"
print(f"verify: {status} rows {len(b)} before={before_rev} after={after_rev}", file=sys.stderr)
sys.exit(1 if bad else 0)
' "$tsv" "$T/rows.before" "$T/rows.after" "$before" "$after"
}

cmd_gain() {
    [ $# -eq 1 ] || die "使い方: sh measure.sh gain <tsv>"
    [ -f "$1" ] || die "tsv が無い: $1"
    python3 -I -c '
import sys
t = {}
for line in open(sys.argv[1], encoding="utf-8").read().splitlines()[1:]:
    c = line.split("\t")
    if len(c) >= 4:
        t[(c[0], c[1])] = (c[2], c[3])
bad = []
def num(m, s):
    if (m, s) not in t:
        bad.append(f"missing\t{m}\t{s}")
        return None
    b, a = t[(m, s)]
    try:
        return int(b), int(a)
    except ValueError:
        bad.append(f"not-number\t{m}\t{s}\tbefore={b}\tafter={a}")
        return None
S4 = ("request/C3", "request/E1", "logical/C3", "logical/E1")
for s in S4:
    v = num("dispatcher_files", s)
    if v and v[1] != v[0]:
        bad.append(f"files-changed\tdispatcher_files\t{s}\t{v[0]} -> {v[1]}")
    for m in ("dispatcher_store_hits", "dispatcher_config_hits"):
        v = num(m, s)
        if v and not v[1] < v[0]:
            bad.append(f"not-decreased\t{m}\t{s}\t{v[0]} -> {v[1]}")
for (m, s) in sorted(t):
    if m.endswith("_files"):
        v = num(m, s)
        if v and v[1] > v[0]:
            bad.append(f"increased\t{m}\t{s}\t{v[0]} -> {v[1]}")
for crate_m, disp_m in (("dispatch_crate_store_hits", "dispatcher_store_hits"), ("dispatch_crate_config_hits", "dispatcher_config_hits")):
    vc, vd = num(crate_m, "crate/E2"), num(disp_m, "request/C3")
    if vc and vd and (vc[0] - vc[1]) < (vd[0] - vd[1]):
        bad.append(f"moved-out\t{crate_m}\tcrate/E2\t減り {vc[0] - vc[1]} < {disp_m} request/C3 の減り {vd[0] - vd[1]}")
v = num("loc_product_lines", "repo/C12")
if v and v[1] > v[0]:
    bad.append(f"increased\tloc_product_lines\trepo/C12\t{v[0]} -> {v[1]}")
for x in bad:
    print(x)
print("gain: " + ("ok" if not bad else "fail"), file=sys.stderr)
sys.exit(1 if bad else 0)
' "$1"
}

[ $# -ge 1 ] || die "使い方: sh measure.sh rows <rev|.> | verify <tsv> <before-rev> <after-rev|-> | shrink [--allow-same] <path>... | gain <tsv>"
mode=$1; shift
case $mode in
    rows) [ $# -eq 1 ] || die "使い方: sh measure.sh rows <rev|.>"; rows "$1" ;;
    verify) cmd_verify "$@" ;;
    shrink) cmd_shrink "$@" ;;
    gain) cmd_gain "$@" ;;
    *) die "mode が無い: $mode（rows | verify | shrink | gain）" ;;
esac
