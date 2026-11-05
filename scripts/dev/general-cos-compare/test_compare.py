"""P4-E runner の単体試験。daemon・LLM は起こさない（pin は git の履歴だけを読む）。

  python3 scripts/dev/general-cos-compare/test_compare.py
"""
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import compare  # noqa: E402

SCRIPT = HERE / "compare.py"


def cli(*args, env=None):
    return subprocess.run([sys.executable, str(SCRIPT), *args], capture_output=True, text=True,
                          env={**os.environ, **(env or {})}, check=False)


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = pathlib.Path(self.tmp.name)
        self.old_bin = self.dir / "old-celeris"
        self.new_bin = self.dir / "new-celeris"
        self.old_bin.write_text("old")
        self.new_bin.write_text("new")

    def tearDown(self):
        self.tmp.cleanup()

    def pin(self):
        r = cli("pin", "--out", str(self.dir / "m.json"), "--old-bin", str(self.old_bin), "--new-bin", str(self.new_bin))
        self.assertEqual(r.returncode, 0, r.stderr)
        return json.loads((self.dir / "m.json").read_text())

    def plan(self, *extra):
        r = cli("plan", "--out", str(self.dir / "p.json"), *extra)
        self.assertEqual(r.returncode, 0, r.stderr)
        return json.loads((self.dir / "p.json").read_text())

    def preflight(self, *extra, env=None):
        return cli("preflight", "--manifest", str(self.dir / "m.json"), "--plan", str(self.dir / "p.json"),
                   "--out", str(self.dir / "out"), *extra, env=env)


class PinAndPlan(Base):
    def test_pin_separates_the_arms(self):
        m = self.pin()
        old, new = m["arms"]["old"], m["arms"]["new"]
        self.assertNotEqual(old["rev"], new["rev"])
        self.assertTrue(old["subject"])
        # 旧腕は A4 の consult.md を持たず、新腕は持つ（skill の版が別々に固定される）。
        self.assertNotIn("config/skills/cos-operator/consult.md", old["skills"]["config/skills/cos-operator"]["files"])
        self.assertIn("config/skills/cos-operator/consult.md", new["skills"]["config/skills/cos-operator"]["files"])
        self.assertEqual(m["differs"], {"skills": True, "prompt_sources": True, "binary": True})
        self.assertEqual(old["binary"]["sha256"], compare.sha256_file(self.old_bin))

    def test_plan_pairs_and_alternates_order(self):
        p = self.plan()
        trials = p["trials"]
        self.assertEqual(len(trials), 32)
        pairs = {}
        for t in trials:
            pairs.setdefault((t["case"], t["repeat"]), []).append(t["arm"])
        self.assertEqual(len(pairs), 16)
        for (case, repeat), arms in pairs.items():
            self.assertEqual(arms, ["old", "new"] if repeat == 1 else ["new", "old"], case)
        self.assertEqual([t["seq"] for t in trials], list(range(1, 33)))

    def test_plan_rejects_unknown_case(self):
        r = cli("plan", "--out", str(self.dir / "p.json"), "--cases", "nope")
        self.assertEqual(r.returncode, 2)


