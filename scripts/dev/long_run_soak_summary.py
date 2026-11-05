#!/usr/bin/env python3
"""P1-I: long_run_soak の標本（soak.jsonl、schema celeris.long-run-soak/1）を条件ごとに集計する。

使い方: python3 -I scripts/dev/long_run_soak_summary.py <soak.jsonl> [--json <out.json>] [--markdown <out.md>]

本番の観測（2026-11-04、pid 1659338、lane_rules・cos_digest が compare）: 起動 26 秒後 181,820 kB →
26 分後 588,668 kB（+406,848 kB、約 +939 MB/時）。同じ指標（VmRSS の起動直後からの増分と 1 時間あたりの傾き）で並べる。
exit 0 は集計できた、2 は入力の誤り（行が無い・schema が違う・条件が欠ける）。
"""
import json
import sys

SCHEMA = "celeris.long-run-soak/1"
CONDITIONS = ["off", "v8_load", "shadow", "enabled"]
PROD = {
    "source": "agent-docs/progress/2026-11-03-ts-control-ops/p1e-shadow-measure.md",
    "condition": "shadow（本番 daemon、lane_rules・cos_digest が compare、本番 DB の events 783,007 行）",
    "rss_start_kb": 181820,
    "rss_end_kb": 588668,
    "minutes": 26.0,
}
PROD["rss_delta_kb"] = PROD["rss_end_kb"] - PROD["rss_start_kb"]
PROD["rss_slope_mb_per_hour"] = round(PROD["rss_delta_kb"] / 1024 / (PROD["minutes"] / 60), 1)


def slope_per_hour(points):
    """(秒, kB) の最小二乗の傾きを MB/時で返す。点が 2 未満なら None。"""
    if len(points) < 2:
        return None
    n = len(points)
    mx = sum(p[0] for p in points) / n
    my = sum(p[1] for p in points) / n
    den = sum((p[0] - mx) ** 2 for p in points)
    if den == 0:
        return None
    k = sum((p[0] - mx) * (p[1] - my) for p in points) / den
    return round(k * 3600 / 1024, 1)


def summarize(rows):
    out = {}
    for cond in CONDITIONS:
        rs = [r for r in rows if r.get("condition") == cond]
        if not rs:
            continue
        start = next(r for r in rs if r["phase"] == "start")
        end = next((r for r in rs if r["phase"] == "end"), rs[-1])
        cyc = [r for r in rs if r["phase"] == "cycle"]
        restart = next((r for r in rs if r["phase"] == "restart"), None)
        # 起動直後の warm-up（最初の cycle）を除いた定常の傾きも出す。
        steady = [(r["elapsed_ms"] / 1000, r["rss_kb"]) for r in cyc[1:]]
        heaps = [r["v8_used_heap_bytes"] for r in rs if r.get("v8_used_heap_bytes") is not None]
        totals = [r["v8_total_heap_bytes"] for r in rs if r.get("v8_total_heap_bytes") is not None]
        dispatched = sum(r["lag_samples"] for r in cyc)
        out[cond] = {
            "test": start["test"],
            "seed_events": start.get("seed_events"),
            "wall_secs": round(end["elapsed_ms"] / 1000, 1),
            "cycles": len(cyc),
            "dispatched": dispatched,
            "rss_start_kb": start["rss_kb"],
            "rss_first_cycle_kb": cyc[0]["rss_kb"] if cyc else None,
            "rss_end_kb": end["rss_kb"],
            "rss_max_kb": max(r["rss_kb"] for r in rs),
            "rss_delta_kb": end["rss_kb"] - start["rss_kb"],
            "rss_anon_delta_kb": end["rss_anon_kb"] - start["rss_anon_kb"],
            "rss_slope_mb_per_hour_all": slope_per_hour([(r["elapsed_ms"] / 1000, r["rss_kb"]) for r in cyc]),
            "rss_slope_mb_per_hour_steady": slope_per_hour(steady),
            "threads_start": start["threads"],
            "threads_end": end["threads"],
            "isolates": end["isolates"],
            "v8_used_heap_start": heaps[0] if heaps else None,
            "v8_used_heap_end": heaps[-1] if heaps else None,
            "v8_used_heap_max": max(heaps) if heaps else None,
            "v8_total_heap_max": max(totals) if totals else None,
            "v8_native_contexts_end": end.get("v8_native_contexts"),
            "v8_restarts_end": end.get("v8_restarts"),
            "v8_timeouts_end": end.get("v8_timeouts"),
            "compare_rows_added": end["compare_events"] - start["compare_events"],
            "compare_rows_per_dispatch": round((end["compare_events"] - start["compare_events"]) / dispatched, 3) if dispatched else None,
            "events_rows_end": end["events_rows"],
            "events_per_dispatch": round((end["events_rows"] - start["events_rows"]) / dispatched, 2) if dispatched else None,
            "lag_ms_first_cycle_p50": cyc[0]["lag_ms_p50"] if cyc else None,
            "lag_ms_last_cycle_p50": cyc[-1]["lag_ms_p50"] if cyc else None,
            "lag_ms_p50": end["lag_ms_p50"],
            "lag_ms_p95": end["lag_ms_p95"],
            "lag_ms_max": end["lag_ms_max"],
            "db_list_us_first": cyc[0]["db_list_us"] if cyc else None,
            "db_list_us_last": cyc[-1]["db_list_us"] if cyc else None,
            "db_count_events_us_last": cyc[-1]["db_count_events_us"] if cyc else None,
            "tasks_end": end["tasks"],
            "tasks_blocked_end": end["tasks_blocked"],
            "runs_end": end["runs"],
            "runs_running_end": end["runs_running"],
            "runs_running_after_restart": restart["runs_running"] if restart else None,
            "restart_recover_ms": restart.get("restart_recover_ms") if restart else None,
            "leases_held_end": end["leases_held"],
            "llm_launches": 0,
        }
    return out


