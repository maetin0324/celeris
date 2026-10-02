#!/bin/sh
# docs の参照検査（ADR-0128 D7）。POSIX sh（dash で動く）。
#
#   sh scripts/dev/check-doc-links.sh              リポジトリ全体
#   sh scripts/dev/check-doc-links.sh <path>...    その範囲だけ（ディレクトリ可）
#   sh scripts/dev/check-doc-links.sh --self-test  scripts/dev/testdata/doc-links/ の fixture を検査
#
# (1) 追跡されている全 .md の Markdown 相対リンク [..](target) の実在
# (2) 生きた参照（CLAUDE.md, AGENTS.md, crates/**/*.rs, scripts/**, .claude/**,
#     docs/architecture-map.md, web/・gui/ のソース）に出る docs/… と agent-docs/… の実在
# 壊れた参照は「ファイル:行: パス」で標準出力に出し、1 件でもあれば exit 1。
set -u

# 移行期間（ADR-0128 D6）の対象外。期間が終わったら空にする。
# check-adr-numbers.sh は移行期間中だけ旧 ADR ディレクトリ（存在するものだけ）も数えるので、旧パスを名指しする。
MIGRATION_EXCLUDE="agent-docs/PROGRESS.md agent-docs/progress/phase-F.md scripts/dev/check-adr-numbers.sh"

# 素のパスを見ない・Markdown も見ないもの（前方一致）。
ALWAYS_EXCLUDE="scripts/dev/testdata/ scripts/dev/docs-layout.tsv scripts/dev/check-doc-links.sh"

# docs/… が「このリポジトリ」ではなく別の文書リポジトリ・試験の作業場所を指すファイル（前方一致）。
# celeris の文書機能（docs/ を持つ案件リポジトリ）と、その試験の fixture。(2) の素のパスを見ない。
FOREIGN_DOCS="crates/task-api/src/docs.rs crates/task-api/tests/docs.rs crates/task-ops/src/docs.rs
crates/task-ops/src/docs/ crates/task-ops/src/docs_maintenance/ crates/celeris/src/doc_gardener/tests.rs
crates/task-dispatch/src/undeclared_artifacts/tests.rs crates/task-worker/src/preamble/tests.rs
crates/task-core/src/execution/tests.rs scripts/tests/test_source_size_report.py
gui/app/lib/docs.ts gui/test/ gui/scripts/lib/celeris-fixture.mjs web/e2e/"

