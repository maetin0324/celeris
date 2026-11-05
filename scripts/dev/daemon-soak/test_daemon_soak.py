"""P4-F: daemon_soak.py の門・自動停止・読み取り・集計の試験（daemon を起こさない）。

python3 -I -m unittest discover -s scripts/dev/daemon-soak -p 'test_*.py'
"""
import datetime
import json
import pathlib
import sqlite3
import sys
import tempfile
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import daemon_soak as ds  # noqa: E402

NOW = datetime.datetime(2026, 11, 5, 12, tzinfo=datetime.timezone.utc)
APPROVAL = {"approved_by": "human:rmaeda", "scope": "non-production", "max_secs": 86400,
            "conditions": ["off", "shadow"], "expires_at": "2026-11-06T00:00:00Z"}


class Gate(unittest.TestCase):
    def test_short_runs_need_no_approval(self):
        self.assertEqual(ds.gate(3600, None, "off", {}, NOW), (True, "short"))
        self.assertEqual(ds.gate(1560, None, "off", {"CELERIS_RUN_ID": "r"}, NOW), (True, "short"))

    def test_long_runs_need_a_human_approval(self):
        ok, why = ds.gate(86400, None, "off", {}, NOW)
        self.assertFalse(ok)
        self.assertIn("approval", why)
        self.assertTrue(ds.gate(86400, APPROVAL, "shadow", {}, NOW)[0])

    def test_long_runs_are_refused_inside_a_worker_run_even_with_approval(self):
        ok, why = ds.gate(86400, APPROVAL, "off", {"CELERIS_RUN_ID": "r"}, NOW)
        self.assertFalse(ok)
        self.assertIn("worker", why)

    def test_approval_fields_are_checked(self):
        for patch, word in (({"approved_by": "operator:x"}, "human"), ({"scope": "production"}, "scope"),
                            ({"max_secs": 3601}, "max_secs"), ({"conditions": ["off"]}, "condition"),
                            ({"expires_at": "2026-11-05T11:00:00Z"}, "expired"), ({"expires_at": "x"}, "ISO")):
            ok, why = ds.gate(86400, {**APPROVAL, **patch}, "shadow", {}, NOW)
            self.assertFalse(ok, patch)
            self.assertIn(word, why)

    def test_production_paths_are_refused(self):
        self.assertTrue(ds.production_path("/local/celeris/state/x"))
        self.assertTrue(ds.production_path(str(pathlib.Path.home() / ".config/celeris")))
        self.assertFalse(ds.production_path("/local/celeris/data/scratch/soak"))


class AutoStop(unittest.TestCase):
    LIMITS = {"max_rss_mb": 100, "min_free_mb": 10, "max_db_mb": 1, "max_api_failures": 3}

    def reason(self, sample, elapsed=10, failures=0):
        return ds.stop_reason(sample, self.LIMITS, elapsed, 60, failures)

    def test_each_limit_stops(self):
        self.assertIsNone(self.reason({"proc": {"rss_kb": 1000}, "disk_free_mb": 100, "db_bytes": 10}))
        self.assertEqual(self.reason({"daemon_exit": 101}), "daemon_exited:101")
        self.assertTrue(self.reason({"proc": {"rss_kb": 200 * 1024}}).startswith("rss_limit"))
        self.assertTrue(self.reason({"disk_free_mb": 5}).startswith("disk_free"))
        self.assertTrue(self.reason({"db_bytes": 2 << 20}).startswith("db_size"))
        self.assertTrue(self.reason({}, failures=3).startswith("api_down"))
        self.assertEqual(self.reason({}, elapsed=60), "wall")


