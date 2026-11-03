#!/usr/bin/env bash
# P2-G（ADR 2026-11-03-llm-self-improve-trial）: 実 LLM の worker が作った候補 patch を、TS の経路か旧 Rust の経路で
# 同じ課題・同じ採点・同じ安全条件で検証し、1 行の raw record を書く。LLM を呼ばない（決定的な検査だけ）。
#   bash scripts/dev/llm-improve/trial.sh --task <課題> --path ts|rust --patch <file> --out <dir> [--label <name>]
#     <課題> は scripts/dev/llm-improve/tasks/ の dir 名（T1-cap-background・T2-cap-item-title）。
#     結果: <dir>/trial.jsonl に 1 行足す（<dir>/<label>/ に段ごとの記録）。
#   bash scripts/dev/llm-improve/trial.sh --validate <trial.jsonl>   記録を検査する（欠け・食い違いがあれば exit 1）
#
# TS の経路（path=ts）:
#   1. scope   patch が control-ts/src/ の中だけを変える（外なら失敗。検証の道具・golden・fixture を変えさせない）
#   2. eval    scripts/dev/ts-behavior-eval.sh --changes <課題 dir>（旧 fixture 2066 件の差分 0・期待差分 fixture の食い違い 0。cargo 0 回）
#   3. pilot   TS_PILOT_PATCH・TS_PILOT_SPEC 付きの scripts/dev/ts-pilot.sh（ts-verify 10 段 → 候補 bundle → 一時 DB の daemon へ
#              配備・登録 → compare → 人でない主体の canary 403・承認 403 → 人の token の承認 → canary → 再起動の復元 → rust へ rollback
#              → 再起動の復元）。本番には触れない
# 旧 Rust の経路（path=rust）:
#   1. scope   patch が crates/task-core/src/・crates/task-dispatch/src/ の中だけを変える
#   2. cargo   使い捨ての作業ツリーで edit-activate-baseline.sh の cos-digest と同じ 6 段（check・clippy・test-build・test-run
#              （-p task-core -p task-dispatch）・build -p celeris・release-build -p celeris）
#   3. score   task-core の example behavior-change-cases score（同じ旧 fixture・同じ期待差分 fixture。採点の file は kit から渡す）
#   旧経路の activate（release・promote・daemon の再起動）と rollback（release の rollback）は本番の操作なので行わない。
# 合否: 全段 exit 0 のときだけ ok。採点の file（課題 dir と旧 fixture）はこの作業ツリーの HEAD のものを使う（候補が変えても効かない）。
# exit: 0 = ok、1 = 段の失敗・採点に落ちた、2 = 使い方の誤り。
set -euo pipefail

usage() {
  echo "usage: trial.sh --task <課題> --path ts|rust --patch <file> --out <dir> [--label <name>] | --validate <trial.jsonl>" >&2
  exit 2
}
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [ "${1:-}" = --validate ]; then
  [ "$#" -eq 2 ] || usage
  python3 -I - "$2" <<'PY'
import json, sys
errors = []
try:
    rows = [json.loads(l) for l in open(sys.argv[1], encoding="utf-8") if l.strip()]
except (OSError, ValueError) as e:
    print(f"trial: 記録を読めない: {e}", file=sys.stderr)
    sys.exit(1)
if not rows:
    errors.append("記録が 0 行")
