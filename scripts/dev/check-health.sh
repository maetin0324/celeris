#!/usr/bin/env bash
# repo health の単一入口（ADR 2026-11-07-repo-health-celeris-profile）。全体の試験は release gate だけで走る。
#
#   bash scripts/dev/check-health.sh [--base <rev>] [--out <report.json>]
#
# repo_health.py に作業ツリー（未 commit の変更を含む）を渡し、report JSON（celeris.repo-health.report/1）を
# --out（既定 ${TMPDIR:-/tmp}/check-health.<pid>.json）に書いて、その path を出す。既存検査（source-size・
# architecture-map・doc-links・adr-numbers・human-text・progress-index）は profile（profiles/celeris.toml）の
# rule として repo_health.py が呼ぶ。root と base（既定 auto = git merge-base HEAD main）は repo_health.py の
# snapshot が 1 か所で決める。この script は root も base も解決しない。
# 必須 rule が pass（か not_applicable）以外なら repo_health.py の exit code（1 fail、2 error・欠測）で終わる。
# 続けて検査器自身の試験（repo_health・architecture-map の unit 試験とゲートの試験）を走らせる（doc-only の変更では走らせない）。
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
base=auto
out="${TMPDIR:-/tmp}/check-health.$$.json"
while [ $# -gt 0 ]; do
  case "$1" in
    --base) base="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    *) echo "usage: check-health.sh [--base <rev>] [--out <report.json>]" >&2; exit 2 ;;
  esac
done

rc=0
python3 "$here/repo-health/repo_health.py" --repo "$here" --base "$base" --head worktree \
  --policy-version 1 --out "$out" || rc=$?
if [ ! -f "$out" ]; then
  echo "check-health: report を書けなかった（exit $rc）" >&2
  exit "$rc"
fi
echo "check-health: report=$out"
if [ "$rc" -ne 0 ]; then
  echo "check-health: 必須 rule が pass でない（exit $rc）" >&2
  exit "$rc"
fi
mode=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["inputs"]["mode"])' "$out")
if [ "$mode" = doc-only ]; then
  echo "check-health: doc-only（検査器の unit 試験を省く）"
  exit 0
fi
tests="$here/../tests"
for pat in 'test_architecture_map.py' 'test_repo_health*.py'; do
  # 試験の file が無い repo（検査器だけを写した repo）では省く。unittest は 0 件を exit 5 にする。
  compgen -G "$tests/$pat" >/dev/null || continue
  python3 -I -m unittest discover -s "$tests" -p "$pat"
done
if [ -f "$here/tests/repo_health_gate.sh" ]; then
  sh "$here/tests/repo_health_gate.sh"
fi