class Readers(unittest.TestCase):
    def test_proc_status(self):
        s = ds.parse_proc_status("VmRSS:\t 588668 kB\nRssAnon:\t 520700 kB\nThreads:\t38\nName:\tceleris\n")
        self.assertEqual(s, {"rss_kb": 588668, "rss_anon_kb": 520700, "threads": 38})

    def test_arena_estimate_counts_64mib_heaps(self):
        maps = "\n".join([
            "7f0000000000-7f0000a00000 rw-p 00000000 00:00 0",       # arena の heap（10 MiB 使用）
            "7f0000a00000-7f0004000000 ---p 00000000 00:00 0",       # 残りの予約（合わせて 64 MiB）
            "7f0008000000-7f0008800000 rw-p 00000000 00:00 0 [stack]",  # 名前つき
            "7f000c000000-7f000c800000 rw-p 00000000 00:00 0",       # 64 MiB にならない
            "7f0010000000-7f0014000000 rw-p 00000000 00:00 0",       # 全部使った arena
        ])
        self.assertEqual(ds.arena_estimate(maps), 2)

    def test_process_memory_line_with_ansi(self):
        line = ("\x1b[2m2026-11-05T21:13:43Z\x1b[0m \x1b[32m INFO\x1b[0m celeris::daemon::process_memory: process_memory "
                "\x1b[3mrss_kb\x1b[0m\x1b[2m=\x1b[0m66432 rss_anon_kb=8832 sqlite_pagecache_bytes=1945200 v8_used_heap=0")
        self.assertEqual(ds.parse_process_memory(line),
                         {"rss_kb": 66432, "rss_anon_kb": 8832, "sqlite_pagecache_bytes": 1945200, "v8_used_heap": 0})
        self.assertIsNone(ds.parse_process_memory("INFO celeris: tick ticks=3"))

    def test_last_process_memory_reads_from_offset(self):
        with tempfile.TemporaryDirectory() as d:
            p = pathlib.Path(d) / "daemon.log"
            p.write_text("process_memory rss_kb=1\nother\nprocess_memory rss_kb=2\n")
            pm, pos = ds.last_process_memory(p)
            self.assertEqual(pm["rss_kb"], 2)
            self.assertEqual(ds.last_process_memory(p, pos), (None, pos))

    def test_ts_epoch_reads_nanoseconds(self):
        self.assertAlmostEqual(ds.ts_epoch("2026-11-05T21:20:02.041355653Z") - ds.ts_epoch("2026-11-05T21:20:02Z"),
                               0.041355653, places=6)

    def test_count_rows_reads_state_and_lag(self):
        conn = sqlite3.connect(":memory:")
        conn.executescript("""
            CREATE TABLE tasks (id TEXT, status TEXT, lease_worker_run_id TEXT);
            CREATE TABLE runs (run_id TEXT, status TEXT);
            CREATE TABLE events (task_id TEXT, seq INTEGER, ts TEXT, json TEXT);
            CREATE TABLE project_wakes (id INTEGER, consumed_by TEXT);
            CREATE TABLE feed_cursor (name TEXT, value TEXT);
            INSERT INTO tasks VALUES ('a','running','r1'),('b','done',NULL);
            INSERT INTO runs VALUES ('r1','running'),('r0','completed');
            INSERT INTO events VALUES ('a',0,'t','{}'),('a',1,'t','{}'),('b',0,'t','{}');
            INSERT INTO project_wakes VALUES (1,NULL),(2,'d1');
            INSERT INTO feed_cursor VALUES ('events','1'),('cos_route_version','cos_v1');
        """)
        c = ds.count_rows(conn)
        self.assertEqual((c["tasks"], c["task_leases"], c["runs_running"], c["wakes_unconsumed"]), (2, 1, 1, 1))
        self.assertEqual(c["tasks_by_status"], {"running": 1, "done": 1})
        self.assertEqual(c["event_lag"], {"events": 2})


class Summary(unittest.TestCase):
    def rows(self, cond, rss):
        out = []
        for i, kb in enumerate(rss):
            out.append({"schema": ds.SCHEMA, "label": "t", "condition": cond, "phase": "cycle", "t": i * 300.0,
                        "proc": {"rss_kb": kb, "rss_anon_kb": kb // 2, "rss_file_kb": kb // 2, "threads": 30},
                        "arena_estimate": 4, "fds": 40,
                        "process_memory": {"malloc_arena": 1 << 20, "malloc_mmap": 0, "malloc_in_use": 1, "malloc_free": 1,
                                           "v8_used_heap": 1000 if cond != "off" else 0, "v8_total_heap": 2000,
                                           "v8_generations": 1 if cond != "off" else 0, "sqlite_memory_used": 0,
                                           "sqlite_pagecache_bytes": 1},
                        "db": {"dispatch_latency_ms": [100, 200]}})
        return out

    def test_slopes_use_the_same_two_point_formula_as_p1i(self):
        self.assertEqual(ds.slope([0, 181820], [1529, 588668]), 935.5)  # P1-I の本番の全区間の傾きと同じ値
        r = ds.summarize_run(self.rows("off", [100000, 100000, 101024, 101024, 102048, 102048, 103072]),
                             [{"step": "stop", "reason": "wall"}])
        self.assertEqual(r["wall_secs"], 1800.0)
        self.assertEqual(r["rss_slope_whole_mb_per_hour"], ds.slope([0, 100000], [1800, 103072]))
        self.assertEqual(r["rss_slope_prod_steady_mb_per_hour"], ds.slope([600, 101024], [1500, 102048]))
        self.assertEqual(r["dispatch_latency_ms"]["p50"], 100)

    def test_thresholds_compare_with_off(self):
        off = ds.summarize_run(self.rows("off", [100000] * 7), [{"step": "stop", "reason": "wall"}])
        bad = ds.summarize_run(self.rows("shadow", [100000, 100000, 100000, 110000, 120000, 130000, 140000]),
                               [{"step": "stop", "reason": "rss_limit:x"}])
        t = {x["condition"]: x for x in ds.thresholds([off, bad])}
        self.assertTrue(t["off"]["pass"])
        self.assertFalse(t["shadow"]["checks"]["steady_slope_vs_off_le_10mb_h"])
        self.assertFalse(t["shadow"]["checks"]["not_auto_stopped"])

    def test_summary_rejects_other_schema(self):
        with tempfile.TemporaryDirectory() as d:
            (pathlib.Path(d) / "samples.jsonl").write_text(json.dumps({"schema": "x"}) + "\n")
            (pathlib.Path(d) / "ops.jsonl").write_text("")
            with self.assertRaises(ValueError):
                ds.load_dir(d)


if __name__ == "__main__":
    unittest.main()
