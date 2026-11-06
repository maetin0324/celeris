#!/usr/bin/env python3
"""P4-F 段 3（N2）: 長時間の daemon soak の推移の集計（時刻を揃えた比較・ウォームアップ・線形の傾き・再起動）。

`daemon_soak.py summary` は最初の process の 0→1,529 秒と 600 秒以降の 2 点の傾きを出す（本番の 26 分の観測と同じ式）。
24/72 時間では足りないので、この script は次を出す。daemon も DB も触らない（`samples.jsonl`・`ops.jsonl`・daemon の log を
読むだけ）。soak の途中でも走らせてよい（途中なら verdict は `incomplete`）。

- 経過時間ごと（既定 1 時間おき）の表: RSS・RssAnon・threads・fds・malloc（in_use・free）・SQLite の page cache・V8 heap・
  generation・task（done・ready）・chat run（CoS の launch の数。fake harness なので LLM 0）・DB の bytes・wake・判断の数。
- process（pid）ごとの区間: ウォームアップ（既定 7,200 秒）の後の RssAnon の最小二乗の傾き（MB/時）と、0→ウォームアップの増分。
- 再起動: 再起動の後の process の、起動から同じ経過時間（既定 3,600 秒）の RssAnon を、最初の process の同じ経過時間の値と比べる。
- 条件の比較: 同じ経過時間の RssAnon の差（条件 − off）と、傾きの差。
- tick: daemon の log の `slow tick` の行（閾値を超えた tick だけが log に出る）の 1 時間ごとの件数・p50 の所要時間・
  p50 の `tick_cos_chat_launch`。
- 合否（runbook N2 の数値）: 完走（最終標本の t ≥ 86,000 秒・再起動あり・API の失敗なし）、傾き（ウォームアップの後 ≤ 1 MB/時、
  shadow・enabled は off + 0.5 MB/時 以下）、再起動の後（±10%）、DB（上限に達しない）。途中なら `incomplete`。

使い方:
  python3 -I scripts/dev/daemon-soak/soak_trend.py <run の dir ...> --json <file> --markdown <file>
      [--log <cond>=<daemon.log> ...] [--warmup-secs 7200] [--step-secs 3600] [--full-secs 86000]
run の dir の `daemon.log` は run の終わりに写される。途中で読むときは `--log off=<一時 home>/daemon.log` のように渡す。
"""
import argparse
import json
import pathlib
import re
import statistics
import sys

SCHEMA = "celeris.daemon-soak-trend/1"
SAMPLE_SCHEMA = "celeris.daemon-soak/1"
# runbook N2 の合格の数値。
SLOPE_MAX_MB_H = 1.0
SLOPE_VS_OFF_MAX_MB_H = 0.5
RESTART_TOLERANCE = 0.10
TICK_LINE = re.compile(r"^(\S+)\s.*tick_loop: slow tick .*?duration_ms=(\d+)")
COS_LAUNCH = re.compile(r'"tick_cos_chat_launch", (\d+)\)')
ANSI = re.compile(r"\x1b\[[0-9;]*m")


def load(d):
    d = pathlib.Path(d)
    samples = [json.loads(x) for x in (d / "samples.jsonl").read_text().splitlines() if x.strip()]
    ops_file = d / "ops.jsonl"
    ops = [json.loads(x) for x in ops_file.read_text().splitlines() if x.strip()] if ops_file.exists() else []
    if any(s.get("schema") != SAMPLE_SCHEMA for s in samples):
        raise ValueError(f"{d}: schema mismatch")
    return samples, ops


def lsq_slope(points):
    """(秒, kB) の点列の最小二乗の傾きを MB/時。点が 2 未満か時間の幅が 0 なら None。"""
    if len(points) < 2:
        return None
    mx = sum(t for t, _ in points) / len(points)
    my = sum(v for _, v in points) / len(points)
    sxx = sum((t - mx) ** 2 for t, _ in points)
    if sxx == 0:
        return None
    sxy = sum((t - mx) * (v - my) for t, v in points)
    return round(sxy / sxx * 3600 / 1024, 2)


def nearest(points, t, tol):
    """t に最も近い点（|差| ≤ tol）。無ければ None。"""
    best = min(points, key=lambda p: abs(p[0] - t), default=None)
    return best if best is not None and abs(best[0] - t) <= tol else None


def pm(s, k):
    return (s.get("process_memory") or {}).get(k)


