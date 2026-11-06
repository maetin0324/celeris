#!/usr/bin/env python3
"""pilot_gate.py の単体試験（ADR 2026-11-06-phase5-n1-pilot-measurement-contract）。一時 dir と一時 SQLite だけを使う。

    python3 -I scripts/dev/long-horizon-pilot/test_pilot_gate.py -v

欠測の 3 種（usage_json null・cache 欄の欠落・USD 未算出）について、修正前の道具では合格になり得たこと
（PreFix）と、今の driver・gate では「不明」になること（Missing）を並べる。
"""
import copy
import datetime
import json
import pathlib
import sqlite3
import sys
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parents[2]
sys.path.insert(0, str(HERE))
import driver  # noqa: E402
import pilot_gate as g  # noqa: E402

# P0-F の前の driver で取った実 LLM の raw（CoS の usage が null のまま missing=None と書かれ、2/2 が pass で採点された）。
PRE_P0F_RAW = ROOT / "agent-docs" / "progress" / "2026-11-05-phase3" / "llm-v2"
NOW = datetime.datetime(2026, 11, 6, 12, 0, tzinfo=datetime.timezone.utc)
TMP = None


def setUpModule():
    global TMP
    TMP = tempfile.TemporaryDirectory()


def tearDownModule():
    TMP.cleanup()


def prefix_collect_usage(conn, pid, harness, secs):
    """N1 の前（main の 01M6P8HSXJ の後）の driver.collect_usage の写し。null だけを欠測とし、cache 欄・USD を見ない。"""
    real = harness != "stub"
    tokens = cache_tokens = 0
    nulls = 0
    for (raw,) in list(conn.execute("SELECT r.usage_json FROM runs r JOIN tasks t ON t.id = r.task_id "
                                    "WHERE t.project_id = ?", (pid,))) + list(conn.execute(
            "SELECT c.usage_json FROM chat_runs c JOIN chat_threads h ON h.id = c.thread_id WHERE h.project_id = ?",
            (pid,))):
        usage = json.loads(raw) if raw else None
        if usage:
            tokens += sum(int(usage.get(k) or 0) for k in ("input_tokens", "output_tokens"))
            cache_tokens += sum(int(usage.get(k) or 0) for k in ("cache_read_tokens", "cache_creation_tokens"))
        elif real:
            nulls += 1
    return (None, None, f"cos_usage_null:{nulls}") if nulls else (tokens, cache_tokens, None)


def usage(i=10, o=5, cr=100, cc=50, usd=0.25):
    return {"input_tokens": i, "output_tokens": o, "cache_read_tokens": cr, "cache_creation_tokens": cc,
            "cost_usd": usd}


def complete_facts(case="C08"):
    inv = [{"at": 1, "role": "cos", "usage": usage()}, {"at": 2, "role": "worker", "usage": usage()},
           {"at": 3, "role": "reviewer", "usage": usage()}]
    return {"case": case, "invocations": inv, "leak_hits": [], "cos_launch_rejected": 0, "cos_ops_rejected": 0,
            "events_append_only": True, "events_checked": {"seen": 40, "gone": 0, "changed": 0},
            "usage": {"tokens": 45, "cache_tokens": 450, "usd": 0.75, "quota": None, "wall_secs": 120,
                      "missing": None}}


def grade_row(trial, case="C08", verdict="pass", achieved=True, double=0):
    return {"trial": trial, "case": case, "grade": {"verdict": verdict},
            "agreement": {"controller_achieved": achieved, "mismatch": (verdict == "pass") != achieved},
            "metrics": {"double_effect": double}}


def approval(**kw):
    a = {"approval_id": "n1-g7-001", "approved_by": "human:rmaeda", "approved_at": "2026-11-06T10:00:00Z",
         "expires_at": "2026-11-07T00:00:00Z", "account": "claude_max_lab", "model": "claude-sonnet-5-5",
         "harness": "claude_code",
         "scope": {"cases": ["C06", "C08"], "arms": ["new"], "environment": "non-production"},
         "limits": {"max_usd": 5, "max_wall_secs": 7200}}
    a.update(kw)
    return a