for i, r in enumerate(rows):
    w = f"行 {i + 1}（{r.get('label')}）"
    for k in ("schema", "task", "path", "label", "commit", "patch_sha256", "stages", "ok", "production_touched", "secs"):
        if k not in r:
            errors.append(f"{w}: 欄 {k} が無い")
    if r.get("schema") != "celeris.llm-improve-trial/1":
        errors.append(f"{w}: schema が違う")
    if r.get("production_touched") is not False:
        errors.append(f"{w}: production_touched が false でない")
    if r.get("path") not in ("ts", "rust"):
        errors.append(f"{w}: path が ts・rust でない")
    st = r.get("stages", {})
    want = ["scope", "eval", "pilot"] if r.get("path") == "ts" else ["scope", "check", "clippy", "test-build", "test-run", "build", "release-build", "score"]
    ran_all = all(s in st for s in want)
    if r.get("ok") and not ran_all:
        errors.append(f"{w}: ok なのに段が欠けた")
    if r.get("ok") and not all(st[s].get("exit") == 0 for s in want):
        errors.append(f"{w}: ok なのに exit 0 でない段がある")
    if r.get("ok") and r.get("path") == "ts" and st["pilot"].get("cargo_invocations") != 0:
        errors.append(f"{w}: TS の経路の ts-verify が cargo を起動した")
if errors:
    for e in errors:
        print("trial: " + e, file=sys.stderr)
    sys.exit(1)
print(f"trial: 記録 OK（{len(rows)} 行）")
PY
  exit $?
fi

task=""; path=""; patch=""; out=""; label=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --task) [ "$#" -ge 2 ] || usage; task="$2"; shift ;;
    --path) [ "$#" -ge 2 ] || usage; path="$2"; shift ;;
    --patch) [ "$#" -ge 2 ] || usage; patch="$2"; shift ;;
    --out) [ "$#" -ge 2 ] || usage; out="$2"; shift ;;
    --label) [ "$#" -ge 2 ] || usage; label="$2"; shift ;;
    *) usage ;;
  esac
  shift
done
[ -n "$task" ] && [ -n "$patch" ] && [ -n "$out" ] || usage
case "$path" in ts|rust) ;; *) usage ;; esac
kit="$here/tasks/$task"
[ -f "$kit/spec.json" ] && [ -f "$kit/spec.cases.jsonl" ] || { echo "trial: 課題が無い: $kit" >&2; exit 2; }
[ -f "$patch" ] || { echo "trial: patch が無い: $patch" >&2; exit 2; }
patch="$(cd "$(dirname "$patch")" && pwd)/$(basename "$patch")"
[ -n "$label" ] || label="$task-$path"
mkdir -p "$out/$label"
out="$(cd "$out" && pwd)"
stage_dir="$out/$label"
root="$(git rev-parse --show-toplevel)"
commit="$(git -C "$root" rev-parse HEAD)"
work="$(mktemp -d "${TMPDIR:?TMPDIR が要る}/llm-improve-trial.XXXXXX")"
tree="$work/tree"
cleanup() {
  git -C "$root" worktree remove --force "$tree" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT
stages="$work/stages.jsonl"
: >"$stages"
now() { date +%s.%N; }
t_all="$(now)"
# stage <name> <exit> <t0> [<detail json>]
stage() {
  python3 -I - "$stages" "$1" "$2" "$3" "$(now)" "${4:-{\}}" <<'PY'
import json, sys
path, name, rc, t0, t1, detail = sys.argv[1:7]
row = {"name": name, "exit": int(rc), "secs": round(float(t1) - float(t0), 3), **json.loads(detail)}
open(path, "a", encoding="utf-8").write(json.dumps(row, ensure_ascii=False) + "\n")
print(f"trial: [{name}] exit {rc}・{row['secs']} 秒", file=sys.stderr)
PY
}
finish() {
  python3 -I - "$stages" "$out/trial.jsonl" "$task" "$path" "$label" "$commit" "$(sha256sum "$patch" | cut -d' ' -f1)" \
    "$t_all" "$(now)" "$(git -C "$root" apply --numstat "$patch" | awk '{a+=$1; d+=$2} END {print a+0 "," d+0}')" <<'PY'
import json, socket, sys
from datetime import datetime, timezone
st_path, out, task, path, label, commit, patch_sha, t0, t1, numstat = sys.argv[1:11]
stages = {}
for l in open(st_path, encoding="utf-8"):
    r = json.loads(l)
    stages[r.pop("name")] = r
added, deleted = (int(x) for x in numstat.split(","))
want = ["scope", "eval", "pilot"] if path == "ts" else ["scope", "check", "clippy", "test-build", "test-run", "build", "release-build", "score"]
ok = all(s in stages and stages[s]["exit"] == 0 for s in want)
first_fail = next((s for s in want if s in stages and stages[s]["exit"] != 0), None)
edit_to_activate = None
if path == "ts" and "pilot" in stages:
    p = stages["pilot"]
    edit_to_activate = p.get("edit_to_activate_secs")
elif path == "rust":
    edit_to_activate = round(sum(stages[s]["secs"] for s in want[1:7] if s in stages), 3)
row = {"schema": "celeris.llm-improve-trial/1", "task": task, "path": path, "label": label, "commit": commit,
       "patch_sha256": patch_sha, "patch_lines": {"added": added, "deleted": deleted},
       "at": datetime.now(timezone.utc).isoformat(timespec="seconds"), "host": socket.gethostname(),
       "secs": round(float(t1) - float(t0), 3), "edit_to_activate_secs": edit_to_activate,
       "stages": stages, "ok": ok, "first_failed_stage": first_fail, "production_touched": False,
       "scope_note": ("TS: ts-verify 10 段（型・bundle・単体・golden・安全・統合・activate の代用）＋期待差分の採点＋一時 daemon の段階昇格と rollback"
                      if path == "ts" else
                      "Rust: 2 crate（task-core・task-dispatch）の check・clippy・試験＋celeris の debug・release build＋期待差分の採点。activate・rollback は本番の操作なので行わない")}
open(out, "a", encoding="utf-8").write(json.dumps(row, ensure_ascii=False) + "\n")
print(f"trial: {label}: ok={ok}（{row['secs']} 秒）", file=sys.stderr)
sys.exit(0 if ok else 1)
PY
}

# 1. scope
t0="$(now)"
case "$path" in
  ts) allowed='^control-ts/src/' ;;
  rust) allowed='^crates/(task-core|task-dispatch)/src/' ;;
