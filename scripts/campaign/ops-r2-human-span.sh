#!/bin/sh
# 人の能動時間の区間を記録する（ADR 2026-10-30-ops-r2-isolation-principal D6）。人が自分の shell で打つ。
#   begin: 開始時刻を状態 file に残すだけ（human-op の行はまだ書かない）。
#   end:   `celeris-campaign human-op record --actor-kind human --time-basis evaluator_live` に開始と終了を 1 回で渡す。
# 人の能動時間はこの区間の和集合だけで測る。web の承認・回答のような点は別に occurred_at で残し、長さ 0 として足さない。
#
# usage: ops-r2-human-span.sh begin --state DIR --logical-op ID
#        ops-r2-human-span.sh end   --state DIR --logical-op ID --campaign-bin P --file F --actor human:<名前> -- <identity・op 引数...>
# end の後ろには --campaign・--manifest・--condition・--case・--rep・--op-kind・--intent 等を書く。
set -eu
die() { echo "ops-r2-human-span: $*" >&2; exit 2; }
[ $# -ge 1 ] || die "begin か end"
CMD=$1
shift
STATE= LOGICAL= BIN= FILE= ACTOR= NOW=${CELERIS_HUMAN_OPS_NOW:-}
while [ $# -gt 0 ]; do
    case $1 in
    --) shift; break ;;
    esac
    [ $# -ge 2 ] || die "$1 に値が要る"
    case $1 in
    --state) STATE=$2 ;; --logical-op) LOGICAL=$2 ;; --campaign-bin) BIN=$2 ;; --file) FILE=$2 ;;
    --actor) ACTOR=$2 ;; --now) NOW=$2 ;;
    *) die "知らない引数 $1" ;;
    esac
    shift 2
done
[ -n "$STATE" ] && [ -n "$LOGICAL" ] || die "--state と --logical-op が要る"
case $LOGICAL in *[!A-Za-z0-9._-]*) die "--logical-op に使えない文字" ;; esac
[ -n "$NOW" ] || NOW=$(date -u +%Y-%m-%dT%H:%M:%SZ)
mark=$STATE/$LOGICAL.start
case $CMD in
begin)
    umask 077
    mkdir -p "$STATE"
    [ ! -e "$mark" ] || die "$LOGICAL は begin 済み（end してから次を始める）"
    printf '%s\n' "$NOW" >"$mark"
    printf '{"begin":"%s","logical_op":"%s"}\n' "$NOW" "$LOGICAL"
    ;;
end)
    [ -f "$mark" ] || die "$LOGICAL の begin が無い（開始時刻の分からない区間は作らない）"
    [ -n "$BIN" ] && [ -n "$FILE" ] && [ -n "$ACTOR" ] || die "end には --campaign-bin・--file・--actor が要る"
    case $ACTOR in human:?*) ;; *) die "--actor は human:<名前>" ;; esac
    start=$(cat "$mark")
    "$BIN" human-op record --file "$FILE" "$@" --actor-kind human --actor "${ACTOR#human:}" --mode manual \
        --decided-by "$ACTOR" --executed-by "$ACTOR" --logical-op "$LOGICAL" \
        --started-at "$start" --ended-at "$NOW" --time-basis evaluator_live --time-precision second
    rm -f -- "$mark"
    ;;
*) die "begin か end（$CMD）" ;;
esac
