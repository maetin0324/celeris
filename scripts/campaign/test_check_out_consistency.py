#!/usr/bin/env python3
"""check-out-consistency.py の試験。合成した out dir（checker 出力・report・export 表を含む）で一致と各種のずれを確かめる。

    python3 scripts/campaign/test_check_out_consistency.py
"""
import copy
import json
import os
import subprocess
import sys
import tempfile
import unittest

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "check-out-consistency.py")
SHA = "a" * 64


def trial(cfg, prob, outcome, **kw):
    r = {"schema": "celeris.campaign-trial/1", "campaign_sha256": SHA, "split": "final", "config": cfg,
         "problem_id": prob, "trial_no": 1, "rep": 1, "outcome": outcome, "passed": outcome == "passed",
         "tokens": 100, "cost_micro_usd": None, "wall_secs": 5, "reason": None,
         "workspace": f"trials/{cfg}/{prob}/rep1/workspace"}
    r.update(kw)
    return r


def grade(cfg, prob, final_pass, failure_class=None):
    g = {"final_pass": final_pass}
    if failure_class:
        g["failure_class"] = failure_class
    return {"schema": "celeris.campaign-grade/1", "graded": True, "grade": g, "failure_class": failure_class,
            "trial": {"config": cfg, "problem_id": prob, "rep": 1, "trial_no": 1}}


def totals(**kw):
    t = {k: 0 for k in ["n", "passed", "final_passed", "grader_failed", "grader_env_failure",
                        "grader_undetermined", "grader_control_only", "grader_observation_failed", "ungraded",
                        "failed", "error", "timeout", "cancelled", "blocked", "missing", "unexecuted",
                        "ran_trials", "tokens_missing_trials", "cost_missing_trials"]}
    t.update(kw)
    return t


def wait(source="missing", reason="記録が無い"):
    return {"source": source, "value": None, "reason": reason}


def report_trial(arm, case):
    t = {"arm": arm, "case_id": case, "rep": 1, "human_time": wait(), "tokens": {"source": "event", "value": 1}}
    for lane in ["human_wait", "celeris_wait", "internal_wait", "external_queue_wait", "external_run"]:
        t[lane] = wait()
    t["external_run"] = {"source": "evaluator", "value": {"union_secs": 3}}
    return t


class Fixture:
    """A: p1 合格・p2 model_failure、B: p1 executor failed（tokens 欠測）・p2 arm 予算で未投入、共有作業 1 件。"""

    def __init__(self, root):
        self.root = root
        self.out = os.path.join(root, "out")
        os.makedirs(self.out)
        self.manifest = {"schema": "celeris.campaign-manifest/1", "campaign_id": "c", "problems": ["p1", "p2"],
                         "experiment_sets": {"main": {"arms": ["A", "B"]}}, "configs": []}
        self.trials = [
            trial("A", "p1", "passed"),
            trial("A", "p2", "passed"),
            trial("B", "p1", "failed", tokens=None),
            trial("B", "p2", "cancelled", tokens=None, reason="arm_budget", workspace="", wall_secs=0),
        ]
        self.grades = [grade("A", "p1", True), grade("A", "p2", False, "model_failure"),
                       grade("B", "p1", False, "model_failure"),
                       {"graded": False, "trial": {"config": "B", "problem_id": "p2", "rep": 1, "trial_no": 1}}]
        self.check = {"ok": True, "complete": False, "violations": [], "incomplete_reasons": ["B: 1 never executed"],
                      "recomputed": {"totals": {
                          "A": totals(n=2, passed=2, final_passed=1, grader_failed=1, ran_trials=2,
                                      cost_missing_trials=2),
                          "B": totals(n=2, failed=1, cancelled=1, unexecuted=1, ran_trials=1,
                                      tokens_missing_trials=1, cost_missing_trials=1)}}}
        self.report = {
            "arms": {
                "A": {"trials": 2, "validated_success": 1, "executor_passed": 2,
                      "success_classes": {"validated_success": 1, "model_failure": 1},
                      "cost_micro_usd_missing_trials": 2},
                "B": {"trials": 2, "success_classes": {"executor_failed": 1, "executor_cancelled": 1},
                      "not_run_trials": 1, "tokens_missing_trials": 1, "tokens_not_run_trials": 1,
                      "cost_micro_usd_missing_trials": 1, "cost_micro_usd_not_run_trials": 1}},
            "shared_overhead": [{"campaign_id": "c", "records": 1}],
            "trials": [report_trial("A", "p1"), report_trial("A", "p2"), report_trial("B", "p1"),
                       report_trial("B", "p2")],
        }
        self.human_ops = [{"scope": "shared", "campaign_id": "c", "op_kind": "env_fix"}]
        self.export = None

    def write(self):
        def dump(name, v):
            with open(os.path.join(self.out, name), "w", encoding="utf-8") as f:
                json.dump(v, f)

        def lines(name, rows):
            with open(os.path.join(self.out, name), "w", encoding="utf-8") as f:
                f.write("".join(json.dumps(r) + "\n" for r in rows))

        dump("campaign.json", self.manifest)
        lines("trials.jsonl", self.trials)
        lines("grades.jsonl", self.grades)
        lines("human-ops.jsonl", self.human_ops)
        with open(os.path.join(self.root, "check.txt"), "w", encoding="utf-8") as f:
            f.write("error: " + json.dumps(self.check, indent=2) + "\n")
        rep = os.path.join(self.root, "human-ops-report")
        os.makedirs(rep, exist_ok=True)
        with open(os.path.join(rep, "report.json"), "w", encoding="utf-8") as f:
            json.dump(self.report, f)
        if self.export is not None:
            os.makedirs(os.path.join(self.out, "export"), exist_ok=True)
            dump(os.path.join("export", "table.incomplete.json"), self.export)

    def run(self, *extra):
        self.write()
        res = os.path.join(self.root, "consistency.json")
        p = subprocess.run([sys.executable, "-I", SCRIPT, "--out", self.out, "--check-json",
                            os.path.join(self.root, "check.txt"), "--json-out", res, *extra],
                           capture_output=True, text=True, check=False)
        doc = None
        if os.path.isfile(res) and p.returncode in (0, 1):
            with open(res, encoding="utf-8") as f:
                doc = json.load(f)
        return p, doc


