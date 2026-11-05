#!/usr/bin/env python3
"""P1-I: long_run_soak の標本（soak.jsonl、schema celeris.long-run-soak/1）を条件ごとに集計する。

使い方: python3 -I scripts/dev/long_run_soak_summary.py <soak.jsonl> [--json <out.json>] [--markdown <out.md>]

本番の観測（2026-11-04、pid 1659338、lane_rules・cos_digest が compare）は 4 標本で、起動 26 秒後の 02:40:43Z から
1,529 秒後の 03:06:12Z までに 181,820 kB → 588,668 kB（+406,848 kB）。比べる傾きはどちらも同じ式（2 点の差 ÷ 2 点の間の
時間）で、同じ経過秒の点を使う。全区間は 0 秒 → 1,529 秒、定常は 600 秒（本番の 2 標本目）→ 1,529 秒。MB は 1,024 kB。
最小二乗の傾きは本番の 4 標本と比べる指標にならないので出さない。
exit 0 は集計できた、2 は入力の誤り（行が無い・schema が違う・条件が欠ける・restart の前に run かリースが無い）。
"""
import json
import sys

SCHEMA = "celeris.long-run-soak/1"
CONDITIONS = ["off", "v8_load", "shadow", "enabled"]
PROD = {
    "source": "agent-docs/progress/2026-11-03-ts-control-ops/p1e-shadow-measure.md",
    "condition": "shadow（本番 daemon、lane_rules・cos_digest が compare、本番 DB の events 783,007 行）",
    # (最初の標本からの秒, VmRSS kB)。02:40:43Z・02:50:43Z・03:00:43Z・03:06:12Z。
    "samples": [[0, 181820], [600, 391292], [1200, 505532], [1529, 588668]],
}
OFFSET_STEADY = PROD["samples"][1][0]
OFFSET_END = PROD["samples"][-1][0]


def slope(p, q):
    """2 点 (秒, kB) の間の傾きを MB/時で返す。時間の差が 0 以下なら None。"""
    dt = q[0] - p[0]
    if dt <= 0:
        return None
    return round((q[1] - p[1]) / 1024 / (dt / 3600), 1)


PROD["rss_start_kb"] = PROD["samples"][0][1]
PROD["rss_end_kb"] = PROD["samples"][-1][1]
PROD["rss_delta_kb"] = PROD["rss_end_kb"] - PROD["rss_start_kb"]
PROD["rss_slope_whole_mb_per_hour"] = slope(PROD["samples"][0], PROD["samples"][-1])
PROD["rss_slope_steady_mb_per_hour"] = slope(PROD["samples"][1], PROD["samples"][-1])


def nearest(points, offset):
    """経過秒が offset に最も近い点。"""
    return min(points, key=lambda p: abs(p[0] - offset))


def ratio(a, b):
    return None if a is None or not b else round(a / b, 4)


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
        after = next((r for r in rs if r["phase"] == "after_restart"), None)
        # 計測の窓の標本（restart の計測の前）。本番の標本と同じ経過秒の点を取る。
        window = [(r["elapsed_ms"] / 1000, r["rss_kb"]) for r in [start] + cyc]
        at_0, at_steady, at_end = window[0], nearest(window, OFFSET_STEADY), nearest(window, OFFSET_END)
        slope_whole, slope_steady = slope(at_0, at_end), slope(at_steady, at_end)
        # V8 heap も窓の中だけを読む（restart の後は新しい isolate）。
        rs_window = [start] + cyc + [end]
        heaps = [r["v8_used_heap_bytes"] for r in rs_window if r.get("v8_used_heap_bytes") is not None]
        totals = [r["v8_total_heap_bytes"] for r in rs_window if r.get("v8_total_heap_bytes") is not None]
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
            "rss_max_kb": max(r["rss_kb"] for r in rs_window),
            "rss_delta_kb": end["rss_kb"] - start["rss_kb"],
            "rss_anon_delta_kb": end["rss_anon_kb"] - start["rss_anon_kb"],
            "rss_at_offsets": {"start": at_0, "steady": at_steady, "end": at_end},
            "rss_delta_same_span_kb": at_end[1] - at_0[1],
            "rss_slope_whole_mb_per_hour": slope_whole,
            "rss_slope_steady_mb_per_hour": slope_steady,
            "rss_delta_ratio_to_prod": ratio(at_end[1] - at_0[1], PROD["rss_delta_kb"]),
            "rss_slope_whole_ratio_to_prod": ratio(slope_whole, PROD["rss_slope_whole_mb_per_hour"]),
            "rss_slope_steady_ratio_to_prod": ratio(slope_steady, PROD["rss_slope_steady_mb_per_hour"]),
            "threads_start": start["threads"],
            "threads_end": end["threads"],
            "isolates": end["isolates"],
            "v8_used_heap_start": heaps[0] if heaps else None,
            "v8_used_heap_end": heaps[-1] if heaps else None,
            "v8_used_heap_max": max(heaps) if heaps else None,
            "v8_total_heap_max": max(totals) if totals else None,
            "v8_native_contexts_end": end.get("v8_native_contexts"),
            "v8_generation_start": start.get("v8_generation"),
            "v8_generation_end": end.get("v8_generation"),
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
            "restart_runs_running_before_crash": restart.get("runs_running_before_crash") if restart else None,
            "restart_leases_held_before_crash": restart.get("leases_held_before_crash") if restart else None,
            "restart_lease_left_secs_min": round(restart["lease_left_secs_min"]) if restart else None,
            "restart_reclaimed_while_alive": restart.get("reclaimed_while_alive") if restart else None,
            "restart_isolate_start_ms": restart.get("isolate_start_ms") if restart else None,
            "restart_dispatcher_build_ms": restart.get("dispatcher_build_ms") if restart else None,
            "restart_first_tick_ms": restart.get("first_tick_ms") if restart else None,
            "restart_reclaim_ms": restart.get("reclaim_ms") if restart else None,
            "restart_settled_ms": restart.get("settled_ms") if restart else None,
            "restart_old_runs_interrupted": restart.get("old_runs_interrupted") if restart else None,
            "restart_orphan_notes": restart.get("orphan_takeover_notes") if restart else None,
            "restart_new_runs_started": restart.get("new_runs_started") if restart else None,
            "runs_running_after_restart": after["runs_running"] if after else None,
            "leases_held_after_restart": after["leases_held"] if after else None,
            "tasks_blocked_at_restart": restart.get("tasks_blocked") if restart else None,
            "leases_held_end": end["leases_held"],
            "llm_launches": 0,
        }
    return out


