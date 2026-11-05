#!/usr/bin/env python3
"""driver.collect_usage の単体試験（ADR 2026-11-05-phase3-p0f-usage-counting）。一時 SQLite だけを使う。

    python3 -I scripts/dev/long-horizon-pilot/test_driver_usage.py -v
"""
import json
import pathlib
import sqlite3
import sys
import unittest

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import driver  # noqa: E402

PID = "P1"


def db():
    c = sqlite3.connect(":memory:")
    c.row_factory = sqlite3.Row
    c.executescript(
        "CREATE TABLE tasks (id TEXT, project_id TEXT);"
        "CREATE TABLE runs (task_id TEXT, role TEXT, started_at TEXT, usage_json TEXT);"
        "CREATE TABLE chat_threads (id TEXT, project_id TEXT);"
        "CREATE TABLE chat_runs (thread_id TEXT, started_at TEXT, usage_json TEXT);"
        "INSERT INTO tasks VALUES ('T1','P1'),('T2','OTHER');"
        "INSERT INTO chat_threads VALUES ('H1','P1'),('H2','OTHER');")
    return c


def u(i, o, cr=0, cc=0):
    return json.dumps({"input_tokens": i, "output_tokens": o, "cache_read_tokens": cr, "cache_creation_tokens": cc})


def secs(iso):
    return 0 if not iso else int(iso[-3:-1])


class CollectUsage(unittest.TestCase):
    def test_cos_and_runs_are_summed_with_cache_separate(self):
        c = db()
        c.execute("INSERT INTO runs VALUES ('T1','worker','2026-01-01T00:00:10Z',?)", (u(100, 10, 5, 7),))
        c.execute("INSERT INTO runs VALUES ('T2','worker','2026-01-01T00:00:11Z',?)", (u(9999, 9999),))
        c.execute("INSERT INTO chat_runs VALUES ('H1','2026-01-01T00:00:05Z',?)", (u(40, 4, 1, 2),))
        c.execute("INSERT INTO chat_runs VALUES ('H2','2026-01-01T00:00:06Z',?)", (u(9999, 9999),))
        inv, tokens, cache, missing = driver.collect_usage(c, PID, "claude-code", secs)
        self.assertEqual((tokens, cache, missing), (154, 15, None))
        self.assertEqual([(i["role"], i["at"]) for i in inv], [("cos", 5), ("worker", 10)])
        self.assertEqual(inv[0]["usage"]["input_tokens"], 40)

    def test_old_cache_key_is_not_read(self):
        c = db()
        c.execute("INSERT INTO runs VALUES ('T1','worker','2026-01-01T00:00:10Z',?)",
                  (json.dumps({"input_tokens": 1, "output_tokens": 1, "cache_creation_input_tokens": 50}),))
        _, tokens, cache, _ = driver.collect_usage(c, PID, "claude-code", secs)
        self.assertEqual((tokens, cache), (2, 0))

    def test_null_cos_usage_sets_missing_and_tokens_null(self):
        c = db()
        c.execute("INSERT INTO runs VALUES ('T1','worker','2026-01-01T00:00:10Z',?)", (u(1, 1),))
        c.execute("INSERT INTO chat_runs VALUES ('H1','2026-01-01T00:00:05Z',NULL)")
        c.execute("INSERT INTO chat_runs VALUES ('H1','2026-01-01T00:00:06Z',NULL)")
        _, tokens, cache, missing = driver.collect_usage(c, PID, "claude-code", secs)
        self.assertEqual((tokens, cache, missing), (None, None, "cos_usage_null:2"))

    def test_null_run_usage_and_both_reasons(self):
        c = db()
        c.execute("INSERT INTO runs VALUES ('T1','worker','2026-01-01T00:00:10Z',NULL)")
        c.execute("INSERT INTO chat_runs VALUES ('H1','2026-01-01T00:00:05Z',NULL)")
        _, tokens, _, missing = driver.collect_usage(c, PID, "claude-code", secs)
        self.assertIsNone(tokens)
        self.assertEqual(missing, "run_usage_null:1,cos_usage_null:1")

    def test_stub_keeps_tier_stub(self):
        c = db()
        c.execute("INSERT INTO chat_runs VALUES ('H1','2026-01-01T00:00:05Z',NULL)")
        _, tokens, _, missing = driver.collect_usage(c, PID, "stub", secs)
        self.assertEqual((tokens, missing), (None, "tier=stub"))


if __name__ == "__main__":
    unittest.main()
