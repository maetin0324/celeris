#!/bin/sh
# 進捗ファイルの索引（ADR-0128 D3・D4）。POSIX sh（dash で動く）。索引は commit せず、都度ここで生成する。
#
#   sh scripts/dev/progress-index.sh          現在地: agent-docs/progress/ の front matter 付きファイルを
#                                             「updated | status | title | tasks | path」で出す（running・blocked が先、updated 降順）
#   sh scripts/dev/progress-index.sh --check  YYYY-MM-DD-*.md の front matter（title, tasks, status, updated）を検査し、
#                                             違反を「path:1: 理由」で出して exit 1
#
# agent-docs/progress/ が無ければ何も出さずに exit 0。旧ファイル（phase-*.md など）は --check の対象外。
set -u

DIR=agent-docs/progress

ROOT=$(git rev-parse --show-toplevel 2>/dev/null) || {
    echo "progress-index: not in a git repository" >&2
    exit 2
}
cd "$ROOT" || exit 2

mode=list
case "${1:-}" in
    "") ;;
    --check) mode=check ;;
    *)
        echo "usage: sh scripts/dev/progress-index.sh [--check]" >&2
        exit 2
        ;;
esac

[ -d "$DIR" ] || exit 0

# front matter を読み「title US tasks US status US updated US 有無」を出す（US = \037。無い欄は空。
# TAB は IFS の空白扱いで空欄が詰まるので使わない）。
front_matter() {
    awk '
        NR == 1 { if ($0 != "---") { fm = 0; exit } fm = 1; next }
        $0 == "---" { closed = 1; exit }
        {
            k = $0; sub(/:.*$/, "", k)
            v = $0; if (sub(/^[^:]*:[ \t]*/, "", v) == 0) next
            sub(/[ \t]+$/, "", v)
            if (k == "title" || k == "tasks" || k == "status" || k == "updated") val[k] = v
        }
        END {
            printf "%s\037%s\037%s\037%s\037%s\n", val["title"], val["tasks"], val["status"], val["updated"], (fm && closed) ? "yes" : "no"
        }' "$1"
}

TAB=$(printf '\t')
US=$(printf '\037')

if [ "$mode" = check ]; then
    out=$(mktemp) || exit 2
    find "$DIR" -type f -name '*.md' | LC_ALL=C sort | while IFS= read -r f; do
        case "${f##*/}" in
            [0-9][0-9][0-9][0-9]-[0-1][0-9]-[0-3][0-9]-?*.md) ;;
            *) continue ;;
        esac
        IFS="$US" read -r title tasks status updated has <<EOF
$(front_matter "$f")
EOF
        if [ "$has" != yes ]; then
            echo "$f:1: front matter（--- で囲んだ title, tasks, status, updated）が無い" >>"$out"
            continue
        fi
        [ -n "$title" ] || echo "$f:1: title が無い" >>"$out"
        case "$tasks" in
            \[?*\]) ;;
            *) echo "$f:1: tasks が [<task id>, ...] の形でない" >>"$out" ;;
        esac
        case "$status" in
            running | done | blocked | abandoned) ;;
            *) echo "$f:1: status が running | done | blocked | abandoned のどれでもない（${status:-空}）" >>"$out" ;;
        esac
        case "$updated" in
            [0-9][0-9][0-9][0-9]-[0-1][0-9]-[0-3][0-9]) ;;
            *) echo "$f:1: updated が YYYY-MM-DD でない（${updated:-空}）" >>"$out" ;;
        esac
    done
    cat "$out"
    n=$(wc -l <"$out" | tr -d ' ')
    rm -f "$out"
    if [ "$n" -gt 0 ]; then
        exit 1
    fi
    echo "progress-index --check: ok" >&2
    exit 0
fi

find "$DIR" -type f -name '*.md' | LC_ALL=C sort | while IFS= read -r f; do
    IFS="$US" read -r title tasks status updated has <<EOF
$(front_matter "$f")
EOF
    [ "$has" = yes ] || continue
    case "$status" in
        running | blocked) rank=0 ;;
        *) rank=1 ;;
    esac
    printf '%s\t%s\t%s | %s | %s | %s | %s\n' "$rank" "${updated:--}" "${updated:--}" "${status:--}" "${title:--}" "${tasks:--}" "$f"
done | LC_ALL=C sort -t "$TAB" -k1,1 -k2,2r -s | cut -f3-
