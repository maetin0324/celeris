"""jev_source_reader（独立読取元の reader）の限定試験。

試験の DB は crates/task-core/migrations/*.sql をそのまま流した一時 SQLite（実 schema の形）。中身は試験用の
合成データで、実 daemon の原記録の証明ではない（変換・抽出アルゴリズムの正例/負例）。
"""
import glob
import hashlib
import importlib.util
import json
import os
import sqlite3
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
DEV = os.path.normpath(os.path.join(HERE, "..", ".."))
REPO = os.path.normpath(os.path.join(DEV, "..", ".."))
SCRIPT = os.path.join(DEV, "jev_source_reader.py")
MIGRATIONS = os.path.join(REPO, "crates", "task-core", "migrations")

_spec = importlib.util.spec_from_file_location("jev_source_reader", SCRIPT)
r = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(r)

TS = "2026-10-19T00:00:00Z"
HASH_C = "a" * 64
ROOT, CHILD, GRAND, OTHER_ROOT, NESTED = (
    "01JEVR00T000000000000000AA", "01JEVCH1LD00000000000000AA", "01JEVGRANDCH1LD000000000AA",
    "01JEV0THER000000000000000A", "01JEVNESTED0000000000000AA")


_TEMPLATE = None


def _template():
    """migrations を流した schema を 1 度だけ memory に作る（file への逐次 commit は遅い）。"""
    global _TEMPLATE
    if _TEMPLATE is None:
        m = sqlite3.connect(":memory:")
        for f in sorted(glob.glob(os.path.join(MIGRATIONS, "*.sql"))):
            with open(f, encoding="utf-8") as fh:
                m.executescript(fh.read())
        m.execute("CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)")
        m.commit()
        _TEMPLATE = m
    return _TEMPLATE


def make_db(path, version=98, wal=False):
    c = sqlite3.connect(path)
    _template().backup(c)
    if wal:
        c.execute("PRAGMA journal_mode=WAL")
    c.execute("INSERT INTO schema_migrations VALUES (?, ?)", (version, TS))
    c.commit()
    return c


def task(c, tid, parent=None):
    c.execute("INSERT INTO tasks (id, status, kind, parent_id, priority, created_at, json, title, updated_at) "
              "VALUES (?, 'done', 'task', ?, 0, ?, ?, ?, ?)", (tid, parent, TS, json.dumps({"id": tid}), tid, TS))


def event(c, tid, seq, body):
    c.execute("INSERT INTO events (task_id, seq, ts, json) VALUES (?, ?, ?, ?)", (tid, seq, TS, json.dumps(body)))


def run(c, rid, tid, seq, campaign, decision=None, parent_run=None, attempt=1, campaign_col=True):
    c.execute("INSERT INTO runs (run_id, task_id, role, seq, status, model, usage_json, started_at) "
              "VALUES (?, ?, 'worker', ?, 'completed', 'claude-sonnet-5-5', ?, ?)",
              (rid, tid, seq, json.dumps({"input_tokens": 10}), TS))
    c.execute("INSERT INTO decision_log_runs (run_id, decision_id, campaign_id, arm, task_id, parent_run_id, attempt_no, "
              "routing_mode, exec_model, manifest_hash, run_end, updated_at) "
              "VALUES (?, ?, ?, 'C', ?, ?, ?, 'decision_assisted', 'claude-sonnet-5-5', ?, 'completed', ?)",
              (rid, decision, campaign, tid, parent_run, attempt, HASH_C, TS))
    c.execute("INSERT INTO decision_log_usage (dedup_key, run_id, decision_id, campaign_id, task_id, role, is_retry, "
              "list_cost_usd, updated_at) VALUES (?, ?, ?, ?, ?, 'worker', ?, 0.5, ?)",
              (f"run:{rid}:src", rid, decision, campaign if campaign_col else None, tid, int(attempt > 1), TS))
    event(c, tid, seq * 10, {"type": "routing_decided", "run_id": rid, "record": {"lane_assist": {"final_lane": "standard"}}})
    if decision:
        c.execute("INSERT INTO route_decisions (decision_id, subject_kind, subject_id, run_id, features_hash, options_hash, "
                  "input_json, output_json, judge_json, stage, created_at) VALUES (?, 'task', ?, ?, 'f', 'o', '{}', ?, '{}', 'live', ?)",
                  (decision, tid, rid, json.dumps({"model_snapshot": "jev-2026-10"}), TS))


