#!/usr/bin/env bash
# release の gate に足した TS 制御層の 4 段（scripts/selfdeploy/script-bundle-gate.sh）の秒を測る
# （ADR 2026-11-03-release-script-bundle-gate）。release そのものは作らない。
#   bash scripts/dev/script-gate-measure.sh <cold|warm> <reps> <out.jsonl>
# 作業ツリーの HEAD で install → bundle → verify（source_commit = HEAD）→ test を順に回し、1 試行 1 行の JSON を追記する。
#   cold: 試行の前に control-ts/node_modules・control-ts/dist と script-load-check の binary を消す
#         （release の作業ツリーは毎回 git clean -ffdx されるので、release の段はいつもこの状態から始まる。
#         pnpm の store と、cargo-test 段が作る試験の binary は消さない）。
#   warm: 何も消さない。
# 作業ツリーに未 commit の変更があれば verify が落ちる（source_commit が -dirty になる）。
set -euo pipefail
[ $# -eq 3 ] || { echo "usage: script-gate-measure.sh <cold|warm> <reps> <out.jsonl>" >&2; exit 2; }
mode="$1" reps="$2" out="$3"
case "$mode" in cold|warm) ;; *) echo "mode must be cold or warm" >&2; exit 2 ;; esac
case "$out" in /*) ;; *) out="$PWD/$out" ;; esac
root="$(git rev-parse --show-toplevel)"
cd "$root"
gate="$root/scripts/selfdeploy/script-bundle-gate.sh"
dir="$root/control-ts/dist"
head="$(git rev-parse HEAD)"
target="${CARGO_TARGET_DIR:-$root/target}"
work="$(mktemp -d "${TMPDIR:-/tmp}/script-gate-measure.XXXXXX")"
trap 'rm -rf "$work"' EXIT
now() { date +%s.%N; }

for rep in $(seq 1 "$reps"); do
  if [ "$mode" = cold ]; then
    rm -rf control-ts/node_modules control-ts/dist "$target/debug/script-load-check"
  fi
  busy="$(pgrep -c -x 'cargo|rustc' || true)"
  : >"$work/stages.tsv"
  t0="$(now)"
  for stage in install bundle verify test; do
    case "$stage" in
      install|bundle) set -- "$stage" "$root/control-ts" ;;
      verify) set -- verify "$root" "$dir" "$head" ;;
      test) set -- test "$root" "$dir" ;;
    esac
    s="$(now)"; rc=0
    CELERIS_SCRIPT_REQUIRE_BUNDLE=1 CELERIS_SCRIPT_BUNDLE_DIR="$dir" bash "$gate" "$@" >"$work/$stage.log" 2>&1 || rc=$?
    e="$(now)"
    printf '%s\t%s\t%s\n' "$stage" "$rc" "$(awk -v a="$s" -v b="$e" 'BEGIN{printf "%.3f", b-a}')" >>"$work/stages.tsv"
    [ "$rc" -eq 0 ] || break
  done
  total="$(awk -v a="$t0" -v b="$(now)" 'BEGIN{printf "%.3f", b-a}')"
  python3 -I - "$work" "$mode" "$rep" "$head" "$total" "$busy" >>"$out" <<'PY'
import datetime, json, re, sys
work, mode, rep, head, total, busy = sys.argv[1:7]
stages, ok = {}, True
for line in open(f"{work}/stages.tsv"):
    name, rc, secs = line.rstrip("\n").split("\t")
    stages[name] = {"secs": float(secs), "exit": int(rc)}
    ok = ok and rc == "0"
tests = None
try:
    m = re.search(r"Summary \[\s*([\d.]+)s\] (\d+) tests run: (\d+) passed", open(f"{work}/test.log").read())
    if m:
        tests = {"nextest_secs": float(m.group(1)), "run": int(m.group(2)), "passed": int(m.group(3))}
except OSError:
    pass
print(json.dumps({"kind": "script_gate", "mode": mode, "rep": int(rep), "commit": head,
                  "at": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
                  "ok": ok, "total_secs": float(total), "stages": stages, "tests": tests,
                  "busy_cargo_rustc": int(busy or 0)}))
PY
  echo "script-gate-measure: $mode rep $rep total ${total}s ($(tr '\t\n' ': ' <"$work/stages.tsv"))" >&2
done