esac
files="$(git -C "$root" apply --numstat "$patch" 2>/dev/null | awk '{print $3}' || true)"
outside="$(printf '%s\n' "$files" | grep -Ev "$allowed" | grep -v '^$' || true)"
if [ -z "$files" ] || [ -n "$outside" ]; then
  stage scope 1 "$t0" "$(python3 -I -c 'import json,sys; print(json.dumps({"files": sys.argv[1].split(), "outside": sys.argv[2].split()}))' "$files" "$outside")"
  finish; exit 1
fi
stage scope 0 "$t0" "$(python3 -I -c 'import json,sys; print(json.dumps({"files": sys.argv[1].split()}))' "$files")"

if [ "$path" = ts ]; then
  # 2. eval
  t0="$(now)"
  set +e
  bash "$root/scripts/dev/ts-behavior-eval.sh" --out "$stage_dir/eval" --patch "$patch" --label "$label" --changes "$kit" \
    >"$stage_dir/eval.log" 2>&1
  rc=$?
  set -e
  detail="$(python3 -I -c '
import json, sys
try:
    r = json.load(open(sys.argv[1]))
    s = r["specs"][0]
    print(json.dumps({"old_fixture_cases": r["old_fixture_cases"], "old_fixture_diffs": r["old_fixture_diffs"],
      "cases": s["cases"], "explained_ok": s["explained_ok"], "equal_ok": s["equal_ok"], "wrong": s["wrong"],
      "cases_sha256_ok": s["cases_sha256_ok"], "cargo_invocations": r["cargo_invocations"]}))