def campaign_fixture(c):
    """root→child→grandchild（retry 付き）が campaign cmp-c1。別 campaign と、木の中で別 campaign に属す子を混ぜる。"""
    for tid, parent in [(ROOT, None), (CHILD, ROOT), (GRAND, CHILD), (NESTED, CHILD), (OTHER_ROOT, None)]:
        task(c, tid, parent)
    c.execute("INSERT INTO task_campaigns (task_id, campaign_id, case_id, arm, repetition, manifest_hash, routing_setting, created_at) "
              "VALUES (?, 'cmp-c1', 'k2', 'C', 1, ?, 'decision_assisted', ?)", (ROOT, HASH_C, TS))
    for tid in (OTHER_ROOT, NESTED):
        c.execute("INSERT INTO task_campaigns (task_id, campaign_id, arm, manifest_hash, created_at) VALUES (?, 'cmp-other', 'B', ?, ?)",
                  (tid, "b" * 64, TS))
    c.execute("INSERT INTO task_routing_overrides VALUES (?, ?, ?)", (ROOT, json.dumps({"mode": "decision_assisted"}), TS))
    run(c, "01JEVRUNR00T0000000000000A", ROOT, 1, "cmp-c1", decision="dec-root")
    run(c, "01JEVRUNCH1LD000000000000A", CHILD, 1, "cmp-c1", decision="dec-child")
    run(c, "01JEVRUNGRAND1000000000000", GRAND, 1, "cmp-c1")
    run(c, "01JEVRUNGRAND2000000000000", GRAND, 2, "cmp-c1", parent_run="01JEVRUNGRAND1000000000000", attempt=2, campaign_col=False)
    run(c, "01JEVRUNNESTED000000000000", NESTED, 1, "cmp-other")
    run(c, "01JEVRUN0THER0000000000000", OTHER_ROOT, 1, "cmp-other", decision="dec-other")
    c.commit()


def sha(path):
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.db = os.path.join(self.tmp.name, "measure", "celeris.db")
        os.makedirs(os.path.dirname(self.db))

    def env(self, **extra):
        e = {r.ENV_SOURCE: self.db}
        e.update(extra)
        return e

    def assertCode(self, ctx, exit_code, code):
        self.assertEqual(ctx.exception.exit_code, exit_code, ctx.exception.diagnostics)
        self.assertIn(code, [d["code"] for d in ctx.exception.diagnostics])

    def cli(self, env, *args):
        full = {k: v for k, v in os.environ.items() if k not in (r.ENV_SOURCE, "CELERIS_DB", "CELERIS_RUN_DB")}
        full.update(env)
        p = subprocess.run([sys.executable, SCRIPT] + list(args), capture_output=True, text=True, env=full)
        return p.returncode, p.stdout, p.stderr


class TrustBoundary(Base):
    def test_no_operator_source_is_nonzero_even_with_daemon_db_vars(self):
        make_db(self.db).close()
        with self.assertRaises(r.SourceError) as ctx:
            r.configured_source({"CELERIS_DB": self.db, "CELERIS_RUN_DB": self.db})
        self.assertCode(ctx, 2, "no_trusted_source")
        code, _, err = self.cli({"CELERIS_RUN_DB": self.db}, "--campaign", "cmp-c1")
        self.assertEqual(code, 2, err)
        self.assertIn("no_trusted_source", err)

    def test_source_equal_to_daemon_or_worker_db_is_refused(self):
        make_db(self.db).close()
        for name in ("CELERIS_DB", "CELERIS_RUN_DB"):
            with self.assertRaises(r.SourceError) as ctx:
                r.configured_source(self.env(**{name: self.db}))
            self.assertCode(ctx, 2, "source_is_daemon_db")

    def test_self_declared_source_inside_evidence_is_refused(self):
        ev = os.path.join(self.tmp.name, "ev")
        os.makedirs(ev)
        bundled = os.path.join(ev, "source.db")
        make_db(bundled).close()
        with open(os.path.join(ev, "provenance.json"), "w") as f:
            json.dump({"source": "celerisctl campaign export", "db": bundled}, f)
        with self.assertRaises(r.SourceError) as ctx:
            r.configured_source({r.ENV_SOURCE: bundled}, evidence_dir=ev)
        self.assertCode(ctx, 2, "source_in_evidence_dir")
        # evidence 内の自称 source は接続先にならない（環境変数が無ければ常に非 0）。
        code, _, err = self.cli({}, "--campaign", "cmp-c1", "--evidence-dir", ev)
        self.assertEqual(code, 2, err)

    def test_unreachable_relative_and_non_sqlite_sources_are_nonzero(self):
        with self.assertRaises(r.SourceError) as ctx:
            r.configured_source(self.env())
        self.assertCode(ctx, 2, "source_unreachable")
        with self.assertRaises(r.SourceError) as ctx:
            r.configured_source({r.ENV_SOURCE: "measure/celeris.db"})
        self.assertCode(ctx, 2, "source_not_absolute")
        with open(self.db, "w") as f:
            f.write("not a database\n" * 100)
        with self.assertRaises(r.SourceError) as ctx:
            r.read_campaign("cmp-c1", self.env())
        self.assertCode(ctx, 2, "source_unreachable")

    def test_cli_takes_no_db_path_argument(self):
        code, _, err = self.cli({}, "--campaign", "cmp-c1", "--db", self.db)
        self.assertEqual(code, 2)
        self.assertIn("unrecognized arguments", err)


