"""N2: soak_trend.py の傾き・区間・再起動の比較・合否の試験（daemon を起こさない）。

python3 -I scripts/dev/daemon-soak/test_soak_trend.py
"""
import json
import pathlib
import sys
import tempfile
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import soak_trend as st  # noqa: E402

SCHEMA = "celeris.daemon-soak/1"


def sample(cond, t, pid, anon_kb, **extra):
    return {"schema": SCHEMA, "label": "main", "condition": cond, "phase": "cycle", "t": t, "at": "x", "pid": pid,
            "proc": {"rss_kb": anon_kb + 40000, "rss_anon_kb": anon_kb, "threads": 30}, "fds": 40,
            "process_memory": {"malloc_in_use": 1 << 20, "malloc_free": 1 << 20, "v8_generations": 1},
            "api_ok": True, "db": {"tasks": 1, "tasks_by_status": {"done": 1}, "dispatch_latency_ms": [100]},
            "db_bytes": 1 << 20, **extra}


def write_run(root, cond, slope_kb_per_h, secs=86400, restart_at=43200, after_kb=None):
    d = pathlib.Path(root) / f"main-{cond}"
    d.mkdir()
    rows = []
    for t in range(0, secs + 1, 600):
        if restart_at and t >= restart_at:
            base = after_kb if after_kb is not None else 10000
            rows.append(sample(cond, t, 2, base + int(slope_kb_per_h * (t - restart_at) / 3600)))
        else:
            rows.append(sample(cond, t, 1, 10000 + int(slope_kb_per_h * t / 3600)))
    (d / "samples.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
    ops = [{"step": "restart", "at": restart_at, "runs_running_after": 0, "task_leases_after": 0}] if restart_at else []
    ops.append({"step": "stop", "reason": "wall" if secs >= 86000 else None})
    (d / "ops.jsonl").write_text("".join(json.dumps(o) + "\n" for o in ops))
    return d


class Slope(unittest.TestCase):
    def test_least_squares_slope_is_mb_per_hour(self):
        self.assertEqual(st.lsq_slope([(0, 0), (3600, 1024), (7200, 2048)]), 1.0)
        self.assertIsNone(st.lsq_slope([(0, 1)]))
        self.assertIsNone(st.lsq_slope([(5, 1), (5, 2)]))


class Summary(unittest.TestCase):
    def test_flat_runs_with_restart_pass(self):
        with tempfile.TemporaryDirectory() as tmp:
            dirs = [write_run(tmp, c, 0) for c in ("off", "shadow")]
            runs = [st.summarize(d) for d in dirs]
            v = st.verdict(runs, max_db_mb=100)
            self.assertEqual(v["overall"], "pass", v)
            self.assertEqual(len(runs[0]["segments"]), 2)
            self.assertEqual(runs[0]["after_restart"]["ratio"], 1.0)

    def test_growth_after_warmup_fails_and_compare_sees_the_difference(self):
        with tempfile.TemporaryDirectory() as tmp:
            runs = [st.summarize(write_run(tmp, "off", 0)), st.summarize(write_run(tmp, "shadow", 15 * 1024))]
            v = st.verdict(runs)
            self.assertEqual(v["overall"], "fail")
            shadow = next(r for r in v["rows"] if r["condition"] == "shadow")
            self.assertFalse(shadow["checks"]["steady_slope_le_1mb_h"])
            self.assertFalse(shadow["checks"]["slope_vs_off_le_0_5mb_h"])
            c = st.compare(runs)[0]
            self.assertAlmostEqual(c["steady_slope_minus_off_mb_h"], 15.0, delta=0.1)

    def test_unfinished_run_is_incomplete_not_pass(self):
        with tempfile.TemporaryDirectory() as tmp:
            runs = [st.summarize(write_run(tmp, "off", 0, secs=32400, restart_at=0))]
            v = st.verdict(runs)
            self.assertEqual(v["overall"], "incomplete")
            self.assertFalse(v["rows"][0]["checks"]["restarted"])

    def test_restart_compares_the_same_elapsed_time_since_process_start(self):
        with tempfile.TemporaryDirectory() as tmp:
            r = st.summarize(write_run(tmp, "off", 0, after_kb=12000))
            self.assertEqual(r["after_restart"]["before_kb"], 10000)
            self.assertEqual(r["after_restart"]["after_kb"], 12000)
            v = st.verdict([r])
            self.assertFalse(v["rows"][0]["checks"]["after_restart_within_10pct"])

    def test_slow_tick_lines_are_grouped_by_hour(self):
        with tempfile.TemporaryDirectory() as tmp:
            log = pathlib.Path(tmp) / "daemon.log"
            line = ('{at}  WARN celeris::daemon::tick_loop: slow tick ticks=1 duration_ms={ms} report=TickReport '
                    '{{ step_us: [("tick_cos_chat_launch", {us})] }}\n')
            log.write_text(line.format(at="2026-11-06T07:00:01.0Z", ms=1000, us=900000)
                           + "\x1b[33m" + line.format(at="2026-11-06T07:30:01.0Z", ms=3000, us=2500000)
                           + line.format(at="2026-11-06T08:00:01.0Z", ms=5000, us=4000000)
                           + "2026-11-06T08:00:02Z  WARN task_dispatch::dispatcher::tick_steps: slow tick phases\n")
            rows = st.tick_trend(log)
            self.assertEqual([r["slow_ticks"] for r in rows], [2, 1])
            self.assertEqual(rows[0]["p50_ms"], 2000)
            self.assertEqual(rows[1]["p50_cos_chat_launch_ms"], 4000)
            self.assertIsNone(st.tick_trend(pathlib.Path(tmp) / "missing.log"))

    def test_cli_writes_json_and_markdown(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = write_run(tmp, "off", 0)
            j, m = pathlib.Path(tmp) / "t.json", pathlib.Path(tmp) / "t.md"
            self.assertEqual(st.main([str(d), "--json", str(j), "--markdown", str(m)]), 0)
            self.assertEqual(json.loads(j.read_text())["schema"], st.SCHEMA)
            self.assertIn("判定", m.read_text())


if __name__ == "__main__":
    unittest.main()
