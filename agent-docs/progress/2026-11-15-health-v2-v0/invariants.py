#!/usr/bin/env python3
"""V0 invariants: 不変条件 → HEAD の試験・検査の対応表（invariants.json）を流す。

    python3 agent-docs/progress/2026-11-15-health-v2-v0/invariants.py --run \
        agent-docs/progress/2026-11-15-health-v2-v0/invariants.json [--record] [--only <id の部分文字列>]

- 表の各行の checks のうち mode=run の command を、同じ command は 1 回だけ、表の順に流す。
- mode=release_gate の command は流さない（userns・web・control-ts は release gate で確かめる）。
- 1 件でも exit が 0 でない、または 0 件の試験にしか当たらない（nextest の Starting 0 tests、
  unittest の Ran 0 tests）command があれば exit 1。表の形が壊れていれば exit 2。
- --record は各行の result を書き戻す（commit hash は書かない。検査は提出時の HEAD で再実行する）。
"""

import argparse
import json
import os
import re
import shlex
import subprocess
import sys
import time

REQUIRED_ROW = ("id", "source", "kind", "invariant", "evidence_path", "coverage", "checks", "result")
MODES = ("run", "release_gate")
COVERAGE = ("full", "partial", "none", "none_in_wu")

NEXTEST_START = re.compile(r"Starting (\d+) tests? across")
NEXTEST_SUMMARY = re.compile(r"Summary \[\s*[\d.]+s\]\s+(\d+) tests? run: (\d+) passed")
UNITTEST_RAN = re.compile(r"^Ran (\d+) tests?", re.M)


def repo_root(path):
    out = subprocess.run(["git", "-C", os.path.dirname(os.path.abspath(path)), "rev-parse", "--show-toplevel"],
                         capture_output=True, text=True, check=True)
    return out.stdout.strip()


def validate(doc, root):
    errors = []
    rows = doc.get("rows")
    if not isinstance(rows, list) or not rows:
        return ["rows が無い"]
    seen = set()
    for r in rows:
        rid = r.get("id", "?")
        for k in REQUIRED_ROW:
            if k not in r:
                errors.append(f"{rid}: {k} が無い")
        if rid in seen:
            errors.append(f"{rid}: id が重複")
        seen.add(rid)
        if r.get("coverage") not in COVERAGE:
            errors.append(f"{rid}: coverage が {COVERAGE} のどれでもない")
        ev = r.get("evidence_path")
        if ev and not os.path.exists(os.path.join(root, ev)):
            errors.append(f"{rid}: evidence_path {ev} が HEAD に無い")
        checks = r.get("checks") or []
        if not checks and r.get("coverage") != "none":
            errors.append(f"{rid}: checks が空なのに coverage が none でない")
        if r.get("coverage") != "full" and not r.get("gap"):
            errors.append(f"{rid}: coverage={r.get('coverage')} なのに gap（未検証の理由）が無い")
        for c in checks:
            if c.get("mode") not in MODES or not c.get("command"):
                errors.append(f"{rid}: check に mode か command が無い")
            if c.get("mode") == "release_gate" and not c.get("reason"):
                errors.append(f"{rid}: release_gate の check に reason が無い")
    return errors


def count_tests(cmd, output):
    """command の種類ごとに走った試験の数を出す。数えられない検査 script は None。"""
    if cmd.startswith("cargo nextest"):
        m = NEXTEST_SUMMARY.search(output)
        if m:
            return int(m.group(1))
        m = NEXTEST_START.search(output)
        return int(m.group(1)) if m else 0
    if "unittest" in cmd:
        m = UNITTEST_RAN.findall(output)
        return sum(int(x) for x in m) if m else 0
    return None


def run_one(cmd, root):
    t0 = time.monotonic()
    p = subprocess.run(shlex.split(cmd), cwd=root, capture_output=True, text=True)
    out = p.stdout + p.stderr
    n = count_tests(cmd, out)
    ok = p.returncode == 0 and (n is None or n > 0)
    return {"exit": p.returncode, "tests_run": n, "ok": ok, "secs": round(time.monotonic() - t0, 1), "tail": out[-3000:]}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--run", required=True, metavar="INVARIANTS_JSON")
    ap.add_argument("--record", action="store_true", help="result を JSON に書き戻す")
    ap.add_argument("--only", default=None, help="id にこの文字列を含む行だけ")
    a = ap.parse_args()

    with open(a.run, encoding="utf-8") as f:
        doc = json.load(f)
    root = repo_root(a.run)
    errors = validate(doc, root)
    if errors:
        for e in errors:
            print("schema:", e, file=sys.stderr)
        return 2

    rows = [r for r in doc["rows"] if a.only is None or a.only in r["id"]]
    results = {}
    order = []
    for r in rows:
        for c in r["checks"]:
            if c["mode"] == "run" and c["command"] not in results:
                results[c["command"]] = None
                order.append(c["command"])
    print(f"rows={len(rows)} run_commands={len(order)}", flush=True)
    for i, cmd in enumerate(order, 1):
        res = run_one(cmd, root)
        results[cmd] = res
        mark = "ok  " if res["ok"] else "FAIL"
        print(f"[{i}/{len(order)}] {mark} exit={res['exit']} tests={res['tests_run']} {res['secs']}s  {cmd}", flush=True)
        if not res["ok"]:
            print(res["tail"], flush=True)

    failed_rows = []
    counts = {"pass": 0, "fail": 0, "release_gate": 0, "unverified": 0}
    for r in rows:
        run = [c for c in r["checks"] if c["mode"] == "run"]
        rg = [c for c in r["checks"] if c["mode"] == "release_gate"]
        if run:
            ok = all(results[c["command"]]["ok"] for c in run)
            status = "pass" if ok else "fail"
            if not ok:
                failed_rows.append(r["id"])
        elif rg:
            status = "release_gate"
        else:
            status = "unverified"
        counts[status] += 1
        for c in run:
            res = results[c["command"]]
            c["result"] = {"exit": res["exit"], "tests_run": res["tests_run"]}
        r["result"] = {"status": status, "release_gate_checks": len(rg), "checked": "提出時の HEAD で invariants.py --run"}
        if r["coverage"] != "full":
            r["result"]["coverage"] = r["coverage"]
    print("rows: " + " ".join(f"{k}={v}" for k, v in counts.items()))
    bad = [c for c in order if not results[c]["ok"]]
    print(f"commands: ok={len(order) - len(bad)} fail={len(bad)}")
    if a.record and a.only is None:
        with open(a.run, "w", encoding="utf-8") as f:
            json.dump(doc, f, ensure_ascii=False, indent=1)
            f.write("\n")
    if bad:
        print("failed rows: " + ", ".join(failed_rows), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
