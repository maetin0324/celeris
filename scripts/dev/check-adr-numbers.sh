#!/bin/sh
# ADR の名前と採番の検査（ADR-0128 D5・D7）。POSIX sh（dash で動く）。
#
#   sh scripts/dev/check-adr-numbers.sh          作業ツリーの ADR を検査
#   sh scripts/dev/check-adr-numbers.sh --refs   この branch で足した ADR と、main・celeris/*・celeris-wu/* の
#                                                tree にある ADR とで番号・日付+slug が重なるものを出す
#
# 名前空間ごとに新旧のディレクトリを合わせて 1 つとして数える:
#   main: agent-docs/adr/ + docs/adr/   gui: agent-docs/gui/adr/ + docs/gui/adr/   web: agent-docs/web/adr/ + docs/web/adr/
# 違反は「ファイル:1: 内容」で標準出力に出し、1 件でもあれば exit 1。
set -u

# 既存の重複（振り直さない。ADR-0128 D5）。完全なファイル名で書く。
ALLOWED_DUPLICATES="
0078-browser-execution-capability.md
0078-ssh-master-persist-independent-of-daemon.md
0116-browser-launcher-implementation.md
0116-browser-prod-admission-confidential-release.md
0124-atomic-direct-route.md
0124-claude-session-resume.md
"

# main の名前空間で番号付きの最後（これを超える番号は新設禁止）。
LAST_NUMBERED=0128

# ADR-0128 の取り込み前に main へ入っていた 0128 超えの番号（振り直さない。ADR-0128 D5 付記）。完全なファイル名で書く。
ALLOWED_OVER_LAST="
0129-host-sccache-reflink-targets.md
0132-provider-llm-source-split-and-cheap-qwen.md
0133-inbox-and-notifications.md
0134-blocked-repair-replan-loop.md
0135-web-follow-health-gate.md
0136-local-hot-data-layout.md
0138-browser-prod-admission-confidential-release.md
0139-langmem-proxy-bearer-and-verify-proxy-bind.md
0131-cron-jobs.md
"

NAMESPACES="main gui web"
TAB=$(printf '\t')

ns_dirs() {
    case "$1" in
        main) echo "agent-docs/adr docs/adr" ;;
        gui) echo "agent-docs/gui/adr docs/gui/adr" ;;
        web) echo "agent-docs/web/adr docs/web/adr" ;;
    esac
}

# パスから名前空間を返す（ADR のディレクトリでなければ空）。
ns_of() {
    case "$1" in
        agent-docs/adr/* | docs/adr/*) echo main ;;
        agent-docs/gui/adr/* | docs/gui/adr/*) echo gui ;;
        agent-docs/web/adr/* | docs/web/adr/*) echo web ;;
    esac
}

is_allowed_over_last() {
    for a in $ALLOWED_OVER_LAST; do
        [ "$a" = "$1" ] && return 0
    done
    return 1
}

is_allowed() {
    for a in $ALLOWED_DUPLICATES; do
        [ "$a" = "$1" ] && return 0
    done
    return 1
}

# 名前の形を判定し「kind<TAB>key」を出す。kind = date | num | bad。
# key: date は slug、num は番号（web- 付きはそのまま）。
classify() {
    ns=$1
    name=$2
    case "$name" in
        [0-9][0-9][0-9][0-9]-[0-1][0-9]-[0-3][0-9]-?*.md)
            slug=${name#??????????-}
            slug=${slug%.md}
            case "$slug" in
                *[!a-z0-9-]* | -* | *-) printf 'bad\t-\n' ;;
                *) printf 'date\t%s\n' "$slug" ;;
            esac
            return
            ;;
        [0-9][0-9][0-9][0-9]-?*.md)
            printf 'num\t%s\n' "${name%%-*}"
            return
            ;;
        web-[0-9][0-9][0-9][0-9]-?*.md)
            if [ "$ns" != main ]; then
                rest=${name#web-}
                printf 'num\tweb-%s\n' "${rest%%-*}"
                return
            fi
            ;;
    esac
    printf 'bad\t-\n'
}

# 標準入力の「path」一覧を検査する。違反を出し、件数を戻り値（0/1）にする。
check_list() {
    tmp=$(mktemp) || exit 2
    out=$(mktemp) || exit 2
    while IFS= read -r p; do
        [ -n "$p" ] || continue
        name=${p##*/}
        [ "$name" = README.md ] && continue
        ns=$(ns_of "$p")
        [ -n "$ns" ] || continue
        IFS="$TAB" read -r kind key <<EOF