def markdown(summary):
    keys = [
        ("wall_secs", "壁時計（秒）"), ("dispatched", "dispatch 数"), ("rss_start_kb", "RSS 開始（kB）"),
        ("rss_end_kb", "RSS 終了（kB）"), ("rss_delta_kb", "RSS 増分（kB）"),
        ("rss_slope_mb_per_hour_steady", "RSS 傾き 定常（MB/時）"), ("threads_end", "threads 終了"),
        ("v8_used_heap_end", "V8 used heap 終了（B）"), ("v8_total_heap_max", "V8 total heap 最大（B）"),
        ("compare_rows_per_dispatch", "比較行 / dispatch"), ("lag_ms_first_cycle_p50", "遅延 p50 最初の cycle（ms）"),
        ("lag_ms_last_cycle_p50", "遅延 p50 最後の cycle（ms）"), ("lag_ms_p95", "遅延 p95 全体（ms）"),
        ("db_list_us_last", "store.list 最後（µs）"), ("runs_running_after_restart", "restart 後の running run"),
    ]
    conds = [c for c in CONDITIONS if c in summary]
    lines = ["| 指標 | " + " | ".join(conds) + " | 本番の観測 |", "|---|" + "---:|" * (len(conds) + 1)]
    prod = {"rss_start_kb": PROD["rss_start_kb"], "rss_end_kb": PROD["rss_end_kb"], "rss_delta_kb": PROD["rss_delta_kb"],
            "rss_slope_mb_per_hour_steady": PROD["rss_slope_mb_per_hour"], "wall_secs": PROD["minutes"] * 60}
    for k, label in keys:
        cells = ["-" if summary[c].get(k) is None else str(summary[c][k]) for c in conds]
        lines.append(f"| {label} | " + " | ".join(cells) + f" | {prod.get(k, '-')} |")
    return "\n".join(lines) + "\n"


def main(argv):
    if len(argv) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    rows = []
    with open(argv[1], encoding="utf-8") as f:
        for line in f:
            if line.strip():
                r = json.loads(line)
                if r.get("schema") != SCHEMA:
                    print(f"bad schema: {r.get('schema')}", file=sys.stderr)
                    return 2
                rows.append(r)
    summary = summarize(rows)
    missing = [c for c in CONDITIONS if c not in summary]
    doc = {"schema": "celeris.long-run-soak-summary/1", "prod_observation": PROD, "conditions": summary, "missing": missing}
    args = argv[2:]
    if "--json" in args:
        with open(args[args.index("--json") + 1], "w", encoding="utf-8") as f:
            json.dump(doc, f, ensure_ascii=False, indent=1, sort_keys=True)
            f.write("\n")
    if "--markdown" in args:
        with open(args[args.index("--markdown") + 1], "w", encoding="utf-8") as f:
            f.write(markdown(summary))
    print(json.dumps(doc, ensure_ascii=False, sort_keys=True))
    return 2 if missing else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
