#!/usr/bin/env python3
"""shadow-bench: long-horizon bench の腕 shadow（R1 を本物の controller の経路で shadow にした記録）を集計する（LLM 0）。

使い方（repo の root で。stdlib だけ）:
  python3 -I agent-docs/progress/2026-11-27-strategy-v1/shadow_bench.py run            # raw から shadow-bench.json を書く
  python3 -I agent-docs/progress/2026-11-27-strategy-v1/shadow_bench.py verify         # 再計算して byte 一致を確かめる（exit 0/1）
  python3 -I agent-docs/progress/2026-11-27-strategy-v1/shadow_bench.py compare <dir>  # <dir>（raw）の case ごとの値を commit 済みと比べる

raw は shadow-bench/raw/shadow/<case>.json（`celeris.long-horizon-shadow/1`。試験 `long_horizon_shadow_` が書く）。
判定（計画で事前に固定。ADR 2026-11-27-r1-stall-shadow D4）:
- 発火: 判断の行のうち rule id が `pc/r1-stall`・行動が escalate のもの。点は (時刻, key の終端の子の数)。
- 全 Outcome 到達: 必要な Outcome（台本の木の判定）が全部到達した時刻（最後の到達）。誤検知はそれより前の発火。
- F2: 全 Outcome に到達した case のうち、到達前に発火した case が 2 件以上。
- F1: controller が記録した途中到達（`mark_milestone` の判断）の後の最初の stall の no_progress_streak が、その間に終端に
  なった root の数と違う。
- 途中到達（全部ではない到達。台本の木）のある case が 3 件未満なら inconclusive（falsified は false）。それ以外は F1 か F2 で falsified。
- r1-bench の予測: r1-bench.json の同じ case の old.proactive の trial（除外なら予測なし）と、r1_bench.judge_trial を同じ記録に
  当てた Python の規則の発火（K = 3）を並べる。
"""
from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
RAW = HERE / "shadow-bench" / "raw"
OUT = HERE / "shadow-bench.json"
R1_BENCH = HERE / "r1-bench.json"
CASES = [f"C{i:02d}" for i in range(1, 11)]
RULE = "pc/r1-stall"
K = 3
SCHEMA = "celeris.shadow-bench/1"
COMPARED = ("fires", "full_reach_at", "midway_reaches", "false_positives", "f1_violations")


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


r1b = load_module("r1_bench", HERE / "r1_bench.py")


def terminal_times(rec: dict) -> list[int]:
    out = []
    for r in rec["roots"]:
        t = [s["at"] for s in r["statuses"] if s["status"] in ("done", "failed", "cancelled")]
        if t:
            out.append(t[0])
    return sorted(out)


def judge_case(rec: dict, r1_bench: dict) -> dict:
    case = rec["case"]
    fires = []
    for d in rec["decisions"]:
        if d["rule_id"] == RULE and d["kind"] == "escalate":
            n = int(str(d["key"]).rsplit(":", 1)[-1])
            fires.append({"at": d["at"], "terminal_children": n, "effect_state": d["effect_state"]})
    waiting = sum(1 for d in rec["decisions"] if d["rule_id"] == f"{RULE}/waiting")
    req = [o for o in rec["outcomes"] if o["required"]]
    times = [o["reached_at"] for o in req]
    full = max(times) if req and all(t is not None for t in times) else None
    midway = sorted({t for t in times if t is not None and (full is None or t < full)})
    fps = [f for f in fires if full is not None and f["at"] < full]
    # F1: controller が記録した途中到達の後の最初の stall。
    term = terminal_times(rec)
    stalls = [s for s in rec["stalls"] if isinstance(s.get("stall"), dict)]
    recorded = [d for d in rec["decisions"] if d["kind"] == "mark_milestone" and d["effect_state"] != "failed"]
    f1, f1_tested = [], 0
    for m in recorded:
        if full is not None and m["at"] >= full:
            continue
        nxt = next((s for s in stalls if s["at"] > m["at"]), None)
        if nxt is None:
            continue
        f1_tested += 1
        expect = sum(1 for t in term if m["at"] < t <= nxt["at"])
        if nxt["stall"]["no_progress_streak"] != expect:
            f1.append({"reach_at": m["at"], "outcome": m["key"], "stall_at": nxt["at"],
                       "streak": nxt["stall"]["no_progress_streak"], "expected": expect})
    escalate_seen = [s["at"] for s in stalls if s["stall"].get("escalate")]
    guards = sorted({d["rule_id"] for d in rec["decisions"] if str(d["rule_id"]).startswith("pc/guard/")})
    py = r1b.judge_trial(rec)
    py_fired = py["fired"].get(str(K), [])
    bench = [t for t in r1_bench["trials"] if t["case"] == case and t["arm"] == "old.proactive"]
    rb = bench[0] if bench else None
    return {
        "case": case,
        "fires": fires,
        "r1_waiting": waiting,
        "escalate_seen_in_snapshot": escalate_seen,
        "guards": guards,
        "full_reach_at": full,
        "midway_reaches": midway,
        "controller_recorded_reaches": [{"at": m["at"], "outcome": m["key"]} for m in recorded],
        "false_positives": fps,
        "f1_tested": f1_tested,
        "f1_violations": f1,
        "terminal_roots": len(term),
        "decisions": len(rec["decisions"]),
        "effects": rec["effects"],
        "python_rule": {
            "fired_terminal_children": py_fired,
            "fired_before_full_reach": py["fired_before_full_reach"].get(str(K), []),
            "matches_controller": sorted(f["terminal_children"] for f in fires) == sorted(py_fired),
        },
        "r1_bench": None if rb is None else {
            "trial": rb["trial"],
            "included": rb["included"],
            "fired": rb["fired"].get(str(K), []),
            "exclusion": [e["code"] for e in rb["exclusion"]],
            "matches": None if not rb["included"] else rb["fired"].get(str(K), []) == py_fired,
        },
    }


