#!/usr/bin/env python3
"""Summarize the four sequential release measurements, rejecting missing or duplicate cells."""
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
cells = {}
for arm in ("before", "after"):
    for cond in ("off", "shadow"):
        path = ROOT / f"{arm}-{cond}.jsonl"
        rows = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
        assert len(rows) == 4, (path, "expected four cells")
        assert sorted(row["blocked"] for row in rows) == [0, 600, 2000, 10000], path
        for row in rows:
            assert row["cond"] == cond, path
            assert row["idle_ticks"] == row["dispatches"] == 5, path
            assert row["integrity"] == "ok", path
            assert row["tasks"] == row["blocked"], path
            assert row["runs_rows"] == row["blocked"] + row["dispatches"], path
            assert row["missing_or_double_runs"] == row["remaining_leases"] == 0, path
            cells[arm, cond, row["blocked"]] = row

summary = []
print("| 条件 | blocked | dispatch p50 前→後 (ms) | restart 最初の tick 前→後 (s) | dispatch ≤1秒 |")
print("|---|---:|---:|---:|---|")
for cond in ("off", "shadow"):
    for n in (0, 600, 2000, 10000):
        before, after = (cells[arm, cond, n] for arm in ("before", "after"))
        old_lag, lag = (r["dispatch_lag_ms_p50"] for r in (before, after))
        old_tick, tick = (r["first_tick_us"] / 1e6 for r in (before, after))
        summary.append({
            "cond": cond, "blocked": n,
            "dispatch_before_ms": old_lag, "dispatch_after_ms": lag,
            "dispatch_under_1s": lag <= 1000,
            "dispatch_under_500ms": lag <= 500,
            "restart_before_s": old_tick, "restart_after_s": tick,
            "restart_ratio": tick / old_tick,
            "restart_under_historical_half": tick <= 9.4,
            "restart_under_same_profile_half": tick <= old_tick / 2,
            "small_scale_regression_pass": lag <= old_lag * 1.2 if n in (600, 2000) else None,
        })
        print(f"| {cond} | {n:,} | {old_lag} → {lag} | {old_tick:.3f} → {tick:.3f} | {'合格' if lag <= 1000 else '不合格'} |")
(ROOT / "summary.json").write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n")