class Raw:
    """facts/・ops/・grade/grades.jsonl を一時 dir に書く。"""

    def __init__(self):
        self.dir = pathlib.Path(tempfile.mkdtemp(dir=TMP.name))
        for sub in ("facts", "ops", "grade"):
            (self.dir / sub).mkdir()
        self.grades = []

    def add(self, trial, facts, label="config", grade=True, approvals=()):
        (self.dir / "facts" / f"{trial}.json").write_text(json.dumps(facts))
        (self.dir / "ops" / f"{trial}.json").write_text(json.dumps(
            {"harness": "config", "harness_label": label, "arm": "new", "case": facts["case"],
             "approvals": list(approvals)}))
        if grade:
            self.grades.append(grade_row(trial, facts["case"]) if grade is True else grade)
        (self.dir / "grade" / "grades.jsonl").write_text("".join(json.dumps(x) + "\n" for x in self.grades))
        return self

    def judge(self, ap=None):
        return g.judge(self.dir, ap)


def check(gate, trial, name):
    t = next(t for t in gate["trials"] if t["trial"] == trial)
    return next(c for c in t["checks"] if c["name"] == name)["state"]


def db_with(run_usage, cos_usage):
    c = sqlite3.connect(":memory:")
    c.row_factory = sqlite3.Row
    c.executescript(
        "CREATE TABLE tasks (id TEXT, project_id TEXT);"
        "CREATE TABLE runs (task_id TEXT, role TEXT, started_at TEXT, usage_json TEXT);"
        "CREATE TABLE chat_threads (id TEXT, project_id TEXT);"
        "CREATE TABLE chat_runs (thread_id TEXT, started_at TEXT, usage_json TEXT);"
        "INSERT INTO tasks VALUES ('T1','P1'); INSERT INTO chat_threads VALUES ('H1','P1');")
    c.execute("INSERT INTO runs VALUES ('T1','worker','2026-01-01T00:00:10Z',?)",
              (None if run_usage is None else json.dumps(run_usage),))
    c.execute("INSERT INTO chat_runs VALUES ('H1','2026-01-01T00:00:05Z',?)",
              (None if cos_usage is None else json.dumps(cos_usage),))
    return c


def drop(d, key):
    d = dict(d)
    d.pop(key)
    return d


class PreFix(unittest.TestCase):
    """修正前の道具が欠測を合格に数え得たこと。"""

    def test_pre_p0f_raw_graded_pass_with_null_cos_usage(self):
        rows = [json.loads(x) for x in (PRE_P0F_RAW / "grade" / "grades.jsonl").read_text().splitlines()]
        self.assertEqual([r["grade"]["verdict"] for r in rows], ["pass", "pass"])
        for f in (PRE_P0F_RAW / "facts").glob("*.json"):
            facts = json.loads(f.read_text())
            self.assertIsNone(facts["usage"]["missing"])
            self.assertTrue(any(i["role"] == "cos" and i["usage"] is None for i in facts["invocations"]))

    def test_prefix_driver_counts_missing_cache_fields_as_complete(self):
        c = db_with(drop(usage(), "cache_creation_tokens"), usage())
        self.assertIsNone(prefix_collect_usage(c, "P1", "claude-code", lambda _: 0)[2])

    def test_prefix_driver_ignores_missing_usd(self):
        c = db_with(drop(usage(), "cost_usd"), usage())
        self.assertIsNone(prefix_collect_usage(c, "P1", "claude-code", lambda _: 0)[2])


