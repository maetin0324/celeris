#!/usr/bin/env bash
# scripts/dev/edit-activate-summary.py の重複除去を確かめる（ADR 2026-11-02-ts-control-layer-boundary 付記 S6-1）。
# 使い方: bash scripts/dev/tests/edit-activate-summary.sh [試す script の path。既定は repo の script]
# 確かめること: 同じ step・rep 0・同じ at の estimate が別 release なら別に数える。
# a・b と連結 file を同時に渡しても試行も gate も二重に数えない。S6 の生データ全部で gate 4 件、exit 0。
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
summary="${1:-$root/scripts/dev/edit-activate-summary.py}"
raw="$root/agent-docs/progress/2026-11-02-ts-control-layer-s6"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

python3 -I - "$work" <<'PY'
import json, sys
work = sys.argv[1]
def rows(reps, at):
    out = [{"kind": "trial", "scenario": "s", "mode": "warm", "rep": r, "at": f"{at}-{r}", "secs": float(r), "exit": 0}
           for r in reps]
    for rel, secs in (("R1", {"x": 1.0, "y": 2.0}), ("R2", {"x": 10.0, "y": 20.0})):
        for step, v in secs.items():
            out.append({"kind": "estimate", "scenario": "s", "mode": "warm", "rep": 0, "at": at, "step": step,
                        "secs": v, "release": {"at": rel, "ok": True}})
    return "".join(json.dumps(r) + "\n" for r in out)
a, b = rows(range(1, 11), "A"), rows(range(11, 21), "B")
open(f"{work}/a.jsonl", "w").write(a)
open(f"{work}/b.jsonl", "w").write(b)
open(f"{work}/all.jsonl", "w").write(a + b)
PY

check() { # 説明 期待の exit 期待の gate（release=total,...） 期待の試行数（空なら見ない） file...
  local what="$1" want="$2" gates="$3" trials="$4" rc=0
  shift 4
  python3 -I "$summary" "$@" >"$work/out.json" 2>"$work/err" || rc=$?
  if [ "$rc" -ne "$want" ]; then
    echo "FAIL: $what: exit $rc（期待 $want）: $(head -c 600 "$work/err")" >&2
    return 1
  fi
  python3 -I - "$work/out.json" "$what" "$gates" "$trials" <<'PY' || return 1
import json, sys
path, what, gates, trials = sys.argv[1:]
d = json.load(open(path))
got = ",".join(f"{g['release_at']}={g['total_secs']}" for g in d["release_gate"])
if got != gates:
    sys.exit(f"FAIL: {what}: release_gate {got}（期待 {gates}）")
counts = {e["trials"] for e in d["scenarios"]}
if trials and counts != {int(trials)}:
    sys.exit(f"FAIL: {what}: 試行数 {sorted(counts)}（期待 {trials}）")
PY
  echo "ok: $what"
}

gates="R1=3.0,R2=30.0"
check "1 file に別 release の estimate が並ぶ（試行 10 で exit 1）" 1 "$gates" 10 "$work/a.jsonl"
check "a・b を渡す" 0 "$gates" 20 "$work/a.jsonl" "$work/b.jsonl"
check "連結 file だけを渡す" 0 "$gates" 20 "$work/all.jsonl"
check "a・b と連結 file を同時に渡しても二重に数えない" 0 "$gates" 20 "$work/a.jsonl" "$work/b.jsonl" "$work/all.jsonl"
check "S6 の生データ全部（ok=true の gate 4 件）" 0 \
  "2026-10-31T23:12:48Z=648.145,2026-11-01T09:02:49Z=804.889,2026-11-01T09:19:14Z=740.921,2026-11-02T09:25:19Z=501.43" 20 \
  "$raw"/*.jsonl
echo "edit-activate-summary: all ok"