# progress・ADR・report の本文の素のパスは履歴なので、(2) は次の形のファイルだけ見る。
is_live_ref() {
    for e in $FOREIGN_DOCS; do
        case "$1" in
            "$e" | "$e"*) return 1 ;;
        esac
    done
    case "$1" in
        CLAUDE.md | AGENTS.md | */CLAUDE.md | */AGENTS.md) return 0 ;;
        docs/architecture-map.md) return 0 ;;
        crates/*.rs) return 0 ;;
        scripts/*) return 0 ;;
        .claude/*) return 0 ;;
        web/* | gui/*)
            case "$1" in
                */node_modules/* | */dist/* | */build/* | */.react-router/* | *pnpm-lock.yaml) return 1 ;;
                *.ts | *.tsx | *.js | *.jsx | *.mjs | *.cjs | *.json | *.html | *.css | *.sh | *.toml | *.yaml | *.yml) return 0 ;;
            esac
            return 1
            ;;
    esac
    return 1
}

excluded() {
    for e in $ALWAYS_EXCLUDE $MIGRATION_EXCLUDE; do
        case "$1" in
            "$e" | "$e"*) return 0 ;;
        esac
    done
    return 1
}

# 範囲（引数）に入るか。引数なしなら全部。
in_scope() {
    [ -z "$SCOPE" ] && return 0
    for s in $SCOPE; do
        case "$1" in
            "$s" | "$s"/*) return 0 ;;
        esac
        [ "$s" = "." ] && return 0
    done
    return 1
}

list_files() {
    if [ -n "${DOC_LINKS_FIXTURE:-}" ]; then
        find . -type f ! -name expected.txt | sed 's|^\./||' | LC_ALL=C sort
    else
        git -c core.quotePath=false ls-files
    fi
}

# Markdown リンクを抜き出す: 「file:line<TAB>解決後のパス<TAB>書かれたパス」
extract_md_links() {
    awk '
    function norm(p,   n, a, i, out, k, st) {
        n = split(p, a, "/")
        k = 0
        for (i = 1; i <= n; i++) {
            if (a[i] == "" || a[i] == ".") continue
            if (a[i] == ".." && k > 0 && st[k] != "..") { k--; continue }
            st[++k] = a[i]
        }
        out = ""
        for (i = 1; i <= k; i++) out = (out == "" ? st[i] : out "/" st[i])
        return out == "" ? "." : out
    }
    FNR == 1 { infence = 0; dir = FILENAME; if (sub(/\/[^\/]*$/, "", dir) == 0) dir = "" }
    /^[ \t]*(```|~~~)/ { infence = !infence; next }
    infence { next }
    {
        line = $0
        gsub(/`[^`]*`/, "", line)
        while (match(line, /\]\([^)]*\)/)) {
            t = substr(line, RSTART + 2, RLENGTH - 3)
            line = substr(line, RSTART + RLENGTH)
            sub(/^[ \t]+/, "", t)
            sub(/[ \t].*$/, "", t)
            if (t == "" || t ~ /^#/ || t ~ /^[A-Za-z][A-Za-z0-9+.-]*:/) continue
            if (t ~ /NNNN|YYYY|xxx|\.\.\.|[<>*{}$]/) continue
            raw = t
            sub(/[#?].*$/, "", t)
            if (t == "") continue
            if (t ~ /^\//) r = norm(substr(t, 2))
            else r = norm((dir == "" ? "" : dir "/") t)
            print FILENAME ":" FNR "\t" r "\t" raw
        }
    }' "$@"
}

# 素のパスを抜き出す: 「file:line<TAB>書かれたパス<TAB>候補1<TAB>候補2<TAB>候補3」
extract_raw_refs() {
    awk '
    function norm(p,   n, a, i, out, k, st) {
        n = split(p, a, "/")
        k = 0
        for (i = 1; i <= n; i++) {
            if (a[i] == "" || a[i] == ".") continue
            if (a[i] == ".." && k > 0 && st[k] != "..") { k--; continue }
            st[++k] = a[i]
        }
        out = ""
        for (i = 1; i <= k; i++) out = (out == "" ? st[i] : out "/" st[i])
        return out == "" ? "." : out
    }
    FNR == 1 {
        dir = FILENAME; if (sub(/\/[^\/]*$/, "", dir) == 0) dir = ""
        top = FILENAME; if (sub(/\/.*$/, "", top) == 0) top = ""
    }
    {
        line = $0
        while (match(line, /(\.\.?\/)*(agent-docs|docs)\/[A-Za-z0-9._\/-]*/)) {
            s = RSTART; l = RLENGTH
            pre = (s > 1) ? substr(line, s - 1, 1) : ""
            m = substr(line, s, l)
            post = substr(line, s + l, 1)
            line = substr(line, s + l)
            if (pre ~ /[A-Za-z0-9_.\/-]/) continue
            if (post ~ /[<{*$]/ || m ~ /NNNN|YYYY|xxx|\.\.\./) continue
            while (m ~ /[.\/]$/ && m !~ /\.\.$/) m = substr(m, 1, length(m) - 1)
            if (m ~ /^\.\.?\//) {
                c1 = norm((dir == "" ? "" : dir "/") m); c2 = c1; c3 = c1
            } else {
                c1 = norm(m)
                c2 = (dir == "" ? c1 : norm(dir "/" m))
                c3 = (top == "" ? c1 : norm(top "/" m))
            }
            print FILENAME ":" FNR "\t" m "\t" c1 "\t" c2 "\t" c3
        }
    }' "$@"
}

TAB=$(printf '\t')

# 実在するか。ADR を番号だけで呼ぶ docs/adr/0008・docs/adr/0040- は「番号-*」が 1 本あれば実在とみなす。
ref_exists() {
    [ -e "$1" ] && return 0
    case "${1##*/}" in
        [0-9][0-9][0-9][0-9] | [0-9][0-9][0-9][0-9]- | web-[0-9][0-9][0-9][0-9] | web-[0-9][0-9][0-9][0-9]-)
            for g in "${1%-}"-*; do
                [ -e "$g" ] && return 0
            done
            ;;
    esac
    return 1
}