class Missing(unittest.TestCase):
    """今の driver と gate は 3 種の欠測を「不明」にする（合格にしない）。"""

    def test_usage_json_null(self):
        c = db_with(usage(), None)
        self.assertEqual(driver.collect_usage(c, "P1", "claude-code", lambda _: 0)[4], "cos_usage_null:1")
        gate = g.judge(PRE_P0F_RAW, approval())
        self.assertEqual(gate["verdict"], "unknown")
        self.assertEqual((gate["g7"], gate["cos_usage_null"]), ("fail", 14))
        r = Raw()
        f = complete_facts()
        f["invocations"][0]["usage"] = None
        self.assertEqual(r.add("t1", f).judge(approval())["verdict"], "unknown")

    def test_cache_field_missing(self):
        c = db_with(drop(usage(), "cache_creation_tokens"), usage())
        self.assertEqual(driver.collect_usage(c, "P1", "claude-code", lambda _: 0)[4], "cache_missing:1")
        f = complete_facts()
        f["invocations"][1]["usage"].pop("cache_read_tokens")
        gate = Raw().add("t1", f).judge(approval())
        self.assertEqual((gate["verdict"], gate["g7"]), ("unknown", "fail"))
        self.assertEqual(check(gate, "t1", "usage_complete"), "unknown")

    def test_usd_not_computed(self):
        c = db_with(drop(usage(), "cost_usd"), usage())
        self.assertEqual(driver.collect_usage(c, "P1", "claude-code", lambda _: 0)[4], "usd_missing:1")
        f = complete_facts()
        f["invocations"][2]["usage"]["cost_usd"] = None
        gate = Raw().add("t1", f).judge(approval())
        self.assertEqual(gate["verdict"], "unknown")
        self.assertIsNone(gate["totals"]["usd"])
        self.assertEqual(next(c for c in gate["checks"] if c["name"] == "cost_cap")["state"], "unknown")

    def test_facts_usd_null_alone_is_unknown(self):
        f = complete_facts()
        f["usage"]["usd"] = None
        self.assertEqual(Raw().add("t1", f).judge(approval())["verdict"], "unknown")


class Gate(unittest.TestCase):
    def test_complete_real_pilot_passes(self):
        r = Raw().add("t1", complete_facts("C06"), approvals=[{"id": "a1", "decision": "once"}]).add(
            "t2", complete_facts("C08"))
        gate = r.judge(approval())
        self.assertEqual((gate["verdict"], gate["g7"], gate["cos_usage_null"]), ("pass", "pass", 0), gate)
        self.assertEqual(gate["totals"]["usd"], 1.5)
        pf = g.pilot_facts(gate)
        self.assertEqual((pf["g7"], pf["cos_runs"], pf["missing"], pf["approval_id"]), ("pass", 2, [], "n1-g7-001"))

    def test_no_approval_is_pending_approval_and_unknown(self):
        gate = Raw().add("t1", complete_facts()).judge(None)
        self.assertEqual((gate["verdict"], gate["approval_state"]), ("unknown", "承認待ち"))

    def test_stub_pilot_keeps_g7_pending_and_no_null_count(self):
        gate = Raw().add("t1", complete_facts(), label="config-stub").judge(approval())
        self.assertEqual((gate["g7"], gate["cos_usage_null"], gate["verdict"]), ("pending", None, "unknown"))
        self.assertIn("not_real_llm", g.pilot_facts(gate)["missing"])

    def test_no_trials_is_unknown(self):
        gate = Raw().judge(approval())
        self.assertEqual((gate["verdict"], gate["g7"]), ("unknown", "pending"))

    def test_no_cos_run_fails(self):
        f = complete_facts()
        f["invocations"] = f["invocations"][1:]
        gate = Raw().add("t1", f).judge(approval())
        self.assertEqual((gate["verdict"], gate["g7"]), ("fail", "fail"))

    def test_no_reviewer_is_unknown(self):
        f = complete_facts()
        f["invocations"] = f["invocations"][:2]
        self.assertEqual(Raw().add("t1", f).judge(approval())["verdict"], "unknown")

    def test_usd_over_the_cap_fails(self):
        gate = Raw().add("t1", complete_facts()).judge(approval(limits={"max_usd": 0.5, "max_wall_secs": 7200}))
        self.assertEqual(gate["verdict"], "fail")

    def test_wall_over_the_cap_fails(self):
        gate = Raw().add("t1", complete_facts()).judge(approval(limits={"max_usd": 5, "max_wall_secs": 60}))
        self.assertEqual(gate["verdict"], "fail")

    def test_quota_cap_cannot_be_judged(self):
        ap = approval(limits={"max_usd": 5, "max_wall_secs": 7200, "max_quota_pct": 20})
        self.assertEqual(Raw().add("t1", complete_facts()).judge(ap)["verdict"], "unknown")

    def test_fixed_append_only_value_is_unknown(self):
        f = complete_facts()
        f.pop("events_checked")
        self.assertEqual(check(Raw().add("t1", f).judge(approval()), "t1", "events_append_only"), "unknown")
        f["events_append_only"] = False
        self.assertEqual(Raw().add("t1", f).judge(approval())["verdict"], "fail")

    def test_unmeasured_rejections_are_unknown_and_launch_rejection_fails(self):
        f = complete_facts()
        f.pop("cos_ops_rejected")
        self.assertEqual(Raw().add("t1", f).judge(approval())["verdict"], "unknown")
        f = complete_facts()
        f["cos_launch_rejected"] = 1
        self.assertEqual(Raw().add("t1", f).judge(approval())["verdict"], "fail")

    def test_leak_double_effect_and_expired_approval_fail(self):
        f = complete_facts()
        f["leak_hits"] = ["runs/x/transcript.jsonl"]
        self.assertEqual(Raw().add("t1", f).judge(approval())["verdict"], "fail")
        r = Raw().add("t1", complete_facts(), grade=grade_row("t1", double=1))
        self.assertEqual(r.judge(approval())["verdict"], "fail")
        r = Raw().add("t1", complete_facts(), approvals=[{"id": "a1", "decision": "expired"}])
        self.assertEqual(check(r.judge(approval()), "t1", "approvals_decided"), "fail")

    def test_missing_oracle_is_unknown(self):
        gate = Raw().add("t1", complete_facts(), grade=False).judge(approval())
        self.assertEqual((check(gate, "t1", "oracle"), gate["verdict"]), ("unknown", "unknown"))

    def test_oracle_fail_is_recorded_not_hidden(self):
        gate = Raw().add("t1", complete_facts(), grade=grade_row("t1", verdict="fail", achieved=True)).judge(approval())
        t = gate["trials"][0]
        self.assertIn("oracle=fail controller_achieved=True mismatch=True",
                      next(c for c in t["checks"] if c["name"] == "oracle")["detail"])

    def test_cli_exit_codes(self):
        r = Raw().add("t1", complete_facts())
        ap = r.dir / "approval.json"
        ap.write_text(json.dumps(approval(expires_at="2999-01-01T00:00:00Z")))
        out, pf = r.dir / "gate.json", r.dir / "pf.json"
        self.assertEqual(g.main(["judge", "--raw", str(r.dir), "--approval", str(ap), "--out", str(out),
                                 "--pilot-facts", str(pf)]), 0)
        self.assertEqual(json.loads(pf.read_text())["g7"], "pass")
        self.assertEqual(g.main(["judge", "--raw", str(r.dir), "--out", str(out)]), 3)
        self.assertEqual(g.main(["check-approval", "--approval", str(ap), "--cases", "C06,C08", "--arms", "new"]), 0)
        self.assertEqual(g.main(["check-approval", "--approval", str(ap), "--cases", "C10", "--arms", "new"]), 2)