class Schema(Base):
    def test_unknown_schema_version_is_not_live(self):
        make_db(self.db, version=99).close()
        with self.assertRaises(r.SourceError) as ctx:
            r.read_campaign("cmp-c1", self.env())
        self.assertCode(ctx, 3, "unknown_schema")

    def test_missing_table_and_column_are_diagnosed(self):
        c = sqlite3.connect(self.db)
        c.executescript("CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);"
                        "INSERT INTO schema_migrations VALUES (98, 'x');"
                        "CREATE TABLE tasks (id TEXT PRIMARY KEY, parent_id TEXT, status TEXT, kind TEXT, created_at TEXT);")
        c.close()
        code, _, err = self.cli(self.env(), "--campaign", "cmp-c1")
        self.assertEqual(code, 3, err)
        diags = json.loads(err)["diagnostics"]
        self.assertIn({"code": "missing_column", "detail": "tasks.json"}, diags)
        self.assertIn({"code": "missing_table", "detail": "decision_log_usage"}, diags)

    def test_unknown_campaign_is_not_live(self):
        c = make_db(self.db)
        campaign_fixture(c)
        c.close()
        with self.assertRaises(r.SourceError) as ctx:
            r.read_campaign("cmp-missing", self.env())
        self.assertCode(ctx, 3, "campaign_not_found")


class Extraction(Base):
    def setUp(self):
        super().setUp()
        c = make_db(self.db)
        campaign_fixture(c)
        c.close()

    def test_ids_follow_the_campaign_tree_and_exclude_other_campaigns(self):
        d = r.read_campaign("cmp-c1", self.env())
        self.assertEqual([t["id"] for t in d["tasks"]], sorted([ROOT, CHILD, GRAND]))
        self.assertEqual(d["campaign"]["manifest_hashes"], [HASH_C])
        runs = sorted(x["run_id"] for x in d["runs"])
        self.assertEqual(runs, sorted(["01JEVRUNR00T0000000000000A", "01JEVRUNCH1LD000000000000A",
                                       "01JEVRUNGRAND1000000000000", "01JEVRUNGRAND2000000000000"]))
        self.assertEqual(sorted(x["run_id"] for x in d["decision_runs"]), runs)
        self.assertEqual(sorted(e["json"]["run_id"] for e in d["routing_decided"]), runs)
        # retry の usage 行（campaign 欄 NULL）は run_id で辿って取り込む。別 campaign の行は入らない。
        self.assertEqual(sorted(u["run_id"] for u in d["usage"]), runs)
        self.assertEqual(sorted(x["decision_id"] for x in d["responses"]), ["dec-child", "dec-root"])
        self.assertEqual(d["responses"][0]["output_json"], {"model_snapshot": "jev-2026-10"})
        self.assertEqual([o["task_id"] for o in d["routing_overrides"]], [ROOT])
        self.assertIn("manifest_body_not_in_source", [x["code"] for x in d["diagnostics"]])
        self.assertEqual(d["source"]["path"], os.path.realpath(self.db))

    def test_reading_does_not_change_the_source(self):
        before = sha(self.db)
        siblings = sorted(os.listdir(os.path.dirname(self.db)))
        code, out, err = self.cli(self.env(), "--campaign", "cmp-c1")
        self.assertEqual(code, 0, err)
        self.assertEqual(json.loads(out)["campaign"]["campaign_id"], "cmp-c1")
        self.assertEqual(sha(self.db), before)
        self.assertEqual(sorted(os.listdir(os.path.dirname(self.db))), siblings)

    def test_connection_is_read_only(self):
        with r.SourceSnapshot.open(r.configured_source(self.env())) as s:
            self.assertEqual(s.conn.execute("PRAGMA query_only").fetchone()[0], 1)
            for sql in ("INSERT INTO task_campaigns (task_id, campaign_id, created_at) VALUES ('x', 'y', 'z')",
                        "CREATE TABLE evil (x)", "DELETE FROM events"):
                with self.assertRaises(sqlite3.Error):
                    s.conn.execute(sql)
            s.conn.execute("PRAGMA query_only=OFF")
            with self.assertRaises(sqlite3.OperationalError):  # mode=ro は query_only を外しても書けない
                s.conn.execute("DELETE FROM events")


class SnapshotConsistency(Base):
    def test_all_reads_see_one_snapshot_while_a_writer_commits(self):
        w = make_db(self.db, wal=True)
        campaign_fixture(w)
        with r.SourceSnapshot.open(r.configured_source(self.env())) as s:
            first = s.extract("cmp-c1")
            # 読取中に別の接続が新しい子 task・run・event を commit しても、同じ snapshot の読みは変わらない。
            task(w, "01JEVLATE00000000000000AAA", ROOT)
            run(w, "01JEVRUNLATE00000000000000", "01JEVLATE00000000000000AAA", 1, "cmp-c1")
            w.commit()
            second = s.extract("cmp-c1")
            self.assertEqual(first, second)
        after = r.read_campaign("cmp-c1", self.env())
        self.assertIn("01JEVLATE00000000000000AAA", [t["id"] for t in after["tasks"]])
        w.close()


if __name__ == "__main__":
    unittest.main()
