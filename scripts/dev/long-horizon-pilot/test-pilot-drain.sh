#!/usr/bin/env bash
# pilot の drain・共有の期限の無 LLM 証拠（ADR 2026-11-06-cos-run-launch-accounting-and-drain D3・D5、
# ADR 2026-11-07-pilot-drain-shared-deadline）。実 daemon・一時 DB・config-stub（LLM 0）で runner を流し、DB → facts → judge を検査する。
#
#   bash scripts/dev/long-horizon-pilot/test-pilot-drain.sh [--out <dir>] [--before-rev <git rev>]
#
# 場面: (1) CoS の返事が終端直前まで遅い → driver は daemon を止める前に待ち、usage が全部ある
#       (2) 供給元（account）の同時 run 数 1（[llm_sources.local] max_runs = 1） → CoS は枠の解放を待って起動し、worker・reviewer の行は 1 本ずつ
#       (3) 期限の中で CoS が終わらない → facts に未完了 run と理由、usage は null、judge は unknown
#       (4) 2 試行が承認の wall の総量を分け合う → 2 本目の期限は 1 本目の開始から数える
# どの場面も config-stub なので G7 は pending、cos_usage_null は null（実 LLM の合格を主張しない）。
# --before-rev を渡すと、その版の pilot（driver・judge）で (1)(3) を流し、修正前に失敗することを示す。
# 本番の DB・daemon に触れない。時間は stub の sleep だけ（負荷は掛けない）。
if [ -z "${BASH_VERSION:-}" ]; then exec bash "$0" "$@"; fi
set -euo pipefail
out="" before=""
while [ $# -gt 0 ]; do
    case "$1" in
        --out) out="$2"; shift 2 ;;
        --before-rev) before="$2"; shift 2 ;;
        *) echo "usage: $0 [--out <dir>] [--before-rev <rev>]" >&2; exit 2 ;;
    esac
done
fail() { echo "test-pilot-drain: FAIL: $*" >&2; exit 1; }
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$root"
: "${CARGO_TARGET_DIR:?CARGO_TARGET_DIR is required}"
out="${out:-$(mktemp -d "${TMPDIR:-/tmp}/pilot-drain.XXXXXX")}"
mkdir -p "$out"; out="$(cd "$out" && pwd)"
runner="$root/scripts/dev/long-horizon-pilot-llm.sh"

summarize() { # raw-dir -> 1 行ずつ facts の要点（人が読む整形。検査は check が行う）
    python3 -I - "$1" <<'PY'
import json, pathlib, sys
raw = pathlib.Path(sys.argv[1])
for f in sorted((raw / "facts").glob("*.json")):
    x = json.loads(f.read_text())
    cos = [i for i in x["invocations"] if i["role"] == "cos"]
    print(json.dumps({"case": x["case"], "cos": [{k: i.get(k) for k in ("at", "launch", "slot_wait_secs", "drain_timeout")
                                                  if k in i} | {"usage": bool(i["usage"])} for i in cos],
                      "usage_missing": x["usage"]["missing"], "usd": x["usage"]["usd"], "wall": x["usage"]["wall_secs"],
                      "drain": {k: v for k, v in (x.get("drain") or {}).items() if k != "timed_out_runs"}}, ensure_ascii=False))
g = json.loads((raw / "grade" / "pilot-gate.json").read_text())
print(json.dumps({"verdict": g["verdict"], "g7": g["g7"], "cos_usage_null": g["cos_usage_null"],
                  "cos_drain_timeout": g.get("cos_drain_timeout"), "cos_not_launched": g.get("cos_not_launched"),
                  "wall_secs": g["totals"]["wall_secs"]}))
PY
}

run_new() { # name env-assignments... -- args
    local name="$1"; shift
    local envs=() args=()
    while [ "$1" != "--" ]; do envs+=("$1"); shift; done; shift
    args=("$@")
    set +e
    env "${envs[@]}" bash "$runner" --harness config-stub --out "$out/$name" "${args[@]}" >"$out/$name.log" 2>&1
    rc=$?
    set -e
    echo "$rc" >"$out/$name.rc"
}

check() { python3 -I - "$@"; }

# (1) 遅い CoS: 最後の操作の後も CoS の返事が続く（20 秒）。daemon を止める前に待ち、usage が全部ある。
run_new drain-slow-cos LONG_HORIZON_PILOT_STUB_COS_DELAY=20 -- --cases C08 --arms new --max-wall-secs 200
check "$out/drain-slow-cos" <<'PY' || fail "slow CoS: facts/judge"
import json, pathlib, sys
raw = pathlib.Path(sys.argv[1])
x = json.loads(next((raw / "facts").glob("*.json")).read_text())
g = json.loads((raw / "grade" / "pilot-gate.json").read_text())
cos = [i for i in x["invocations"] if i["role"] == "cos"]
assert cos and all(i["usage"] and i["launch"] == "launched" for i in cos), cos
assert x["usage"]["missing"] is None and x["usage"]["usd"] is not None, x["usage"]
assert x["drain"]["timed_out"] == 0 and x["drain"]["waited_secs"] >= 10, x["drain"]
assert (g["g7"], g["cos_usage_null"], g["cos_drain_timeout"]) == ("pending", None, None), g
print("test-pilot-drain: (1) slow CoS: drain waited", x["drain"]["waited_secs"], "s; every CoS run has usage; g7 pending")
PY