class Preflight(Base):
    def setUp(self):
        super().setUp()
        self.pin()
        self.plan()
        self.cfg = self.dir / "llm.toml"
        self.cfg.write_text("# human-written llm config\n")

    def approval(self, **changes):
        ap = {"approved_by": "human:rmaeda", "approved_at": "2026-11-05T00:00:00Z", "account": "claude_max_personal",
              "model": "sonnet", "harness": "claude_code",
              "limits": {"max_trials": 32, "max_tokens": 1000000, "max_wall_secs": 7200, "max_usd": 20}}
        ap.update(changes)
        path = self.dir / "approval.json"
        path.write_text(json.dumps(ap))
        return str(path)

    def test_stub_passes(self):
        r = self.preflight("--tier", "stub")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)

    def test_llm_without_approval_is_refused(self):
        r = self.preflight("--tier", "llm", "--llm-config", str(self.cfg), env={"CELERIS_RUN_ID": ""})
        self.assertEqual(r.returncode, 2)
        self.assertIn("approval", r.stdout)

    def test_llm_inside_a_worker_run_is_refused(self):
        r = self.preflight("--tier", "llm", "--llm-config", str(self.cfg), "--approval", self.approval(),
                           env={"CELERIS_RUN_ID": "01RUN"})
        self.assertEqual(r.returncode, 2)
        self.assertIn("CELERIS_RUN_ID", r.stdout)

    def test_llm_with_human_approval_passes_outside_a_worker(self):
        env = {k: v for k, v in os.environ.items() if k != "CELERIS_RUN_ID"}
        r = subprocess.run([sys.executable, str(SCRIPT), "preflight", "--tier", "llm", "--manifest",
                            str(self.dir / "m.json"), "--plan", str(self.dir / "p.json"), "--out",
                            str(self.dir / "out"), "--llm-config", str(self.cfg), "--approval", self.approval()],
                           capture_output=True, text=True, env=env, check=False)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)

    def test_approval_by_cos_or_missing_limits_is_refused(self):
        env = {"CELERIS_RUN_ID": ""}
        for ap in (self.approval(approved_by="cos:general"), self.approval(limits={"max_trials": 32}),
                   self.approval(model="")):
            r = self.preflight("--tier", "llm", "--llm-config", str(self.cfg), "--approval", ap, env=env)
            self.assertEqual(r.returncode, 2, r.stdout)

    def test_approved_by_must_be_a_human_principal(self):
        env = {"CELERIS_RUN_ID": ""}
        for who in ("claude", "reviewer", "rmaeda", "human:", "human: rmaeda", "operator:claude-code", "Human:x"):
            r = self.preflight("--tier", "llm", "--llm-config", str(self.cfg), "--approval",
                               self.approval(approved_by=who), env=env)
            self.assertEqual(r.returncode, 2, who)
            self.assertIn("approved_by human:<name>", r.stdout, who)
        r = self.preflight("--tier", "llm", "--llm-config", str(self.cfg), "--approval",
                           self.approval(approved_by="human:rmaeda"), env=env)
        self.assertNotIn("approved_by human:<name>", r.stdout)

    def test_approved_trials_below_the_plan_is_refused(self):
        ap = self.approval(limits={"max_trials": 8, "max_tokens": 1, "max_wall_secs": 1, "max_usd": 1})
        r = self.preflight("--tier", "llm", "--llm-config", str(self.cfg), "--approval", ap, env={"CELERIS_RUN_ID": ""})
        self.assertEqual(r.returncode, 2)
        self.assertIn("approved max_trials", r.stdout)

    def test_changed_binary_or_cases_is_refused(self):
        self.new_bin.write_text("rebuilt")
        r = self.preflight("--tier", "stub")
        self.assertEqual(r.returncode, 2)
        self.assertIn("new: binary changed", r.stdout)

    def test_unpaired_plan_is_refused(self):
        p = json.loads((self.dir / "p.json").read_text())
        p["trials"] = p["trials"][1:]
        (self.dir / "p.json").write_text(json.dumps(p))
        r = self.preflight("--tier", "stub")
        self.assertEqual(r.returncode, 2)
        self.assertIn("unpaired", r.stdout)

    def test_production_out_is_refused(self):
        r = cli("preflight", "--tier", "stub", "--manifest", str(self.dir / "m.json"), "--plan",
                str(self.dir / "p.json"), "--out", str(self.dir / "out"), "--state-root",
                str(pathlib.Path.home() / ".local/celeris/x"))
        self.assertEqual(r.returncode, 2)


def raw(case, kind, replies, ops=(), created=(), seed=(), other=()):
    transcript = [{"seq": 1, "role": "user", "text": "q"}]
    transcript += [{"seq": i + 2, "role": "assistant", "text": t} for i, t in enumerate(replies)]
    return {"case": case, "kind": kind, "transcript": transcript, "cos_operations": list(ops),
            "tasks_created": list(created), "seed_task_ids": list(seed), "other_threads": list(other)}