def row_of(s):
    p = s.get("proc") or {}
    db = s.get("db") or {}
    st = db.get("tasks_by_status") or {}
    kb = lambda v: None if v is None else v // 1024
    return {
        "t": s["t"], "at": s.get("at"), "pid": s.get("pid"),
        "rss_kb": p.get("rss_kb"), "rss_anon_kb": p.get("rss_anon_kb"), "threads": p.get("threads"), "fds": s.get("fds"),
        "malloc_in_use_kb": kb(pm(s, "malloc_in_use")), "malloc_free_kb": kb(pm(s, "malloc_free")),
        "sqlite_pagecache_kb": kb(pm(s, "sqlite_pagecache_bytes")),
        "v8_used_heap_kb": kb(pm(s, "v8_used_heap")), "v8_total_heap_kb": kb(pm(s, "v8_total_heap")),
        "v8_generations": pm(s, "v8_generations"),
        "tasks": db.get("tasks"), "done": st.get("done"), "ready": st.get("ready"),
        "chat_runs": db.get("chat_runs"), "cos_inbox_items": db.get("cos_inbox_items"),
        "project_wakes": db.get("project_wakes"), "decisions": db.get("project_controller_decisions"),
        "db_mb": (s.get("db_bytes") or 0) >> 20,
        "ticks": (s.get("daemon") or {}).get("ticks"),
    }


def segments(samples):
    """pid ごとの区間（出た順）。RSS の無い標本（process が無い瞬間）は除く。"""
    segs = []
    for s in samples:
        if not (s.get("proc") or {}).get("rss_anon_kb"):
            continue
        if not segs or segs[-1]["pid"] != s.get("pid"):
            segs.append({"pid": s.get("pid"), "start_t": s["t"], "samples": []})
        segs[-1]["samples"].append(s)
    return segs


def segment_stats(seg, warmup):
    pts = [(s["t"] - seg["start_t"], s["proc"]["rss_anon_kb"]) for s in seg["samples"]]
    end = pts[-1][0]
    steady = [p for p in pts if p[0] >= warmup]
    w = nearest(pts, warmup, 600)
    out = {"pid": seg["pid"], "start_t": seg["start_t"], "secs": end, "samples": len(pts),
           "anon_start_kb": pts[0][1], "anon_end_kb": pts[-1][1], "anon_max_kb": max(v for _, v in pts),
           "warmup_secs": warmup, "warmup_delta_kb": (w[1] - pts[0][1]) if w else None,
           "steady_slope_mb_h": lsq_slope(steady) if end - warmup >= 3600 else None,
           "whole_slope_mb_h": lsq_slope(pts)}
    # 窓ごとの傾き（ウォームアップの見分け: 傾きが下がって落ち着くか）。
    out["window_slopes_mb_h"] = [
        {"from": a, "to": a + 7200, "slope": lsq_slope([p for p in pts if a <= p[0] < a + 7200])}
        for a in range(0, int(end) - 3600 + 1, 7200)]
    return out


