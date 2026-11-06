#!/usr/bin/env python3
"""pc_rollout.py の純粋な関数の試験（daemon なし）。python3 -I scripts/dev/pc-rollout/test_pc_rollout.py"""
import datetime
import pathlib
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import pc_rollout as p  # noqa: E402

API = "http://127.0.0.1:7721"
GEN = "a" * 64
NOW = datetime.datetime(2026, 11, 6, tzinfo=datetime.timezone.utc)


def appr(kind="observe", stages=("dry_run", "shadow"), **kw):
    d = {"approved_by": "human:rmaeda", "kind": kind, "stages": list(stages), "scope": "production",
         "project_id": "P1", "api": API, "expires_at": "2026-11-07T00:00:00Z", "generation": GEN}
    d.update(kw)
    return d


GATE, GATE_SCHEMA = "schema", "celeris.long-horizon-pilot-gate/1"


def facts(**kw):
    f = {"project": "P1", "git": {"missing_subjects": [], "release_contains_required": True},
         "config": {"read": True, "enabled": True, "allowed_project_ids": ["P1"], "sha256": "x", "budget_present": True},
         "daemon_config_sha256": "x", "other_modes": {"P2": "off"}, "domain": {"mode": "compare"},
         "guard_halted": None, "controller": {"mode": "dry_run"},
         "decisions": {"total": 2, "dry_run": 2, "not_suppressed": 0}, "bad_wait_rule": None, "controller_roots": 0,
         "approval_check": (True, "ok"), "pilot_facts": {GATE: GATE_SCHEMA, "g7": "pass", "cos_runs": 3, "cos_usage_null": 0, "missing": []}}
    f.update(kw)
    return f


def states(rows):
    return {r["id"]: r["state"] for r in rows}


class Approval(unittest.TestCase):
    def check(self, a, stage="shadow", env=None, sha=None):
        return p.check_approval(a, stage, "P1", API, env or {}, now=NOW, shadow_report_sha=sha)

    def test_observe_approval_covers_shadow_only(self):
        self.assertTrue(self.check(appr())[0])
        ok, why = self.check(appr(), "canary")
        self.assertFalse(ok)
        self.assertIn("kind effects", why)
        self.assertFalse(self.check(appr(), "enabled")[0])

    def test_effects_approval_needs_matching_shadow_report(self):
        a = appr("effects", ["canary"], shadow_report_sha256="s1", canary_bps=100)
        self.assertFalse(self.check(a, "canary", sha=None)[0])
        self.assertFalse(self.check(a, "canary", sha="other")[0])
        self.assertTrue(self.check(a, "canary", sha="s1")[0])
        self.assertFalse(self.check(appr("effects", ["canary"], shadow_report_sha256="s1"), "canary", sha="s1")[0])
        self.assertFalse(self.check(a, "enabled", sha="s1")[0])  # enabled は stages に無い

    def test_rejects_non_human_expired_other_project_api_generation(self):
        self.assertFalse(self.check(appr(approved_by="operator:claude-code"))[0])
        self.assertFalse(self.check(appr(expires_at="2026-11-05T00:00:00Z"))[0])
        self.assertFalse(self.check(appr(project_id="P2"))[0])
        self.assertFalse(self.check(appr(api="http://127.0.0.1:9999"))[0])
        self.assertFalse(self.check(appr(generation="short"))[0])
        self.assertFalse(self.check(None)[0])

    def test_production_approval_refused_inside_worker_run(self):
        ok, why = self.check(appr(), env={"CELERIS_RUN_ID": "r1"})
        self.assertFalse(ok)
        self.assertIn("worker run", why)
        self.assertTrue(self.check(appr(scope="non-production"), env={"CELERIS_RUN_ID": "r1"})[0])