# 現在のディレクトリを root として検査し、壊れた参照を出す。exit は 0/1。
run_check() {
    md_list=$(mktemp) || exit 2
    raw_list=$(mktemp) || exit 2
    out=$(mktemp) || exit 2
    list_files | while IFS= read -r f; do
        [ -f "$f" ] || continue
        excluded "$f" && continue
        in_scope "$f" || continue
        case "$f" in
            *.md) printf '%s\n' "$f" >>"$md_list" ;;
        esac
        if is_live_ref "$f"; then printf '%s\n' "$f" >>"$raw_list"; fi
    done
    if [ -s "$md_list" ]; then
        while IFS="$TAB" read -r loc res raw; do
            [ -n "$loc" ] || continue
            [ -e "$res" ] || printf '%s: %s\n' "$loc" "$raw" >>"$out"
        done <<EOF
$(md_files_args <"$md_list")
EOF
    fi
    if [ -s "$raw_list" ]; then
        while IFS="$TAB" read -r loc raw c1 c2 c3; do
            [ -n "$loc" ] || continue
            ref_exists "$c1" || ref_exists "$c2" || ref_exists "$c3" || printf '%s: %s\n' "$loc" "$raw" >>"$out"
        done <<EOF
$(raw_files_args <"$raw_list")
EOF
    fi
    LC_ALL=C sort -t: -k1,1 -k2,2n -s "$out"
    n=$(wc -l <"$out" | tr -d ' ')
    rm -f "$md_list" "$raw_list" "$out"
    if [ "$n" -gt 0 ]; then
        echo "check-doc-links: ${n} broken reference(s)" >&2
        return 1
    fi
    echo "check-doc-links: ok" >&2
    return 0
}

# ファイル一覧（標準入力）を引数に分けて awk に渡す。
md_files_args() {
    set --
    while IFS= read -r f; do set -- "$@" "$f"; done
    [ $# -gt 0 ] && extract_md_links "$@"
}

raw_files_args() {
    set --
    while IFS= read -r f; do set -- "$@" "$f"; done
    [ $# -gt 0 ] && extract_raw_refs "$@"
}

self_test() {
    base="$ROOT/scripts/dev/testdata/doc-links"
    fail=0
    for case_dir in "$base"/good "$base"/bad; do
        name=${case_dir##*/}
        if [ ! -d "$case_dir" ]; then
            echo "self-test: missing fixture $case_dir"
            return 1
        fi
        got=$(cd "$case_dir" && DOC_LINKS_FIXTURE=1 SCOPE="" run_check 2>/dev/null)
        rc=$?
        if [ "$name" = good ]; then want_rc=0; else want_rc=1; fi
        if [ -f "$case_dir/expected.txt" ]; then want=$(cat "$case_dir/expected.txt"); else want=""; fi
        if [ "$rc" -ne "$want_rc" ]; then
            echo "self-test: $name: exit $rc, want $want_rc"
            fail=1
        fi
        if [ "$got" != "$want" ]; then
            echo "self-test: $name: output differs"
            echo "--- want"
            printf '%s\n' "$want"
            echo "--- got"
            printf '%s\n' "$got"
            fail=1
        fi
    done
    if [ "$fail" -eq 0 ]; then
        echo "check-doc-links self-test: ok"
    fi
    return "$fail"
}

ROOT=$(git rev-parse --show-toplevel 2>/dev/null) || {
    echo "check-doc-links: not in a git repository" >&2
    exit 2
}

if [ "${1:-}" = "--self-test" ]; then
    self_test
    exit $?
fi

cd "$ROOT" || exit 2
SCOPE=""
for a in "$@"; do
    a=${a#./}
    while [ "${a%/}" != "$a" ]; do a=${a%/}; done
    [ -z "$a" ] && a="."
    if [ ! -e "$a" ]; then
        echo "check-doc-links: no such path: $a" >&2
        exit 2
    fi
    SCOPE="$SCOPE $a"
done
run_check
exit $?