def tick_trend(log_path):
    """slow tick の行を 1 時間（UTC の時）ごとにまとめる。log が無ければ None。"""
    path = pathlib.Path(log_path)
    if not path.exists():
        return None
    hours = {}
    with open(path, errors="replace") as f:
        for raw in f:
            if "slow tick" not in raw:
                continue
            line = ANSI.sub("", raw)
            m = TICK_LINE.match(line)
            if not m:
                continue
            c = COS_LAUNCH.search(line)
            hours.setdefault(m.group(1)[:13], []).append((int(m.group(2)), int(c.group(1)) // 1000 if c else None))
    rows = []
    for h, v in sorted(hours.items()):
        launch = [y for _, y in v if y is not None]
        rows.append({"hour": h, "slow_ticks": len(v), "p50_ms": round(statistics.median(x for x, _ in v)),
                     "max_ms": max(x for x, _ in v),
                     "p50_cos_chat_launch_ms": round(statistics.median(launch)) if launch else None})
    return rows


def summarize(d, cond_log=None, warmup=7200, step=3600, restart_at=3600):
    samples, ops = load(d)
    if not samples:
        raise ValueError(f"{d}: no samples")
    cond = samples[0]["condition"]
    rows = [row_of(s) for s in samples]
    last_t = samples[-1]["t"]
    table = []
    for t in range(0, int(last_t) + 1, step):
        r = nearest([(x["t"], x) for x in rows], t, 300)
        if r:
            table.append({"elapsed_h": t / 3600, **r[1]})
    segs = [segment_stats(s, warmup) for s in segments(samples)]
    restart = next((o for o in ops if o.get("step") == "restart"), None)
    stop = next((o for o in ops if o.get("step") == "stop"), None)
    after_restart = None
    if len(segs) >= 2:
        first, second = segments(samples)[0], segments(samples)[1]
        pa = nearest([(s["t"] - first["start_t"], s["proc"]["rss_anon_kb"]) for s in first["samples"]], restart_at, 300)
        pb = nearest([(s["t"] - second["start_t"], s["proc"]["rss_anon_kb"]) for s in second["samples"]], restart_at, 300)
        if pa and pb:
            after_restart = {"elapsed_secs": restart_at, "before_kb": pa[1], "after_kb": pb[1],
                             "ratio": round(pb[1] / pa[1], 3)}
    lat = [x for s in samples for x in (s.get("db") or {}).get("dispatch_latency_ms") or []]
    lat_hours = []
    for t in range(0, int(last_t) + 1, step):
        v = [x for s in samples if t <= s["t"] < t + step for x in (s.get("db") or {}).get("dispatch_latency_ms") or []]
        if v:
            lat_hours.append({"elapsed_h": t / 3600, "n": len(v), "p50_ms": round(statistics.median(v)),
                              "max_ms": max(v)})
    return {
        "condition": cond, "label": samples[0].get("label"), "dir": str(d), "last_t": last_t,
        "last_at": samples[-1].get("at"), "samples": len(samples),
        "api_failures": sum(1 for s in samples if s.get("api_ok") is False),
        "daemon_exit": next((s.get("daemon_exit") for s in samples if "daemon_exit" in s), None),
        "stop": stop.get("reason") if stop else None,
        "restart": {k: restart.get(k) for k in ("at", "start_secs", "reclaimed_secs", "runs_running_after",
                                                "task_leases_after")} if restart else None,
        "segments": segs, "after_restart": after_restart, "table": table,
        "dispatch_latency": {"n": len(lat), "p50_ms": round(statistics.median(lat)) if lat else None,
                             "by_hour": lat_hours},
        "db_mb_max": max(r["db_mb"] for r in rows),
        "ticks": tick_trend(cond_log) if cond_log else None,
    }


def compare(runs, step=3600):
    """同じ経過時間の RssAnon の差（条件 − off）と、最初の区間のウォームアップ後の傾きの差。"""
    off = next((r for r in runs if r["condition"] == "off"), None)
    if not off:
        return []
    out = []
    off_rows = {round(x["elapsed_h"], 3): x for x in off["table"]}
    off_slope = off["segments"][0]["steady_slope_mb_h"] if off["segments"] else None
    for r in runs:
        if r is off:
            continue
        diffs = []
        for x in r["table"]:
            o = off_rows.get(round(x["elapsed_h"], 3))
            if o and x["rss_anon_kb"] is not None and o["rss_anon_kb"] is not None:
                diffs.append({"elapsed_h": x["elapsed_h"], "anon_minus_off_kb": x["rss_anon_kb"] - o["rss_anon_kb"]})
        s = r["segments"][0]["steady_slope_mb_h"] if r["segments"] else None
        out.append({"condition": r["condition"], "anon_minus_off": diffs,
                    "steady_slope_minus_off_mb_h": None if s is None or off_slope is None else round(s - off_slope, 2)})
    return out


def verdict(runs, full_secs=86000, max_db_mb=None):
    """runbook N2 の合否。最終標本が full_secs に届いていない run があれば全体は incomplete（合格にしない）。"""
    off = next((r for r in runs if r["condition"] == "off"), None)
    off_slope = off["segments"][0]["steady_slope_mb_h"] if off and off["segments"] else None
    rows = []
    for r in runs:
        seg = r["segments"]
        slopes = [s["steady_slope_mb_h"] for s in seg if s["steady_slope_mb_h"] is not None]
        checks = {
            "complete": r["last_t"] >= full_secs and r["stop"] == "wall",
            "restarted": r["restart"] is not None and len(seg) >= 2,
            "api_ok": r["api_failures"] == 0,
            "steady_slope_le_1mb_h": bool(slopes) and all(s <= SLOPE_MAX_MB_H for s in slopes),
        }
        if r["condition"] in ("shadow", "enabled"):
            s = seg[0]["steady_slope_mb_h"] if seg else None
            checks["slope_vs_off_le_0_5mb_h"] = (s is not None and off_slope is not None
                                                 and s <= off_slope + SLOPE_VS_OFF_MAX_MB_H)
        ar = r["after_restart"]
        checks["after_restart_within_10pct"] = ar is not None and abs(ar["ratio"] - 1) <= RESTART_TOLERANCE
        if max_db_mb:
            checks["db_below_limit"] = r["db_mb_max"] < max_db_mb
        rows.append({"condition": r["condition"], "checks": checks, "pass": all(checks.values())})
    complete = all(x["checks"]["complete"] for x in rows) and bool(rows)
    overall = "incomplete" if not complete else ("pass" if all(x["pass"] for x in rows) else "fail")
    return {"overall": overall, "rows": rows}


def markdown(doc):
    lines = [f"判定: **{doc['verdict']['overall']}**（runbook N2 の数値。途中の run は incomplete で、合格にしない）", ""]
    for r in doc["runs"]:
        lines.append(f"### {r['label']}/{r['condition']}（最終標本 t={r['last_t']:.0f} 秒・{r['last_at']}、停止 {r['stop']}）")
        lines.append("| 経過 h | RssAnon MB | RSS MB | threads | fds | malloc in_use MB | malloc free MB | page cache MB | "
                     "V8 used/total KB | gen | task done/ready | chat run | DB MB |")
        lines.append("|---:|---:|---:|---:|---:|---:|---:|---:|---|---:|---|---:|---:|")
        mb = lambda v: "-" if v is None else f"{v / 1024:.0f}"
        for x in r["table"]:
            lines.append(
                f"| {x['elapsed_h']:.0f} | {mb(x['rss_anon_kb'])} | {mb(x['rss_kb'])} | {x['threads']} | {x['fds']} | "
                f"{mb(x['malloc_in_use_kb'])} | {mb(x['malloc_free_kb'])} | {mb(x['sqlite_pagecache_kb'])} | "
                f"{x['v8_used_heap_kb']}/{x['v8_total_heap_kb']} | {x['v8_generations']} | {x['done']}/{x['ready']} | "
                f"{x['chat_runs']} | {x['db_mb']} |")
        for i, s in enumerate(r["segments"]):
            ws = ", ".join(f"{w['from'] // 3600}–{w['to'] // 3600}h {w['slope']}" for w in s["window_slopes_mb_h"])
            lines.append(f"- 区間 {i + 1}（{s['secs']:.0f} 秒）: ウォームアップ {s['warmup_secs']} 秒の増分 "
                         f"{s['warmup_delta_kb']} kB、その後の最小二乗の傾き {s['steady_slope_mb_h']} MB/時、"
                         f"全体 {s['whole_slope_mb_h']} MB/時。2 時間の窓ごとの傾き: {ws}")
        if r["restart"]:
            lines.append(f"- 再起動: {json.dumps(r['restart'], ensure_ascii=False)}、後の比較 "
                         f"{json.dumps(r['after_restart'], ensure_ascii=False)}")
        dl = r["dispatch_latency"]
        lines.append(f"- dispatch 遅延（created→最初の routing_decided）: n={dl['n']}、p50 {dl['p50_ms']} ms。時間ごとの p50: "
                     + ", ".join(f"{h['elapsed_h']:.0f}h {h['p50_ms']}" for h in dl["by_hour"]))
        if r["ticks"]:
            lines.append("- slow tick（UTC の時ごと、件数・p50 ms・p50 tick_cos_chat_launch ms）: "
                         + ", ".join(f"{t['hour'][11:13]}時 {t['slow_ticks']}・{t['p50_ms']}・{t['p50_cos_chat_launch_ms']}"
                                     for t in r["ticks"]))
        lines.append("")
    lines.append("### 条件の比較（同じ経過時間、条件 − off）")
    for c in doc["compare"]:
        diffs = ", ".join(f"{d['elapsed_h']:.0f}h {d['anon_minus_off_kb'] // 1024:+d}" for d in c["anon_minus_off"])
        lines.append(f"- {c['condition']}: 傾きの差 {c['steady_slope_minus_off_mb_h']} MB/時。RssAnon の差 MB: {diffs}")
    lines.append("")
    for v in doc["verdict"]["rows"]:
        lines.append(f"- {v['condition']}: {'pass' if v['pass'] else 'not pass'} {json.dumps(v['checks'])}")
    return "\n".join(lines) + "\n"


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("dirs", nargs="+")
    ap.add_argument("--json", required=True)
    ap.add_argument("--markdown", required=True)
    ap.add_argument("--log", action="append", default=[], help="<cond>=<daemon.log>（途中の run の log）")
    ap.add_argument("--warmup-secs", type=int, default=7200)
    ap.add_argument("--step-secs", type=int, default=3600)
    ap.add_argument("--full-secs", type=int, default=86000)
    ap.add_argument("--restart-compare-secs", type=int, default=3600)
    ap.add_argument("--max-db-mb", type=int)
    a = ap.parse_args(argv)
    logs = dict(x.split("=", 1) for x in a.log)
    runs = []
    for d in a.dirs:
        cond = load(d)[0][0]["condition"] if (pathlib.Path(d) / "samples.jsonl").exists() else None
        log = logs.get(cond) or (pathlib.Path(d) / "daemon.log")
        runs.append(summarize(d, log, a.warmup_secs, a.step_secs, a.restart_compare_secs))
    doc = {"schema": SCHEMA, "runs": runs, "compare": compare(runs, a.step_secs),
           "verdict": verdict(runs, a.full_secs, a.max_db_mb)}
    pathlib.Path(a.json).write_text(json.dumps(doc, ensure_ascii=False, indent=1) + "\n")
    pathlib.Path(a.markdown).write_text(markdown(doc))
    print(markdown(doc))
    return 0


if __name__ == "__main__":
    sys.exit(main())