# (2) 供給元（account）の同時 run 数 1（[llm_sources.local] max_runs = 1）: CoS は枠の解放を待って起動する。worker・reviewer の行が重複せず、起動前の失敗もない。
run_new drain-max-runs-1 LONG_HORIZON_PILOT_STUB_COS_DELAY=3 LONG_HORIZON_PILOT_STUB_WORKER_DELAY=5 \
    LONG_HORIZON_PILOT_STUB_MAX_RUNS=1 -- \
    --cases C08 --arms new --max-wall-secs 300
check "$out/drain-max-runs-1" <<'PY' || fail "max_runs=1: facts/judge"
import json, pathlib, sys
raw = pathlib.Path(sys.argv[1])
x = json.loads(next((raw / "facts").glob("*.json")).read_text())
g = json.loads((raw / "grade" / "pilot-gate.json").read_text())
inv = x["invocations"]
cos = [i for i in inv if i["role"] == "cos"]
assert cos and all(i["launch"] == "launched" and i["usage"] for i in cos), cos
assert x["usage"]["missing"] is None and x["drain"]["timed_out"] == 0, (x["usage"], x["drain"])
# root ごとに worker 1・reviewer 1（max_runs=1 で重複して数えない）。
t = next(t for t in g["trials"])
roots = len(x["roots"])
assert t["roles"].get("reviewer") == t["roles"].get("worker") and t["roles"]["worker"] >= roots, (t["roles"], roots)
assert (g["g7"], g["cos_usage_null"]) == ("pending", None), g
waits = [i["slot_wait_secs"] for i in cos]
assert max(waits) >= 1, ("no CoS run waited for a slot", waits)
print("test-pilot-drain: (2) max_runs=1: roles", t["roles"], "cos slot waits", waits)
PY

# (3) 期限超過: CoS の返事が 90 秒かかるが共有の期限は短い。未完了 run と理由を facts に残し、usage は 0 にせず、judge は unknown。
run_new drain-deadline LONG_HORIZON_PILOT_STUB_COS_DELAY=90 -- --cases C08 --arms new --max-wall-secs 75
check "$out/drain-deadline" <<'PY' || fail "deadline: facts/judge"
import json, pathlib, sys
raw = pathlib.Path(sys.argv[1])
x = json.loads(next((raw / "facts").glob("*.json")).read_text())
g = json.loads((raw / "grade" / "pilot-gate.json").read_text())
assert x["drain"]["timed_out"] >= 1 and x["drain"]["timed_out_runs"], x["drain"]
assert all(r["finished"] is False or r["usage"] is False for r in x["drain"]["timed_out_runs"]), x["drain"]
assert "cos_drain_timeout" in x["usage"]["missing"], x["usage"]
assert x["usage"]["usd"] is None and x["usage"]["tokens"] is None, x["usage"]
assert g["verdict"] == "unknown" and g["g7"] == "pending" and g["cos_drain_timeout"] is None, g
t = g["trials"][0]
assert next(c for c in t["checks"] if c["name"] == "drain")["state"] == "unknown", t["checks"]
print("test-pilot-drain: (3) deadline: timed_out", x["drain"]["timed_out"], "missing", x["usage"]["missing"], "verdict", g["verdict"])
PY

# (4) 2 試行が承認の wall の総量 100 秒を分け合う。2 本目の期限は 1 本目の開始から数える（試行ごとにリセットしない）。
run_new drain-total-wall LONG_HORIZON_PILOT_STUB_COS_DELAY=8 -- --cases C08,C09 --arms new --max-wall-secs 120 --total-wall-secs 100
check "$out/drain-total-wall" <<'PY' || fail "total wall: facts/judge"
import datetime, json, pathlib, sys
raw = pathlib.Path(sys.argv[1])
fs = sorted(((json.loads(f.read_text())) for f in (raw / "facts").glob("*.json")), key=lambda x: x["case"])
assert [x["case"] for x in fs] == ["C08", "C09"], [x["case"] for x in fs]
t = lambda s: datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp()
d1, d2 = (t(x["drain"]["deadline_at"]) for x in fs)
# 1 本目の期限は試行の上限（開始 + 120）より承認の総量（開始 + 100）が先で、2 本目も同じ期限（承認の開始 + 100）。
assert 0 <= d2 - d1 <= 2.0, (d1, d2)
# 試行ごとにリセットすると 2 本目は約 76 秒の試行をもう一度走らせて合計 150 秒前後になる。共有の期限なら 2 本目は残りで打ち切られる。
walls = [x["usage"]["wall_secs"] for x in fs]
assert sum(walls) <= 125, walls
assert fs[1]["drain"]["wall_limit_hit"] is True, fs[1]["drain"]
g = json.loads((raw / "grade" / "pilot-gate.json").read_text())
# 期限超過の試行は drain の検査が unknown（他に確定した不合格があれば全体は fail。unknown を pass にしない）。
second = next(t for t in g["trials"] if t["case"] == "C09")
assert next(c for c in second["checks"] if c["name"] == "drain")["state"] == "unknown", second["checks"]
assert g["verdict"] in ("unknown", "fail") and g["g7"] == "pending", g["verdict"]
print("test-pilot-drain: (4) total wall: shared deadline", fs[0]["drain"]["deadline_at"], "walls", [x["usage"]["wall_secs"] for x in fs],
      "wall_limit_hit", [x["drain"]["wall_limit_hit"] for x in fs], "verdict", g["verdict"])