def compute(raw: Path) -> dict:
    r1_bench = json.loads(R1_BENCH.read_text())
    cases = []
    for c in CASES:
        rec = json.loads((raw / "shadow" / f"{c}.json").read_text())
        if rec.get("schema") != "celeris.long-horizon-shadow/1" or rec.get("case") != c or rec.get("stall_k") != K:
            raise SystemExit(f"{c}: unexpected raw (schema/case/stall_k)")
        cases.append(judge_case(rec, r1_bench))
    full = [c for c in cases if c["full_reach_at"] is not None]
    fp_cases = [c["case"] for c in full if c["false_positives"]]
    midway_cases = [c["case"] for c in cases if c["midway_reaches"]]
    f1_cases = [c["case"] for c in cases if c["f1_violations"]]
    inconclusive = len(midway_cases) < 3
    f2 = len(fp_cases) >= 2
    py_fp = [c["case"] for c in full if c["python_rule"]["fired_before_full_reach"]]
    return {
        "schema": SCHEMA,
        "k": K,
        "rule": RULE,
        "llm_calls": 0,
        "cases": cases,
        "counts": {
            "cases": len(cases),
            "full_reach_cases": len(full),
            "midway_reach_cases": midway_cases,
            "fire_points": sum(len(c["fires"]) for c in cases),
            "false_positive_cases": fp_cases,
            "f1_tested_reaches": sum(c["f1_tested"] for c in cases),
            "f1_violation_cases": f1_cases,
            "effects_total": {k: sum(c["effects"][k] for c in cases) for k in sorted(cases[0]["effects"])},
            "python_rule_false_positive_cases": py_fp,
            "python_rule_matches_controller": [c["case"] for c in cases if c["python_rule"]["matches_controller"]],
        },
        "f2": f2,
        "f1": bool(f1_cases),
        "inconclusive": inconclusive,
        "falsified": (not inconclusive) and (f2 or bool(f1_cases)),
    }


def render(doc: dict) -> bytes:
    return (json.dumps(doc, ensure_ascii=False, indent=1, sort_keys=True) + "\n").encode()


def main(argv: list[str]) -> int:
    if len(argv) == 2 and argv[1] == "run":
        OUT.write_bytes(render(compute(RAW)))
        doc = json.loads(OUT.read_text())
        print(f"run: cases=10 fires={doc['counts']['fire_points']} midway={len(doc['counts']['midway_reach_cases'])} "
              f"inconclusive={doc['inconclusive']} falsified={doc['falsified']}")
        return 0
    if len(argv) == 2 and argv[1] == "verify":
        ok = render(compute(RAW)) == OUT.read_bytes()
        print("verify: ok" if ok else "verify: shadow-bench.json differs from the recomputation")
        return 0 if ok else 1
    if len(argv) == 3 and argv[1] == "compare":
        got = compute(Path(argv[2]))
        want = json.loads(OUT.read_text())
        bad = []
        for g, w in zip(got["cases"], want["cases"]):
            for k in COMPARED:
                if g[k] != w[k]:
                    bad.append(f"{g['case']}.{k}: {g[k]} != {w[k]}")
        for k in ("inconclusive", "falsified"):
            if got[k] != want[k]:
                bad.append(f"{k}: {got[k]} != {want[k]}")
        print("compare: ok" if not bad else "compare: differs\n" + "\n".join(bad))
        return 0 if not bad else 1
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