class Criteria(unittest.TestCase):
    def test_all_pass_is_go(self):
        rows = p.criteria("shadow", facts(), "production")
        self.assertEqual(p.verdict(rows), "go", rows)

    def test_missing_pilot_facts_is_pending_in_production(self):
        rows = p.criteria("shadow", facts(pilot_facts=None), "production")
        self.assertEqual(states(rows)["G7"], "pending")
        self.assertEqual(p.verdict(rows), "no-go (pending)")

    def test_cos_usage_null_fails(self):
        pf = {GATE: GATE_SCHEMA, "g7": "fail", "cos_runs": 14, "cos_usage_null": 14, "missing": ["cos_usage_null:14"]}
        rows = p.criteria("shadow", facts(pilot_facts=pf), "production")
        self.assertEqual(states(rows)["G7"], "fail")

    def test_pilot_facts_not_from_the_gate_fail(self):
        # 修正前の形（cos_runs と missing だけ）は G7 を通さない。driver の facts をそのまま渡しても同じ。
        rows = p.criteria("shadow", facts(pilot_facts={"cos_runs": 3, "missing": []}), "production")
        self.assertEqual(states(rows)["G7"], "fail")

    def test_stub_pilot_keeps_g7_pending(self):
        pf = {GATE: GATE_SCHEMA, "g7": "pending", "cos_runs": 3, "cos_usage_null": None, "missing": ["not_real_llm"]}
        rows = p.criteria("shadow", facts(pilot_facts=pf), "production")
        self.assertEqual(states(rows)["G7"], "pending")

    def test_g7_pass_with_null_count_fails(self):
        pf = {GATE: GATE_SCHEMA, "g7": "pass", "cos_runs": 3, "cos_usage_null": None, "missing": []}
        self.assertEqual(states(p.criteria("shadow", facts(pilot_facts=pf), "production"))["G7"], "fail")

    def test_missing_fix_on_main_fails(self):
        rows = p.criteria("shadow", facts(git={"missing_subjects": ["x"], "release_contains_required": None}),
                          "production")
        self.assertEqual(states(rows)["G1"], "fail")

    def test_two_allowed_projects_or_other_project_on_fails(self):
        cfg = dict(facts()["config"], allowed_project_ids=["P1", "P2"])
        self.assertEqual(states(p.criteria("shadow", facts(config=cfg), "production"))["G2"], "fail")
        self.assertEqual(states(p.criteria("shadow", facts(other_modes={"P2": "shadow"}), "production"))["G3"], "fail")

    def test_config_file_differs_from_daemon_fails(self):
        self.assertEqual(states(p.criteria("shadow", facts(daemon_config_sha256="y"), "production"))["G2s"], "fail")

    def test_shadow_needs_suppressed_dry_run_decisions(self):
        f = facts(decisions={"total": 0, "dry_run": 0, "not_suppressed": 0})
        self.assertEqual(states(p.criteria("shadow", f, "production"))["G5"], "fail")
        f = facts(decisions={"total": 2, "dry_run": 2, "not_suppressed": 1})
        self.assertEqual(states(p.criteria("shadow", f, "production"))["G5"], "fail")
        f = facts(bad_wait_rule="pc/script-unavailable")
        self.assertEqual(states(p.criteria("shadow", f, "production"))["G5w"], "fail")

    def test_canary_needs_24h_clean_production_shadow_report(self):
        rep = {"schema": p.SCHEMA, "kind": "monitor", "stage": "shadow", "scope": "production",
               "observed_secs": 90000, "violations": []}
        f = facts(domain={"mode": "compare"}, controller={"mode": "shadow"}, shadow_report=rep)
        self.assertEqual(states(p.criteria("canary", f, "production"))["G5"], "pass")
        short = dict(rep, observed_secs=91)
        self.assertEqual(states(p.criteria("canary", dict(f, shadow_report=short), "production"))["G5"], "fail")
        nonprod = dict(rep, scope="non-production")
        self.assertEqual(states(p.criteria("canary", dict(f, shadow_report=nonprod), "production"))["G5"], "fail")
        dirty = dict(rep, violations=["x"])
        self.assertEqual(states(p.criteria("canary", dict(f, shadow_report=dirty), "production"))["G5"], "fail")

    def test_guard_halted_fails(self):
        self.assertEqual(states(p.criteria("shadow", facts(guard_halted="x"), "production"))["G4"], "fail")


class Diff(unittest.TestCase):
    base = {"project_mode": "shadow", "domain_mode": "compare", "domain_generation": GEN, "other_modes": {"P2": "off"},
            "daemon_config_sha256": "x", "daemon_instance": "i1", "effects": {"suppressed": 1}, "controller_roots": 0,
            "cos_inputs_total": 0, "decisions_total": 1}

    def test_suppressed_and_none_decisions_are_clean(self):
        cur = dict(self.base, effects={"suppressed": 3, "none": 1}, decisions_total=4, daemon_instance="i2")
        d = p.diff(self.base, cur, "shadow")
        self.assertEqual(d["violations"], [])
        self.assertTrue(d["changes"]["daemon_restarted"])

    def test_effects_roots_cos_inputs_drift_are_violations(self):
        cases = [dict(effects={"suppressed": 1, "applied": 1}), dict(controller_roots=1), dict(cos_inputs_total=1),
                 dict(other_modes={"P2": "dry_run"}), dict(domain_mode="ts"), dict(daemon_config_sha256="y"),
                 dict(project_mode="enabled"), dict(wait_rule="pc/script-unavailable"), dict(error="down")]
        for c in cases:
            with self.subTest(c=c):
                self.assertTrue(p.diff(self.base, dict(self.base, **c), "shadow")["violations"])


class Paths(unittest.TestCase):
    def test_production_paths(self):
        self.assertTrue(p.production_path("/local/celeris/state/x"))
        self.assertFalse(p.production_path("/local/celeris/data/workspaces/x"))


if __name__ == "__main__":
    unittest.main()