class Approval(unittest.TestCase):
    def problems(self, ap, cases=("C06", "C08"), arms=("new",), wall=None):
        return g.check_approval(ap, list(cases), list(arms), wall, now=NOW)

    def test_valid(self):
        self.assertEqual(self.problems(approval()), [])

    def test_expired_or_without_expiry(self):
        self.assertIn("approval expired at 2026-11-06T11:00:00Z",
                      self.problems(approval(expires_at="2026-11-06T11:00:00Z")))
        ap = approval()
        ap.pop("expires_at")
        self.assertIn("approval: expires_at is missing", self.problems(ap))

    def test_not_a_human_or_out_of_scope(self):
        self.assertTrue(self.problems(approval(approved_by="operator:claude-code")))
        self.assertTrue(self.problems(approval(), cases=("C10",)))
        self.assertTrue(self.problems(approval(), arms=("old.passive",)))
        ap = approval()
        ap["scope"] = dict(ap["scope"], environment="production")
        self.assertTrue(self.problems(ap))

    def test_token_limits_and_too_long_wall_are_refused(self):
        ap = copy.deepcopy(approval())
        ap["limits"]["max_tokens"] = 1_000_000
        self.assertIn("approval.limits.max_tokens is not accepted (limits are max_usd, max_wall_secs, max_quota_pct)",
                      self.problems(ap))
        self.assertTrue(self.problems(approval(), wall=14400))
        self.assertTrue(self.problems(approval(limits={"max_wall_secs": 7200})))


if __name__ == "__main__":
    unittest.main()
