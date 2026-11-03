#!/usr/bin/env bash
# scripts/dev/ts-pilot.sh --validate の検査を、記録済みの pilot の record.jsonl と、それを 1 か所ずつ壊した写しで確かめる。
#   - 記録済みの record.jsonl は exit 0
#   - 段の欠け・synthetic の印の欠け・人でない主体の canary・fixture の差分・拒否の code の食い違い・JSON の壊れは exit 1
# daemon・cargo・node を起動しない。
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
src="$root/scripts/dev/ts-pilot.sh"
rec="$root/agent-docs/progress/2026-11-02-ts-control-layer-s6/pilot/record.jsonl"
work="$(mktemp -d "${TMPDIR:-/tmp}/ts-pilot-validate.XXXXXX")"
trap 'rm -rf "$work"' EXIT

fail=0
expect() { # want_exit name file
  local rc=0
  bash "$src" --validate "$3" >/dev/null 2>"$work/err" || rc=$?
  if [ "$rc" -ne "$1" ]; then
    echo "FAIL $2: exit $rc（期待 $1）"; cat "$work/err"; fail=1
  else
    echo "ok   $2"
  fi
}
mutate() { # name python-expr-on-rows
  python3 -I - "$rec" "$work/$1.jsonl" "$2" <<'PY'
import json, sys
src, out, expr = sys.argv[1:4]
rows = [json.loads(l) for l in open(src, encoding="utf-8") if l.strip()]
by = {r["stage"]: r for r in rows}
exec(expr)
with open(out, "w", encoding="utf-8") as f:
    for r in rows:
        f.write(json.dumps(r, ensure_ascii=False) + "\n")
PY
}

expect 0 recorded "$rec"
mutate no-rollback 'rows[:] = [r for r in rows if r["stage"] != "rollback"]
for i, r in enumerate(rows): r["seq"] = i + 1'
expect 1 no-rollback "$work/no-rollback.jsonl"
mutate not-synthetic 'by["guard_seed"]["detail"]["synthetic"] = False'
expect 1 not-synthetic "$work/not-synthetic.jsonl"
mutate canary-by-operator 'by["canary"]["actor"] = "operator:pilot"'
expect 1 canary-by-operator "$work/canary-by-operator.jsonl"
mutate fixture-diff 'by["diff"]["detail"]["fixture_diffs"] = 1'
expect 1 fixture-diff "$work/fixture-diff.jsonl"
mutate wrong-code 'by["canary_denied_non_human"]["detail"]["error"] = None'
expect 1 wrong-code "$work/wrong-code.jsonl"
mutate restore-lost 'by["restart_restore_canary"]["detail"]["mode"] = "rust"'
expect 1 restore-lost "$work/restore-lost.jsonl"
cp "$rec" "$work/broken.jsonl"; echo '{not json' >>"$work/broken.jsonl"
expect 1 broken-json "$work/broken.jsonl"
expect 1 missing-file "$work/absent.jsonl"

[ "$fail" -eq 0 ] && echo "ts-pilot-validate: 全部通った"
exit "$fail"
