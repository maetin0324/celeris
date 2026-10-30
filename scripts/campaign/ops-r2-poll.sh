#!/bin/sh
# ops-r2 の単純な状態 poll と記録（LLM を呼ばない。ADR 2026-10-30-ops-r2-isolation-principal D7）。
# 測定 daemon の GET /api/v1/tasks/<id> だけを一定間隔で呼び、1 回ごとに human-op の status_check 行を
# actor.kind=script・mode=automatic で書く。止まる条件で exit し、operator の LLM session はそのときだけ判断する。
# 常駐しない（daemon にも仕組みを足さない）。汎用の監視基盤ではない。
#
# usage: ops-r2-poll.sh --api URL --token-file F --task ID --human-ops F --campaign-bin P
#          [--interval SECS (既定 60)] [--max-polls N (既定 0 = 無制限)] [--cutoff RFC3339] -- <human-op の identity 引数...>
# identity 引数は `celeris-campaign human-op record` の --campaign・--manifest・--condition・--case・--rep・--credential 等。
# exit: 0 終端（done・failed・cancelled）、10 判断が要る状態（blocked・reviewing）、11 cutoff、12 poll 回数の上限、2 取得・記録の失敗
set -eu
die() { echo "ops-r2-poll: $*" >&2; exit 2; }
API= TOKEN= TASK= FILE= BIN= INTERVAL=60 MAX=0 CUTOFF=
while [ $# -gt 0 ]; do
    case $1 in
    --) shift; break ;;
    esac
    [ $# -ge 2 ] || die "$1 に値が要る"
    case $1 in
    --api) API=$2 ;; --token-file) TOKEN=$2 ;; --task) TASK=$2 ;; --human-ops) FILE=$2 ;;
    --campaign-bin) BIN=$2 ;; --interval) INTERVAL=$2 ;; --max-polls) MAX=$2 ;; --cutoff) CUTOFF=$2 ;;
    *) die "知らない引数 $1" ;;
    esac
    shift 2
done
for v in API TOKEN TASK FILE BIN; do eval "[ -n \"\$$v\" ]" || die "--$(echo "$v" | tr 'A-Z' 'a-z') が要る"; done
[ -s "$TOKEN" ] || die "--token-file $TOKEN が空か無い"
case $TASK in *[!A-Za-z0-9_-]*) die "--task に使えない文字" ;; esac
CUTOFF_EPOCH=
[ -z "$CUTOFF" ] || CUTOFF_EPOCH=$(date -u -d "$CUTOFF" +%s) || die "--cutoff が読めない"

n=0
while :; do
    if [ -n "$CUTOFF_EPOCH" ] && [ "$(date -u +%s)" -ge "$CUTOFF_EPOCH" ]; then
        echo '{"stop":"cutoff"}'
        exit 11
    fi
    n=$((n + 1))
    # token は argv に出さない（ps から見えないよう header を stdin の設定で渡す）。
    body=$(printf 'header = "Authorization: Bearer %s"\n' "$(tr -d '\n' <"$TOKEN")" |
        curl -fsS --max-time 30 -K - "$API/api/v1/tasks/$TASK") || die "GET tasks/$TASK に失敗"
    status=$(printf '%s' "$body" | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["status"])') ||
        die "応答に status が無い"
    "$BIN" human-op record --file "$FILE" "$@" --op-kind status_check --actor-kind script --actor ops-r2-poll \
        --mode automatic --intent monitoring --trigger scheduled --started-at now --ended-at now \
        --target "task:$TASK" --task-id "$TASK" --note "status=$status" >/dev/null || die "human-op record に失敗"
    case $status in
    done | failed | cancelled) printf '{"stop":"terminal","status":"%s","polls":%d}\n' "$status" "$n"; exit 0 ;;
    blocked | reviewing) printf '{"stop":"attention","status":"%s","polls":%d}\n' "$status" "$n"; exit 10 ;;
    esac
    if [ "$MAX" -gt 0 ] && [ "$n" -ge "$MAX" ]; then
        printf '{"stop":"max_polls","status":"%s","polls":%d}\n' "$status" "$n"
        exit 12
    fi
    sleep "$INTERVAL"
done
