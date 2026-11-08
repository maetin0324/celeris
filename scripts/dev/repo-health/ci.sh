#!/bin/sh
# CI から呼ぶ repo health の入口。作業ツリーでなく commit の head を検査する（report の summary.reviewable が
# true の形）。report の形と exit code は ADR 2026-11-07-repo-health-contract（D3）、policy の扱いは
# ADR 2026-11-07-health-review-integration（D6）。check-health.sh の D6.2（policy の file だけの変更を exit 0 に
# する扱い）はしない。exit は repo_health.py の exit code のまま返す。
#
#   sh scripts/dev/repo-health/ci.sh --base <rev> [--head <rev>] [--out <report.json>]
#   sh scripts/dev/repo-health/ci.sh --help
#
# POSIX sh（dash で動く）。外部ネットワークに出ない。
set -eu

usage() {
    cat <<'EOF'
usage: sh scripts/dev/repo-health/ci.sh --base <rev> [--head <rev>] [--out <report.json>]
       sh scripts/dev/repo-health/ci.sh --help

この script を置いた repo の commit <head>（既定 HEAD）を <base> と比べて repo health を検査する。
作業ツリーの未 commit の変更は見ない。report（celeris.repo-health.report/1）を <out>
（既定 ${TMPDIR:-/tmp}/repo-health-ci.<pid>.json）に書き、validate_report.py で確かめてから
その path を出す。

exit:
  0  必須 rule が全部 pass
  1  必須 rule の fail
  2  必須 rule の error・unknown・unsupported、report を書けない・契約に合わない、使い方誤り、
     shallow clone で base に届かない（深さを足す手順を stderr に出す）
EOF
}

die_usage() {
    echo "ci.sh: $1" >&2
    usage >&2
    exit 2
}

here=$(cd "$(dirname "$0")" && pwd)
base=
head=HEAD
out="${TMPDIR:-/tmp}/repo-health-ci.$$.json"
while [ $# -gt 0 ]; do
    case "$1" in
        --help | -h) usage; exit 0 ;;
        --base | --head | --out)
            [ $# -ge 2 ] || die_usage "$1 に値が無い"
            case "$1" in
                --base) base=$2 ;;
                --head) head=$2 ;;
                --out) out=$2 ;;
            esac
            shift 2
            ;;
        *) die_usage "未知の引数: $1" ;;
    esac
done
[ -n "$base" ] || die_usage "--base が無い"
[ -n "$head" ] || die_usage "--head が空"
[ -n "$out" ] || die_usage "--out が空"

root=$(git -C "$here" rev-parse --show-toplevel 2>/dev/null) || {
    echo "ci.sh: $here は Git の作業ツリーの中にない" >&2
    exit 2
}

deepen_hint() {  # deepen_hint <届かない rev>
    {
        echo "ci.sh: $1 に届かない。$root は shallow clone で、その commit の履歴を持っていない。"
        echo "ci.sh: 履歴を足してから同じコマンドを再実行する。どちらか 1 つ:"
        echo "  git -C $root fetch --deepen=<n>    # n は head から base までの commit 数以上"
        echo "  git -C $root fetch --unshallow     # 全履歴を取る"
        echo "ci.sh: base が branch 名で、その branch を取っていなければ、remote と branch を付けて fetch する"
        echo "  （例: git -C $root fetch --deepen=<n> origin main）。CI の checkout なら履歴の深さの設定を増やす。"
    } >&2
}

shallow=$(git -C "$root" rev-parse --is-shallow-repository)
head_sha=$(git -C "$root" rev-parse --verify --quiet "$head^{commit}") || {
    echo "ci.sh: --head $head が commit に解決できない" >&2
    [ "$shallow" != true ] || deepen_hint "--head $head"
    exit 2
}

# 前の report を確かめてしまわないよう、書き先を先に消す。
rm -f "$out"
rc=0
python3 -I "$here/repo_health.py" --repo "$root" --base "$base" --head "$head_sha" \
    --policy-version 1 --out "$out" || rc=$?
if [ ! -f "$out" ]; then
    echo "ci.sh: report を書けなかった（repo_health.py の exit $rc）" >&2
    exit 2
fi
echo "ci.sh: report=$out"
if ! verdict=$(python3 -I "$here/validate_report.py" "$out" 2>&1); then
    printf '%s\n' "$verdict" >&2
    echo "ci.sh: report が契約（celeris.repo-health.report/1）に合わない" >&2
    exit 2
fi
# report の exit_code と repo_health.py の exit code が食い違えば（report を書いた後の異常終了など）report を信じない。
facts=$(python3 -I -c '
import json, sys
r = json.load(open(sys.argv[1], encoding="utf-8"))
print(r["summary"]["exit_code"], str(r["snapshot"]["base"]["reachable"]).lower(), str(r["snapshot"]["shallow"]).lower())
' "$out") || {
    echo "ci.sh: report の summary・snapshot を読めない" >&2
    exit 2
}
set -- $facts
report_rc=$1 reachable=$2 report_shallow=$3
if [ "$rc" -ne "$report_rc" ]; then
    echo "ci.sh: repo_health.py の exit $rc が report の exit_code $report_rc と違う" >&2
    exit 2
fi
if [ "$reachable" = false ]; then
    if [ "$report_shallow" = true ]; then
        deepen_hint "--base $base"
    else
        echo "ci.sh: --base $base に届かない" >&2
    fi
    exit 2
fi
exit "$rc"
