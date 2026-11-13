#!/usr/bin/env bash
# 健全性 v2 R5 affected-features: test-affected の crate の組が変わっても共有 crate を作り直さないことを確かめる
# （ADR 2026-11-10-test-target-bundles D6-7）。
#   bash agent-docs/progress/2026-11-10-health-v2-r5/rebuild-check.sh
#
# test-affected.sh と同じ cargo の呼び方（`cargo nextest run --workspace -E 'package(=a) | package(=b) …'`）で --no-run を
#   1. 組 A: workspace の全 crate（task-core を含む。workspace 共通 file の変更で test-affected が選ぶ組）
#   2. 組 B: celeris,celerisctl（celeris の編集で test-affected が選ぶ組）
#   3. 組 A
# の順に走らせ、各回の Compiling 行の数を出す。1 回目と 2 回目の間に crates/celeris/src/lib.rs の mtime を進める（内容は
# 変えない。組 B を選ぶ celeris の編集の代わり）。mtime を進めないと、`-p` の組で build していた直す前も組ごとの版が target
# dir に残るので 3 回目は 0 になり、直す前と後を区別できない（baseline.md の A→B→A→B の計測）。直す前は 3 回目に A の版の
# celeris・celerisctl を作り直す。
#
# 呼び方の写しを持たないように、test-affected.sh の filter と nextest の 2 行が在ることを確かめ、その 2 行を eval で実行する。
# exit: 0 = 3 回目に作り直した crate が 0（workspace の crate も外部の crate も）、1 = 1 以上、2 = build の失敗・呼び方の食い違い。
# log は $TMPDIR の使い捨ての dir に置き、作業ツリーには書かない。
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
cd "$root"
ta="scripts/dev/test-affected.sh"

# test-affected.sh の呼び方（この 2 行が test-affected.sh に無ければ、この台本の呼び方は test-affected と違う）。
filter_line='  while IFS= read -r c; do filter="${filter:+$filter | }package(=$c)"; done <<<"$crates"'
run_line='  ttl_run cargo nextest run --workspace -E "$filter"'
for l in "$filter_line" "$run_line"; do
  if ! grep -qxF -- "$l" "$ta"; then
    echo "rebuild-check: $ta に次の行が無い（cargo の呼び方がこの台本と違う）: $l" >&2
    exit 2
  fi
done

members="$(cargo metadata --format-version 1 --no-deps | python3 -I -c '
import json, sys
m = json.load(sys.stdin)
ids = set(m["workspace_members"])
print("\n".join(sorted(p["name"] for p in m["packages"] if p["id"] in ids)))
')"
group_a="$members"
group_b="$(printf '%s\n' celeris celerisctl)"
for c in $group_b; do
  printf '%s\n' "$members" | grep -qx "$c" || { echo "rebuild-check: workspace に $c が無い" >&2; exit 2; }
done
printf '%s\n' "$group_a" | grep -qx task-core || { echo "rebuild-check: 組 A に task-core が無い" >&2; exit 2; }

# test-affected.sh で確かめた 2 行をそのまま実行する（crates は 1 行 1 crate）。nextest の行には --no-run だけを足す。
nextest_no_run() {
  local crates="$1" c filter=""
  eval "$filter_line"
  eval "${run_line#  ttl_run } --no-run"
}

logdir="$(mktemp -d "${TMPDIR:-/tmp}/rebuild-check.XXXXXX")"
trap 'rm -rf "$logdir"' EXIT
export CARGO_TERM_COLOR=never CARGO_TERM_PROGRESS_WHEN=never

# step <n> <label> <crates>: --no-run を走らせ、Compiling の数を "<n> <label> <secs> <total> <workspace>" で出す。
step() {
  local n="$1" label="$2" crates="$3" lg t0 t1 rc
  lg="$logdir/step$n.log"
  t0="$(date +%s.%N)"
  set +e
  nextest_no_run "$crates" >"$lg" 2>&1
  rc=$?
  set -e
  t1="$(date +%s.%N)"
  if [ "$rc" -ne 0 ]; then
    echo "rebuild-check: $n 回目（組 $label）の build が失敗した (exit $rc)。log の末尾:" >&2
    tail -n 30 "$lg" >&2
    exit 2
  fi
  python3 -I - "$lg" "$n" "$label" "$t0" "$t1" "$members" <<'PY'
import re, sys
lg, n, label, t0, t1, members = sys.argv[1:7]
members = set(members.split())
names = [m.group(1) for m in (re.match(r"^\s*Compiling (\S+) v\S+", l) for l in open(lg, encoding="utf-8", errors="replace")) if m]
ws = sorted(x for x in names if x in members)
print("%s %s %.2f %d %d %s" % (n, label, float(t1) - float(t0), len(names), len(ws), ",".join(ws) or "-"))
PY
}

out1="$(step 1 A "$group_a")"
echo "rebuild-check: $out1" >&2
touch crates/celeris/src/lib.rs
out2="$(step 2 B "$group_b")"
echo "rebuild-check: $out2" >&2
out3="$(step 3 A "$group_a")"
echo "rebuild-check: $out3" >&2

total3="$(printf '%s\n' "$out3" | awk '{print $4}')"
ws3="$(printf '%s\n' "$out3" | awk '{print $5}')"
echo "rebuild-check: 回 組 秒 Compiling workspace の crate"
printf '%s\n' "$out1" "$out2" "$out3" | awk '{printf "rebuild-check: %s %s %s %s %s (%s)\n", $1, $2, $3, $4, $5, $6}'
echo "rebuild-check: 3 回目に作り直した workspace の crate: $ws3（Compiling 行の合計 $total3）"
if [ "$total3" -eq 0 ] && [ "$ws3" -eq 0 ]; then
  exit 0
fi
exit 1
