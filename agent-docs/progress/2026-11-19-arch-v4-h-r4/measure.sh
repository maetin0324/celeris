#!/bin/sh
# アーキ v4 H R4 の測定（POSIX sh。dash で動く）。作業ツリーでなく git grep <rev> で数える。
#
#   sh agent-docs/progress/2026-11-19-arch-v4-h-r4/measure.sh <rev>
#       3 行 `<metric> <value>` を出す（metric は rusqlite_prod_files・sql_string_prod_files・conn_open_files）
#   sh agent-docs/progress/2026-11-19-arch-v4-h-r4/measure.sh files <metric> <rev>
#       その metric に数えた file の一覧を出す
#
# 数え方は agent-docs/progress/2026-11-19-arch-v4-h/struct-store.md §6 の C4 の 2 つの git grep を写した
# （X・P の除外と crates/task-core/src/store/ の除外も同じ）。conn_open_files は同じ除外で
# `git grep -lE 'Connection::open|TransactionBehavior|busy_timeout'` の file。scope は prod-excl-task-core-store。
set -u

METRICS='rusqlite_prod_files sql_string_prod_files conn_open_files'
X='(^|/)(tests?|benches|examples)/|(^|/)(tests|[^/]*_tests?|test_support)\.rs$|_fixture\.rs$'
P='^crates/(campaign|routing-eval|jev-classifier)/'
SQL='"[^"]*\b(SELECT .* FROM|INSERT (OR [A-Z]+ )?INTO|UPDATE [a-z_]+ SET|DELETE FROM)\b'
CONN='Connection::open|TransactionBehavior|busy_timeout'

usage() {
    echo "usage: measure.sh <rev> | measure.sh files <metric> <rev>（metric: $METRICS）" >&2
    exit 2
}

top=$(git rev-parse --show-toplevel) || exit 2
cd "$top" || exit 2

resolve() {
    git rev-parse -q --verify "$1^{commit}" || { echo "measure.sh: rev を commit に解決できない: $1" >&2; exit 2; }
}

# list <metric> <sha>: 除外を当てた file の一覧（並びは git grep のまま = path の順）
list() {
    case $1 in
        rusqlite_prod_files) git grep -l 'rusqlite' "$2" -- 'crates/*.rs' ;;
        sql_string_prod_files) git grep -lE "$SQL" "$2" -- 'crates/*.rs' ;;
        conn_open_files) git grep -lE "$CONN" "$2" -- 'crates/*.rs' ;;
        *) usage ;;
    esac | sed "s/^$2://" | grep -vE "$X" | grep -vE "$P" | grep -v '^crates/task-core/src/store/'
    return 0
}

case $# in
    1)
        [ "$1" = files ] && usage
        sha=$(resolve "$1") || exit 2
        for m in $METRICS; do
            n=$(list "$m" "$sha" | wc -l)
            echo "$m $((n + 0))"
        done
        ;;
    3)
        [ "$1" = files ] || usage
        case " $METRICS " in *" $2 "*) ;; *) usage ;; esac
        sha=$(resolve "$3") || exit 2
        list "$2" "$sha"
        ;;
    *) usage ;;
esac
