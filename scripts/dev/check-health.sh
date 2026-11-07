#!/usr/bin/env bash
# repo health の単一入口。root と base（既定 auto）を snapshot が決めること・既存検査を rule として呼ぶことは
# ADR 2026-11-07-repo-health-celeris-profile（D1・D2・D4）、report の形と exit code は ADR 2026-11-07-repo-health-contract。
#
#   bash scripts/dev/check-health.sh [--base <rev>] [--out <report.json>]
#
# 作業ツリー（未 commit の変更を含む）を repo_health.py に渡し、report を --out（既定
# ${TMPDIR:-/tmp}/check-health.<pid>.json）に書いてその path を出す。exit は repo_health.py の exit code。
# ただし policy の file だけの変更は exit 0 にして `policy review required: <files>` を出す（report の verdict は
# fail のまま。ADR 2026-11-07-health-review-integration D6.2）。合格の後、doc-only でなければ検査器自身の
# unit 試験とゲートの試験を走らせる。全体の試験は release gate だけで走る。
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
if [ "$rc" -eq 1 ]; then
  # D6.2: fail 級の finding が policy.changed だけで、base と作業ツリーの差分の path が全部 policy.files にある。
  # 差分の path は report と同じ snapshot（report の base の sha）で取る。
  policy_files=$(python3 -I - "$here/repo-health" "$here" "$out" <<'PY'
import json
import sys

sys.dont_write_bytecode = True
sys.path.insert(0, sys.argv[1])
from health import engine, snapshot  # noqa: E402

with open(sys.argv[3], encoding="utf-8") as fh:
    report = json.load(fh)
snap = snapshot.resolve(sys.argv[2], report["snapshot"]["base"]["sha"], snapshot.WORKTREE)
snap.head_tree.reader.close()
files = engine.policy_review_files(report, snap.changed)
if files:
    print(", ".join(files))
PY
  )
  if [ -n "$policy_files" ]; then
    echo "policy review required: $policy_files"
    echo "check-health: policy の file だけの変更。report の verdict は fail のまま、policy の変更は人の承認で通す"
    rc=0
  fi
fi
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