def export_rows(check):
    rows = []
    for a, t in check["recomputed"]["totals"].items():
        r = dict(t, config=a, failures=t["n"] - t["final_passed"])
        rows.append(r)
    return {"configs": rows}


class Consistency(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.fx = Fixture(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def test_consistent_out_exits_zero_with_missing_and_wait_intervals(self):
        p, d = self.fx.run()
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertTrue(d["consistent"], d["mismatches"])
        self.assertEqual(d["counts"]["B"]["not_run"], {"raw": 1, "report": 1})
        self.assertEqual(d["counts"]["A"]["final_passed"]["checker"], 1)
        # 未導出の wait 区間は lane ごとに理由つきで出る（external_run だけ導出済み）。
        self.assertEqual(d["wait_intervals_underived"],
                         ["celeris_wait", "external_queue_wait", "human_wait", "internal_wait"])
        self.assertTrue(any(w["lane"] == "external_run" and w["derived"] for w in d["wait_intervals"]))
        self.assertTrue(any(m["item"] == "human_time" for m in d["missing"]))
        self.assertTrue(d["shared"]["counted_once"])
        self.assertEqual(d["sources"]["export"]["status"], "not_produced")

    def test_export_table_is_compared_when_present(self):
        self.fx.export = export_rows(self.fx.check)
        p, d = self.fx.run()
        self.assertEqual(p.returncode, 0, d and d["mismatches"])
        self.assertEqual(d["counts"]["A"]["final_passed"]["export"], 1)
        bad = copy.deepcopy(self.fx.export)
        bad["configs"][0]["final_passed"] = 2
        self.fx.export = bad
        p, d = self.fx.run()
        self.assertEqual(p.returncode, 1)
        self.assertIn("A.final_passed: export=2 raw=1", d["mismatches"])

    def test_report_counting_not_run_cells_as_missing_is_a_mismatch(self):
        b = self.fx.report["arms"]["B"]
        b["tokens_missing_trials"], b["tokens_not_run_trials"] = 2, 0
        p, d = self.fx.run()
        self.assertEqual(p.returncode, 1)
        self.assertIn("B.report_tokens_missing: report=2 raw=1", d["mismatches"])

    def test_checker_that_unbinds_grades_disagrees_with_a_report_that_binds_them(self):
        a = self.fx.check["recomputed"]["totals"]["A"]
        a.update(final_passed=0, grader_failed=0, ungraded=2)
        self.fx.check["violations"] = ["lock: grades.jsonl is not bound because the campaign lock is not trusted"]
        p, d = self.fx.run()
        self.assertEqual(p.returncode, 1)
        self.assertTrue(any(m.startswith("A.success_classes: report=") for m in d["mismatches"]), d["mismatches"])
        # checker 側は生記録（未結合）と一致し、ずれは report だけに出る。
        self.assertFalse(any("checker=" in m for m in d["mismatches"]), d["mismatches"])

    def test_checker_missing_count_must_match_planned_cells(self):
        # 計画 cell（B/p2）の記録が無い: 分母に残り missing。checker が分母を減らしていればずれ。
        self.fx.trials = self.fx.trials[:3]
        self.fx.grades = self.fx.grades[:3]
        p, d = self.fx.run()
        self.assertEqual(p.returncode, 1)
        self.assertIn("B.missing: checker=0 raw=1", d["mismatches"])
        self.assertEqual(d["cells"]["B/p2/rep1"]["class"], "missing")

    def test_unexecuted_consumption_is_never_counted_as_observed(self):
        # checker が未実行の試行の tokens を観測済み 0 として扱う（欠測数を減らす）とずれる。
        self.fx.check["recomputed"]["totals"]["B"]["tokens_missing_trials"] = 0
        p, d = self.fx.run()
        self.assertEqual(p.returncode, 1)
        self.assertIn("B.tokens_missing_trials: checker=0 raw=1", d["mismatches"])

    def test_shared_overhead_must_be_counted_once(self):
        self.fx.report["shared_overhead"] = [{"campaign_id": "c", "records": 1}, {"campaign_id": "c", "records": 1}]
        p, d = self.fx.run()
        self.assertEqual(p.returncode, 1)
        self.assertFalse(d["shared"]["counted_once"])
        self.assertIn("shared_overhead lists a campaign more than once", d["mismatches"])

    def test_not_an_out_dir_is_an_input_error(self):
        os.makedirs(os.path.join(self.tmp.name, "empty"))
        p = subprocess.run([sys.executable, "-I", SCRIPT, "--out", os.path.join(self.tmp.name, "empty"),
                            "--check-json", os.path.join(self.tmp.name, "nothing")],
                           capture_output=True, text=True, check=False)
        self.assertEqual(p.returncode, 2, p.stderr)
        self.assertIn("campaign.json is missing", p.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)
