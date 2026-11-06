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


def u(i, o, cr=0, cc=0, usd=0.1):
    return json.dumps({"input_tokens": i, "output_tokens": o, "cache_read_tokens": cr, "cache_creation_tokens": cc,
                       "cost_usd": usd})


def secs(iso):
    return 0 if not iso else int(iso[-3:-1])


class CollectUsage(unittest.TestCase):
    def test_cos_and_runs_are_summed_with_cache_separate(self):
        c = db()
        c.execute("INSERT INTO runs VALUES ('T1','worker','2026-01-01T00:00:10Z',?)", (u(100, 10, 5, 7),))
        c.execute("INSERT INTO runs VALUES ('T2','worker','2026-01-01T00:00:11Z',?)", (u(9999, 9999),))
        c.execute("INSERT INTO chat_runs VALUES ('H1','2026-01-01T00:00:05Z',?)", (u(40, 4, 1, 2),))
        c.execute("INSERT INTO chat_runs VALUES ('H2','2026-01-01T00:00:06Z',?)", (u(9999, 9999),))
        inv, tokens, cache, usd, missing = driver.collect_usage(c, PID, "claude-code", secs)
        self.assertEqual((tokens, cache, missing), (154, 15, None))
        self.assertAlmostEqual(usd, 0.2)  # 他の案件の行（T2・H2）は足さない
        self.assertEqual([(i["role"], i["at"]) for i in inv], [("cos", 5), ("worker", 10)])
        self.assertEqual(inv[0]["usage"]["input_tokens"], 40)

    def test_old_cache_key_is_not_read_and_counts_as_missing(self):
        c = db()
        c.execute("INSERT INTO runs VALUES ('T1','worker','2026-01-01T00:00:10Z',?)",
                  (json.dumps({"input_tokens": 1, "output_tokens": 1, "cache_creation_input_tokens": 50,
                               "cost_usd": 0.1}),))
        _, tokens, cache, usd, missing = driver.collect_usage(c, PID, "claude-code", secs)
        self.assertEqual((tokens, cache, usd, missing), (None, None, None, "cache_missing:1"))

    def test_null_cos_usage_sets_missing_and_tokens_null(self):
        c = db()
        c.execute("INSERT INTO runs VALUES ('T1','worker','2026-01-01T00:00:10Z',?)", (u(1, 1),))
        c.execute("INSERT INTO chat_runs VALUES ('H1','2026-01-01T00:00:05Z',NULL)")
        c.execute("INSERT INTO chat_runs VALUES ('H1','2026-01-01T00:00:06Z',NULL)")
        _, tokens, cache, usd, missing = driver.collect_usage(c, PID, "claude-code", secs)
        self.assertEqual((tokens, cache, usd, missing), (None, None, None, "cos_usage_null:2"))

    def test_null_run_usage_and_both_reasons(self):
        c = db()
        c.execute("INSERT INTO runs VALUES ('T1','worker','2026-01-01T00:00:10Z',NULL)")
        c.execute("INSERT INTO chat_runs VALUES ('H1','2026-01-01T00:00:05Z',NULL)")
        _, tokens, _, _, missing = driver.collect_usage(c, PID, "claude-code", secs)
        self.assertIsNone(tokens)
        self.assertEqual(missing, "run_usage_null:1,cos_usage_null:1")

    def test_stub_keeps_tier_stub(self):
        c = db()
        c.execute("INSERT INTO chat_runs VALUES ('H1','2026-01-01T00:00:05Z',NULL)")
        _, tokens, _, _, missing = driver.collect_usage(c, PID, "stub", secs)
        self.assertEqual((tokens, missing), (None, "tier=stub"))

    def test_usd_missing_is_not_added_as_zero(self):
        c = db()
        c.execute("INSERT INTO runs VALUES ('T1','worker','2026-01-01T00:00:10Z',?)", (u(1, 1),))
        c.execute("INSERT INTO runs VALUES ('T1','reviewer','2026-01-01T00:00:11Z',?)",
                  (json.dumps({"input_tokens": 1, "output_tokens": 1, "cache_read_tokens": 0,
                               "cache_creation_tokens": 0, "cost_usd": None}),))
        _, tokens, cache, usd, missing = driver.collect_usage(c, PID, "claude-code", secs)
        self.assertEqual((tokens, cache, usd, missing), (None, None, None, "usd_missing:1"))


class AppendOnly(unittest.TestCase):
    def test_gone_and_changed_rows_are_counted(self):
        c = sqlite3.connect(":memory:")
        c.execute("CREATE TABLE events (id INTEGER PRIMARY KEY, json TEXT)")
        c.executemany("INSERT INTO events VALUES (?, ?)", [(1, "{}"), (2, "{}"), (3, "{}")])
        seen = driver.events_digest(c)
        self.assertEqual(driver.append_only_violations(seen, driver.events_digest(c)), (0, 0))
        c.execute("INSERT INTO events VALUES (4, '{}')")
        self.assertEqual(set(driver.events_digest(c, 3)), {4})
        c.execute("UPDATE events SET json = '{\"x\":1}' WHERE id = 2")
        c.execute("DELETE FROM events WHERE id = 3")
        self.assertEqual(driver.append_only_violations(seen, driver.events_digest(c)), (1, 1))


class CosRejections(unittest.TestCase):
    def test_launch_rejections_are_counted_per_project(self):
        c = db()
        c.execute("CREATE TABLE cos_operations (thread_id TEXT, state TEXT, reason TEXT)")
        c.executemany("INSERT INTO cos_operations VALUES (?, ?, ?)", [
            ("H1", "rejected", "rejected: the project controller is enabled for this project and launches its "
                               "next root; requested reason: x"),
            ("H1", "rejected", "rejected: other"),
            ("H1", "applied", "ok"),
            ("H2", "rejected", "launches its next root")])
        self.assertEqual(driver.cos_rejections(c, PID), (2, 1))


class CosSkills(unittest.TestCase):
    def test_cos_skills_are_installed_from_the_repo(self):
        import tempfile
        with tempfile.TemporaryDirectory() as d:
            kb = pathlib.Path(d)
            driver.install_cos_skills(kb)
            for name in driver.COS_SKILLS:
                self.assertTrue((kb / "skills" / name / "SKILL.md").is_file(), name)

    def test_failed_cos_runs_are_counted_by_reason(self):
        c = db()
        c.execute("ALTER TABLE chat_runs ADD COLUMN state TEXT")
        c.execute("ALTER TABLE chat_runs ADD COLUMN reason TEXT")
        c.execute("INSERT INTO chat_runs VALUES ('H1','2026-01-01T00:00:05Z',NULL,'failed',"
                  "'CoS skills unavailable: cos-operator')")
        c.execute("INSERT INTO chat_runs VALUES ('H2','2026-01-01T00:00:05Z',NULL,'failed','x')")
        self.assertEqual(driver.cos_run_failures(c, PID), {"CoS skills unavailable: cos-operator": 1})


if __name__ == "__main__":
    unittest.main()