PY
for n in drain-slow-cos drain-max-runs-1 drain-deadline drain-total-wall; do
    [ "$(cat "$out/$n.rc")" -eq 0 ] || fail "$n: runner exited $(cat "$out/$n.rc") (see $out/$n.log)"
done

if [ -n "$before" ]; then
    # 修正前の pilot（driver・judge・stub を --before-rev の版に戻した写し。runner は遅延の注入に今の版を使う）で (1)(3) を流す。cargo の build は同じ。
    old="$out/before-root"
    rm -rf "$old"; mkdir -p "$old/scripts/dev"
    for e in Cargo.toml Cargo.lock crates control-ts tests config agent-docs .cargo; do
        [ ! -e "$root/$e" ] || ln -s "$root/$e" "$old/$e"
    done
    for e in "$root"/scripts/dev/*; do
        [ "$(basename "$e")" = long-horizon-pilot ] || [ "$(basename "$e")" = long-horizon-pilot-llm.sh ] || ln -s "$e" "$old/scripts/dev/$(basename "$e")"
    done
    mkdir -p "$old/scripts/dev/long-horizon-pilot"
    for f in driver.py pilot_effect.py pilot_gate.py stub_harness.py; do
        git -C "$root" show "$before:scripts/dev/long-horizon-pilot/$f" >"$old/scripts/dev/long-horizon-pilot/$f"
    done
    # runner は今の版を使う（試験用の遅延の環境変数を stub に渡すため。--total-wall-secs を渡さなければ旧い driver に新しい引数は渡らない）。
    cp "$root/scripts/dev/long-horizon-pilot-llm.sh" "$old/scripts/dev/long-horizon-pilot-llm.sh"
    # 旧い stub は --cos-delay-secs を知らない。同じ遅延を旧い stub にも足す（遅延の注入だけ。driver・judge は旧いまま）。
    python3 -I - "$old/scripts/dev/long-horizon-pilot/stub_harness.py" <<'PY'
import sys
p = sys.argv[1]
s = open(p).read()
a = "    # 作業場所の外は読み取り専用（worker の sandbox）。記録は run の成果物の dir に置く。"
assert a in s
s = s.replace(a, '''    delay = float(sys.argv[sys.argv.index("--cos-delay-secs") + 1]) if "--cos-delay-secs" in sys.argv else 0.0
    if delay and kind != "review" and not task.get("acceptance"):
        time.sleep(delay)
''' + a, 1)
open(p, "w").write(s)
PY
    old_run() { # name env... -- args
        local name="$1"; shift
        local envs=()
        while [ "$1" != "--" ]; do envs+=("$1"); shift; done; shift
        set +e
        env "${envs[@]}" bash "$old/scripts/dev/long-horizon-pilot-llm.sh" --harness config-stub --out "$out/$name" "$@" >"$out/$name.log" 2>&1
        echo "$?" >"$out/$name.rc"
        set -e
    }
    old_run before-slow-cos LONG_HORIZON_PILOT_STUB_COS_DELAY=20 -- --cases C08 --arms new --max-wall-secs 200
    old_run before-deadline LONG_HORIZON_PILOT_STUB_COS_DELAY=90 -- --cases C08 --arms new --max-wall-secs 75
    python3 -I - "$out/before-slow-cos" "$out/before-deadline" <<'PY' || fail "before: the old pilot did not fail as expected"
import json, pathlib, sys
for name in sys.argv[1:]:
    raw = pathlib.Path(name)
    x = json.loads(next((raw / "facts").glob("*.json")).read_text())
    cos = [i for i in x["invocations"] if i["role"] == "cos"]
    nulls = [i for i in cos if not i["usage"]]
    print("test-pilot-drain: before", raw.name, "missing", x["usage"]["missing"], "cos", len(cos), "cos_usage_null", len(nulls),
          "drain" in x)
    assert nulls and "cos_usage_null" in (x["usage"]["missing"] or "") and "drain" not in x, x["usage"]
PY
fi
echo "test-pilot-drain: ok (out: $out)"