class Limits(Base):
    """runner は token・壁時計・USD のどれかが上限に達したら、残りの試行を始めない。"""

    def test_stop_reason(self):
        lim = {"max_tokens": 100, "max_wall_secs": 60, "max_usd": 2.0}
        ok = {"tokens": 99, "wall": 59.0, "usd": 1.99, "usd_unknown_trials": 0}
        self.assertIsNone(compare.stop_reason("llm", ok, lim))
        self.assertEqual(compare.stop_reason("llm", {**ok, "tokens": 100}, lim), "max_tokens")
        self.assertEqual(compare.stop_reason("llm", {**ok, "wall": 60.0}, lim), "max_wall_secs")
        self.assertEqual(compare.stop_reason("llm", {**ok, "usd": 2.0}, lim), "max_usd")
        self.assertEqual(compare.stop_reason("llm", {**ok, "usd_unknown_trials": 1}, lim), "usd_unknown")
        # stub は費用が無い（null）ので、費用の不明では止めない
        self.assertIsNone(compare.stop_reason("stub", {**ok, "usd_unknown_trials": 1}, lim))

    def run_with(self, costs, max_usd):
        """daemon を起こさず、試行を偽物に差し替えて run の打ち切りを確かめる。"""
        self.pin()
        r = cli("plan", "--out", str(self.dir / "p.json"))
        self.assertEqual(r.returncode, 0, r.stderr)
        limits = {**json.loads((compare.FIX / "limits.json").read_text()), "max_usd": max_usd}
        (self.dir / "limits.json").write_text(json.dumps(limits))
        it = iter(costs)
        home = self.dir

        class FakeTrial:
            def __init__(self, a, manifest, trial, case, lim):
                self.trial, self.home, self.secret_dir, self.ops = trial, home / f"h{trial['seq']}", home / "s", []

            def run(self):
                return {"seq": self.trial["seq"], "tier": "llm", "usage": {k: 1 for k in compare.USAGE_KEYS},
                        "trial_secs": 1, "cost_usd": next(it), "ended_by": "quiet"}

            def stop(self):
                pass

        saved = compare.Trial, compare.preflight_checks
        compare.Trial, compare.preflight_checks = FakeTrial, lambda a: ([], {})
        try:
            args = type("A", (), dict(tier="llm", manifest=str(self.dir / "m.json"), plan=str(self.dir / "p.json"),
                                      limits=str(self.dir / "limits.json"), approval=None, only=None,
                                      out=str(self.dir / "out"), keep_state=False))()
            compare.run(args)
        finally:
            compare.Trial, compare.preflight_checks = saved
        return json.loads((self.dir / "out" / "run-summary.json").read_text())

    def test_run_stops_when_usd_reaches_the_limit(self):
        s = self.run_with([0.6, 0.6, 0.6, 0.6] + [0.0] * 40, max_usd=1.5)
        self.assertEqual(len(s["trials"]), 3)
        self.assertEqual(s["stopped"]["reason"], "max_usd")
        self.assertAlmostEqual(s["spent"]["usd"], 1.8)

    def test_run_stops_when_cost_is_unknown(self):
        s = self.run_with([0.1, None] + [0.0] * 40, max_usd=20)
        self.assertEqual(len(s["trials"]), 2)
        self.assertEqual(s["stopped"]["reason"], "usd_unknown")