except Exception as e:
    print(json.dumps({"error": str(e)}))' "$stage_dir/eval/$label.json")"
  stage eval "$rc" "$t0" "$detail"
  [ "$rc" -eq 0 ] || { finish; exit 1; }
  # 3. pilot
  t0="$(now)"
  set +e
  TS_PILOT_PATCH="$patch" TS_PILOT_SPEC="$kit/spec.json" \
    TS_PILOT_CHANGE="P2-G 課題 $task の候補（$label）" \
    bash "$root/scripts/dev/ts-pilot.sh" --out "$stage_dir/pilot" >"$stage_dir/pilot.log" 2>&1
  rc=$?
  set -e
  detail="$(python3 -I -c '
import json, sys
try:
    rows = {r["stage"]: r for r in (json.loads(l) for l in open(sys.argv[1]) if l.strip())}
    tv = rows.get("ts_verify", {}).get("detail", {})
    # edit→activate: ts-verify（10 段。最後が activate の代用）＋候補 bundle＋配備＋登録（一時 daemon）
    e2a = sum(rows[s]["secs"] for s in ("ts_verify", "bundle", "deploy", "register") if s in rows)
    print(json.dumps({"stages": list(rows), "ts_verify_secs": tv.get("total_secs"), "cargo_invocations": tv.get("cargo_invocations"),
      "edit_to_activate_secs": round(e2a, 3),
      "rollback_secs": rows.get("rollback", {}).get("secs"),
      "restart_restore_canary_secs": rows.get("restart_restore_canary", {}).get("secs"),
      "diff": {k: rows.get("diff", {}).get("detail", {}).get(k) for k in ("fixture_cases", "fixture_diffs", "probe_cases", "probe_diffs")}}))
except Exception as e:
    print(json.dumps({"error": str(e)}))' "$stage_dir/pilot/record.jsonl")"
  stage pilot "$rc" "$t0" "$detail"
  [ "$rc" -eq 0 ] || { finish; exit 1; }
  finish
  exit $?
fi

# ---- 旧 Rust の経路 ----
: "${CARGO_TARGET_DIR:?trial: CARGO_TARGET_DIR が要る}"
git -C "$root" worktree add --detach "$tree" HEAD >/dev/null 2>&1
git -C "$tree" apply "$patch"
pflags="-p task-core -p task-dispatch"
run_step() {
  local name="$1"; shift
  local t0 rc
  t0="$(now)"
  set +e
  (cd "$tree" && "$@") >"$stage_dir/$name.log" 2>&1
  rc=$?
  set -e
  stage "$name" "$rc" "$t0" "{\"cmd\": \"$*\"}"
  if [ "$rc" -ne 0 ]; then
    tail -30 "$stage_dir/$name.log" >&2
    finish || true
    exit 1
  fi
}
# shellcheck disable=SC2086
run_step check cargo check $pflags
# shellcheck disable=SC2086
run_step clippy cargo clippy $pflags -- -D warnings
# shellcheck disable=SC2086
run_step test-build cargo nextest run $pflags --no-run
# shellcheck disable=SC2086
run_step test-run cargo nextest run $pflags --no-fail-fast
run_step build cargo build -p celeris
run_step release-build cargo build --release -p celeris
t0="$(now)"
set +e
(cd "$tree" && cargo run -q -p task-core --example behavior-change-cases -- score --spec "$kit/spec.json" \
  --cases "$kit/spec.cases.jsonl" --fixtures "$root/control-ts/fixtures/cos_digest") >"$stage_dir/score.json" 2>"$stage_dir/score.log"
rc=$?
set -e
detail="$(python3 -I -c '
import json, sys
try:
    r = json.loads(open(sys.argv[1]).read())
    s = r["specs"][0]
    print(json.dumps({"old_fixture_cases": r["old_fixture_cases"], "old_fixture_diffs": r["old_fixture_diffs"],
      "cases": s["cases"], "explained_ok": s["explained_ok"], "equal_ok": s["equal_ok"], "wrong": s["wrong"],
      "cases_sha256_ok": s["cases_sha256_ok"]}))
except Exception as e:
    print(json.dumps({"error": str(e)}))' "$stage_dir/score.json")"
stage score "$rc" "$t0" "$detail"
finish