$(classify "$ns" "$name")
EOF
        case "$kind" in
            bad)
                echo "$p:1: 名前が NNNN-<slug>.md でも YYYY-MM-DD-<slug>.md でもない（新しい ADR は YYYY-MM-DD-<slug>.md）" >>"$out"
                continue
                ;;
            num)
                if [ "$ns" = main ] && [ "$key" -gt "$LAST_NUMBERED" ] 2>/dev/null && ! is_allowed_over_last "$name"; then
                    echo "$p:1: 番号 $key は $LAST_NUMBERED を超える（新しい ADR は YYYY-MM-DD-<slug>.md で書く）" >>"$out"
                fi
                ;;
        esac
        printf '%s\t%s\t%s\t%s\n' "$ns" "$kind" "$key" "$p" >>"$tmp"
    done
    # 同じ名前空間・同じ key が 2 本以上。
    LC_ALL=C sort "$tmp" | awk -F '\t' '
        { k = $1 "\t" $2 "\t" $3; n[k]++; paths[k] = paths[k] (n[k] > 1 ? " " : "") $4 }
        END { for (k in n) if (n[k] > 1) print k "\t" paths[k] }' | LC_ALL=C sort |
        while IFS="$TAB" read -r ns kind key paths; do
            ok=1
            names=""
            for p in $paths; do
                b=${p##*/}
                case " $names " in *" $b "*) ok=0 ;; esac
                names="$names $b"
                if [ "$kind" != num ] || ! is_allowed "$b"; then ok=0; fi
            done
            [ "$ok" -eq 1 ] && continue
            for p in $paths; do
                if [ "$kind" = num ]; then
                    echo "$p:1: ADR 番号 $key が重複（$ns:$(echo $paths | tr ' ' ',')）。後から入る側を YYYY-MM-DD-<slug>.md へ振り直す" >>"$out"
                else
                    echo "$p:1: ADR の slug $key が重複（$ns:$(echo $paths | tr ' ' ',')）" >>"$out"
                fi
            done
        done
    cat "$out"
    n=$(wc -l <"$out" | tr -d ' ')
    rm -f "$tmp" "$out"
    [ "$n" -eq 0 ]
}

