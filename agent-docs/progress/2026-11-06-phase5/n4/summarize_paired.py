#!/usr/bin/env python3
"""Summarize the paired rounds: before/after x off/shadow run at the same time in each round.

Each round started the four release binaries together, so host load applies to every arm alike.
Dispatch samples are pooled over rounds (5 per cell per round); restart first tick is listed per round.
Fails on a missing, duplicate or inconsistent cell.
"""
import json
import statistics
from pathlib import Path

ROOT = Path(__file__).resolve().parent / "paired"
ROUNDS = {"round1": (0, 600, 2000, 10000), "round2": (0, 600, 2000, 10000),
          "round3": (0, 600, 2000, 10000), "round4": (10000,)}
cells = {}
for rnd, sizes in ROUNDS.items():
    for arm in ("before", "after"):
        for cond in ("off", "shadow"):
            path = ROOT / rnd / f"{arm}-{cond}.jsonl"
            rows = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
            assert sorted(r["blocked"] for r in rows) == list(sizes), path
            for r in rows:
                assert r["cond"] == cond, path
                assert r["idle_ticks"] == r["dispatches"] == 5, path
                assert r["integrity"] == "ok", path
                assert r["tasks"] == r["blocked"], path
                assert r["runs_rows"] == r["blocked"] + r["dispatches"], path
                assert r["missing_or_double_runs"] == r["remaining_leases"] == 0, path
                cells.setdefault((arm, cond, r["blocked"]), []).append(r)

summary = []
print("| 条件 | blocked | 標本 | dispatch p50 前→後 (ms) | 後/前 | restart 最初の tick 中央値 前→後 (s) | dispatch ≤1秒 | ≤500 ms |")
print("|---|---:|---:|---:|---:|---:|---|---|")
for cond in ("off", "shadow"):
    for n in (0, 600, 2000, 10000):
        b, a = cells["before", cond, n], cells["after", cond, n]
        lag_b = [x for r in b for x in r["dispatch_lag_ms"]]
        lag_a = [x for r in a for x in r["dispatch_lag_ms"]]
        p50_b, p50_a = statistics.median(lag_b), statistics.median(lag_a)
        tick_b = statistics.median(r["first_tick_us"] / 1e6 for r in b)
        tick_a = statistics.median(r["first_tick_us"] / 1e6 for r in a)
        row = {
            "cond": cond, "blocked": n, "samples": len(lag_a),
            "dispatch_before_ms": p50_b, "dispatch_after_ms": p50_a,
            "dispatch_ratio": round(p50_a / p50_b, 3),
            "dispatch_under_1s": p50_a <= 1000, "dispatch_under_500ms": p50_a <= 500,
            "per_round_p50_before": [r["dispatch_lag_ms_p50"] for r in b],
            "per_round_p50_after": [r["dispatch_lag_ms_p50"] for r in a],
            "restart_before_s": [round(r["first_tick_us"] / 1e6, 3) for r in b],
            "restart_after_s": [round(r["first_tick_us"] / 1e6, 3) for r in a],
            "restart_median_ratio": round(tick_a / tick_b, 3),
            "small_scale_regression_pass": p50_a <= p50_b * 1.2 if n in (600, 2000) else None,
        }
        summary.append(row)
        print(f"| {cond} | {n:,} | {len(lag_a)} | {p50_b:g} → {p50_a:g} | {p50_a / p50_b:.2f} "
              f"| {tick_b:.3f} → {tick_a:.3f} | {'合格' if p50_a <= 1000 else '不合格'} "
              f"| {'合格' if p50_a <= 500 else '不合格'} |")
(ROOT / "summary.json").write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n")
