#!/bin/sh
# アーキ v4 H R5 の測定（POSIX sh。dash で動く）。作業ツリーでなく git grep <rev> と git show <rev>:<path> で数える。
#
#   sh agent-docs/progress/2026-11-19-arch-v4-h-r5/measure.sh <rev>
#       3 行 `<metric> <value>` を出す（metric は applied_wide・applied_narrow・applied_functions）
#   sh agent-docs/progress/2026-11-19-arch-v4-h-r5/measure.sh files <metric> <rev>
#       その metric に数えたものを sort して 1 行ずつ出す（applied_wide・applied_narrow は file の path、
#       applied_functions は `<path> <fn>`）。行数は `measure.sh <rev>` の値と等しい
#
# 数え方は agent-docs/progress/2026-11-19-arch-v4-h/struct-store.md §6 の「Applied の形（D の付記）」の 2 つの git grep と、
# 「# 補助: file ごとの関数」の awk を写した（awk の program は変えていない）。scope は crates/task-api/src。
#   applied_wide      git grep -lE "Option<&('[a-z_]+ )?(crate::cos::operations::)?OperationAudit>" <rev> -- crates/task-api/src
#   applied_narrow    git grep -l 'Option<&OperationAudit>' <rev> -- crates/task-api/src
#   applied_functions applied_wide の file ごとに awk で `<path> <fn>` を出し sort -u した行
set -u
LC_ALL=C
export LC_ALL

METRICS='applied_wide applied_narrow applied_functions'
WIDE="Option<&('[a-z_]+ )?(crate::cos::operations::)?OperationAudit>"
NARROW='Option<&OperationAudit>'

usage() {
    echo "usage: measure.sh <rev> | measure.sh files <metric> <rev>（metric: $METRICS）" >&2
    exit 2
}

top=$(git rev-parse --show-toplevel) || exit 2
cd "$top" || exit 2

resolve() {
    git rev-parse -q --verify "$1^{commit}" || { echo "measure.sh: rev を commit に解決できない: $1" >&2; exit 2; }
}

# list <metric> <sha>: 数える対象を sort して出す
list() {
    B=$2
    case $1 in
        applied_wide) git grep -lE "$WIDE" "$B" -- crates/task-api/src | sed "s/^$B://" | sort ;;
        applied_narrow) git grep -l "$NARROW" "$B" -- crates/task-api/src | sed "s/^$B://" | sort ;;
        applied_functions)
            # struct-store.md §6 の「# 補助: file ごとの関数（type Audit の行は除く）」の for と awk をそのまま写した
            for f in $(git grep -lE "Option<&('[a-z_]+ )?(crate::cos::operations::)?OperationAudit>" $B -- crates/task-api/src | sed "s/^$B://"); do
              git show $B:$f | awk -v f=$f '/^[[:space:]]*(pub(\([a-z]+\))? )?(async )?fn [a-z_0-9]+/ {match($0,/fn [a-z_0-9]+/); fn=substr($0,RSTART+3,RLENGTH-3)}
                /^type Audit</ {next}
                /Option<&('"'"'[a-z_]+ )?(crate::cos::operations::)?OperationAudit>/ || /: Audit<'"'"'_>/ {print f, fn}'; done | sort -u
            ;;
        *) usage ;;
    esac
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