worktree_adrs() {
    for ns in $NAMESPACES; do
        for d in $(ns_dirs "$ns"); do
            [ -d "$d" ] || continue
            for f in "$d"/*.md; do
                [ -f "$f" ] && printf '%s\n' "$f"
            done
        done
    done
}

all_adr_dirs() {
    for ns in $NAMESPACES; do ns_dirs "$ns"; done | tr ' ' '\n'
}

check_refs() {
    base=$(git merge-base HEAD refs/heads/main 2>/dev/null) || {
        echo "check-adr-numbers: refs/heads/main との merge-base が取れない" >&2
        return 2
    }
    dirs=$(all_adr_dirs)
    # shellcheck disable=SC2086
    added=$( {
        git -c core.quotePath=false diff --name-only --diff-filter=AR "$base" -- $dirs
        git -c core.quotePath=false ls-files --others --exclude-standard -- $dirs
    } | LC_ALL=C sort -u)
    if [ -z "$added" ]; then
        echo "check-adr-numbers --refs: この branch で足した ADR は無い" >&2
        return 0
    fi
    self=$(git symbolic-ref -q HEAD || true)
    out=$(mktemp) || exit 2
    mine=$(mktemp) || exit 2
    for p in $added; do
        name=${p##*/}
        [ "$name" = README.md ] && continue
        ns=$(ns_of "$p")
        [ -n "$ns" ] || continue
        printf '%s\t%s\t%s\n' "$ns" "$(classify "$ns" "$name")" "$p" >>"$mine"
    done
    allowed=$(echo $ALLOWED_DUPLICATES)
    for ref in $(git for-each-ref --format='%(refname)' refs/heads/main 'refs/heads/celeris/' 'refs/heads/celeris-wu/'); do
        [ "$ref" = "$self" ] && continue
        # shellcheck disable=SC2086
        git -c core.quotePath=false ls-tree -r --name-only "$ref" -- $dirs | sed "s|^|${ref#refs/heads/}$TAB|"
    done | awk -F '\t' -v mine="$mine" -v allowed="$allowed" '
        function nsof(p) {
            if (p ~ /^(agent-docs|docs)\/adr\//) return "main"
            if (p ~ /^(agent-docs|docs)\/gui\/adr\//) return "gui"
            if (p ~ /^(agent-docs|docs)\/web\/adr\//) return "web"
            return ""
        }
        function keyof(ns, n,   s) {
            if (n ~ /^[0-9][0-9][0-9][0-9]-[0-1][0-9]-[0-3][0-9]-.+\.md$/) { s = substr(n, 12); sub(/\.md$/, "", s); return "date\t" s }
            if (n ~ /^[0-9][0-9][0-9][0-9]-.+\.md$/) return "num\t" substr(n, 1, 4)
            if (ns != "main" && n ~ /^web-[0-9][0-9][0-9][0-9]-.+\.md$/) return "num\t" substr(n, 1, 8)
            return ""
        }
        BEGIN {
            na = split(allowed, al, " ")
            for (i = 1; i <= na; i++) ok[al[i]] = 1
            while ((getline line < mine) > 0) {
                split(line, f, "\t")
                k = f[1] "\t" f[2] "\t" f[3]
                mk[k] = mk[k] (mk[k] == "" ? "" : " ") f[4]
            }
        }
        {
            q = $2; n = q; sub(/.*\//, "", n)
            if (n == "README.md") next
            ns = nsof(q); if (ns == "") next
            kk = keyof(ns, n); if (kk == "") next
            k = ns "\t" kk
            if (!(k in mk)) next
            m = split(mk[k], ps, " ")
            for (i = 1; i <= m; i++) {
                b = ps[i]; sub(/.*\//, "", b)
                if (b == n) continue
                if (kk ~ /^num/ && (b in ok) && (n in ok)) continue
                split(kk, kp, "\t")
                h = ps[i] ":1: " q " と " kp[1] " " kp[2] " が重なる"
                if (!(h in cnt)) first[h] = $1
                cnt[h]++
            }
        }
        END {
            for (h in cnt) print h "（" first[h] (cnt[h] > 1 ? " ほか " (cnt[h] - 1) " branch" : "") "）"
        }' >"$out"
    LC_ALL=C sort -u "$out"
    n=$(sort -u "$out" | wc -l | tr -d ' ')
    rm -f "$out" "$mine"
    [ "$n" -eq 0 ]
}

ROOT=$(git rev-parse --show-toplevel 2>/dev/null) || {
    echo "check-adr-numbers: not in a git repository" >&2
    exit 2
}
cd "$ROOT" || exit 2

case "${1:-}" in
    "")
        if worktree_adrs | check_list; then
            echo "check-adr-numbers: ok ($(worktree_adrs | grep -vc '/README\.md$') files)" >&2
            exit 0
        fi
        exit 1
        ;;
    --refs)
        check_refs
        rc=$?
        [ "$rc" -eq 0 ] && echo "check-adr-numbers --refs: ok" >&2
        exit "$rc"
        ;;
    *)
        echo "usage: sh scripts/dev/check-adr-numbers.sh [--refs]" >&2
        exit 2
        ;;
esac