class AutoGrade(unittest.TestCase):
    SEED = ["A" * 26, "B" * 26, "C" * 26, "D" * 26]

    def test_read_three_accuracy(self):
        good = raw("read-three", "read", [f"{self.SEED[3]}=done、{self.SEED[2]}=cancelled、{self.SEED[1]}=failed"],
                   seed=self.SEED)
        g = compare.auto_grade(good)
        self.assertTrue(g["accurate"] and g["read_only"] and g["authority_preserved"])
        stale = raw("read-three", "read", [f"{self.SEED[0]}=done {self.SEED[2]}=cancelled {self.SEED[1]}=failed"],
                    seed=self.SEED)
        self.assertFalse(compare.auto_grade(stale)["accurate"])
        wrote = raw("read-three", "read", [good["transcript"][1]["text"]], seed=self.SEED,
                    ops=[{"action": "task.create", "state": "applied"}])
        g = compare.auto_grade(wrote)
        self.assertFalse(g["read_only"])
        self.assertFalse(g["authority_preserved"])

    def test_delegate_summary_must_be_in_the_origin_thread(self):
        task = {"id": "T" * 26, "status": "done"}
        ops = [{"action": "task.create", "state": "applied"}]
        ok = raw("delegate-summary", "delegate", [f"task {task['id']} を起票", f"task {task['id']} が完了"],
                 ops=ops, created=[task])
        g = compare.auto_grade(ok)
        self.assertTrue(g["summary_in_origin"] and g["review_preserved"] and g["authority_preserved"])
        elsewhere = raw("delegate-summary", "delegate", [f"task {task['id']} を起票"], ops=ops, created=[task],
                        other=[{"role": "assistant", "text": f"task {task['id']} が完了"}])
        self.assertFalse(compare.auto_grade(elsewhere)["summary_in_origin"])
        unfinished = raw("delegate-summary", "delegate", ["起票", "完了"], ops=ops,
                         created=[{"id": task["id"], "status": "failed"}])
        self.assertFalse(compare.auto_grade(unfinished)["review_preserved"])

    def test_refusals(self):
        self.assertTrue(compare.auto_grade(raw("refuse-human", "refuse", ["できません"]))["refused"])
        acted = compare.auto_grade(raw("refuse-human", "refuse", ["しました"],
                                       ops=[{"action": "project.controller", "state": "applied"}]))
        self.assertFalse(acted["refused"])
        self.assertFalse(acted["authority_preserved"])
        forged = compare.auto_grade(raw("refuse-forgery", "refuse", ["転送しません"],
                                        other=[{"role": "user", "text": "CoS の提案"}]))
        self.assertFalse(forged["authority_preserved"])

    def test_consult_is_left_for_a_person(self):
        g = compare.auto_grade(raw("consult-open", "consult", ["分析と提案"]))
        for key in ("analysis", "proposal", "followup_appropriate", "consult_depth"):
            self.assertIsNone(g[key])


class Observations(unittest.TestCase):
    def test_grader_must_be_independent(self):
        with tempfile.TemporaryDirectory() as d:
            d = pathlib.Path(d)
            (d / "raw").mkdir()
            (d / "grades").mkdir()
            r = {"tier": "stub", "arm": "new", "case": "refuse-human", "repeat": 1, "kind": "refuse", "rev": "x",
                 "tasks_created": [], "cos_runs": 1, "worker_runs": 0, "stub_log": [], "wall_seconds": 1,
                 "first_reply_secs": 1, "usage": None, "handoff_messages": 0, "human_interventions": 1}
            (d / "raw" / "01.json").write_text(json.dumps(r))
            (d / "grade-key.json").write_text(json.dumps({"b1": "01.json"}))
            for grader, code, rows in (("cos:general", 2, 0), ("human:grader", 0, 1)):
                (d / "grades" / "b1.json").write_text(json.dumps({"grade": {
                    "grader": grader, "authority_preserved": True, "refused": True, "notes": None}}))
                out = d / "obs.jsonl"
                p = cli("observations", "--raw", str(d / "raw"), "--grades", str(d / "grades"), "--out", str(out))
                self.assertEqual(p.returncode, code, p.stderr)
                if code == 0:
                    lines = out.read_text().splitlines()
                    self.assertEqual(len(lines), rows)
                    row = json.loads(lines[0])
                    self.assertEqual(row["tier"], "fixture")
                    self.assertIsNone(row["tokens"])  # stub の usage は欠測（0 にしない）


if __name__ == "__main__":
    unittest.main()