def markdown(summary):
    keys = [
        ("wall_secs", "壁時計（秒）"), ("dispatched", "dispatch 数"), ("rss_start_kb", "RSS 開始（kB）"),
        ("rss_end_kb", "RSS 終了（kB）"), ("rss_delta_kb", "RSS 増分（kB）"),
        ("rss_delta_same_span_kb", "RSS 増分 同じ 1,529 秒（kB）"), ("rss_delta_ratio_to_prod", "同 本番比"),
        ("rss_slope_whole_mb_per_hour", "RSS 傾き 全区間 0→1,529 秒（MB/時）"), ("rss_slope_whole_ratio_to_prod", "同 本番比"),
        ("rss_slope_steady_mb_per_hour", "RSS 傾き 定常 600→1,529 秒（MB/時）"), ("rss_slope_steady_ratio_to_prod", "同 本番比"),
        ("threads_end", "threads 終了"),
        ("v8_used_heap_end", "V8 used heap 終了（B）"), ("v8_total_heap_max", "V8 total heap 最大（B）"),
        ("compare_rows_per_dispatch", "比較行 / dispatch"), ("lag_ms_first_cycle_p50", "遅延 p50 最初の cycle（ms）"),
        ("lag_ms_last_cycle_p50", "遅延 p50 最後の cycle（ms）"), ("lag_ms_p95", "遅延 p95 全体（ms）"),
        ("db_list_us_last", "store.list 最後（µs）"),
        ("restart_runs_running_before_crash", "SIGKILL の前の running の run"), ("restart_leases_held_before_crash", "SIGKILL の前のリース"),
        ("restart_reclaim_ms", "SIGKILL から元の run の回収まで（ms）"), ("restart_settled_ms", "SIGKILL から再実行の終わりまで（ms）"),
        ("runs_running_after_restart", "restart 後の running の run"), ("leases_held_after_restart", "restart 後のリース"),
    ]
    conds = [c for c in CONDITIONS if c in summary]
    lines = ["| 指標 | " + " | ".join(conds) + " | 本番の観測 |", "|---|" + "---:|" * (len(conds) + 1)]
    prod = {"rss_start_kb": PROD["rss_start_kb"], "rss_end_kb": PROD["rss_end_kb"], "rss_delta_kb": PROD["rss_delta_kb"],
            "rss_delta_same_span_kb": PROD["rss_delta_kb"], "rss_slope_whole_mb_per_hour": PROD["rss_slope_whole_mb_per_hour"],
            "rss_slope_steady_mb_per_hour": PROD["rss_slope_steady_mb_per_hour"], "wall_secs": OFFSET_END}
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
    # 回収の対象が無い restart は計測にならない。
    vacuous = [c for c, v in summary.items() if not (v["restart_runs_running_before_crash"] and v["restart_leases_held_before_crash"])]
    if vacuous:
        print(f"restart without runs or leases: {vacuous}", file=sys.stderr)
        return 2
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
