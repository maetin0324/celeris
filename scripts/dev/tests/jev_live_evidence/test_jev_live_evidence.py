import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "..", "..", "jev-live-evidence.py")
sys.path.insert(0, HERE)
import builder as b  # noqa: E402


def run(directory, live=True):
    args = [sys.executable, SCRIPT, "--evidence-dir", directory, "--quiet"] + (["--require-live"] if live else [])
    p = subprocess.run(args, capture_output=True, text=True)
    return p.returncode, p.stderr


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)

    def check(self, case, want_code, needle=None, **kw):
        d = b.write(case, os.path.join(self.tmp.name, "ev"), **kw)
        code, err = run(d)
        self.assertEqual(code, want_code, err)
        if needle:
            self.assertIn(needle, err)
        return err


class Positive(Base):
    def test_real_export_shape_passes(self):
        self.check(b.default_case(), 0)

    def test_report_has_stage_counts(self):
        d = b.write(b.default_case(), os.path.join(self.tmp.name, "ev"))
        p = subprocess.run([sys.executable, SCRIPT, "--evidence-dir", d, "--require-live"], capture_output=True, text=True)
        self.assertEqual(p.returncode, 0, p.stderr)
        import json
        rep = json.loads(p.stdout)
        c = rep["stages"]["by_arm"]["C"]
        self.assertEqual((c["trials"], c["jev_ok"], c["live_proven"], c["final_lane_changed"]), (4, 4, 4, 4))
        self.assertEqual(rep["stages"]["by_arm"]["B"]["jev_ok"], 0)

    def test_fallback_is_counted_as_suppression_not_success(self):
        case = b.default_case()
        b.set_event(case, "r2", jev_ok=False)
        case["records"] = [r for r in case["records"] if not (r["record_type"] == "usage" and r["run_id"] == "r2" and r["role"] == "jev")]
        err = self.check(case, 0)
        self.assertIn("OK", err)


class NoEvidence(Base):
    def test_missing_dir(self):
        code, err = run(os.path.join(self.tmp.name, "none"))
        self.assertEqual(code, 2)
        self.assertIn("live 証拠なし", err)

    def test_empty_dir(self):
        code, err = run(self.tmp.name)
        self.assertEqual(code, 2)
        self.assertIn("live 証拠なし", err)


class HandMadeFixture(Base):
    def test_fixture_only_fails_require_live(self):
        case = b.default_case()
        err = self.check(case, 1, "provenance", prov_over={"source": "hand-made", "fixture": True})
        self.assertIn("手組み fixture", err)

    def test_fixture_task_ids_fail(self):
        case = b.default_case()
        for r in case["records"]:
            if r.get("task_id") == b.ROOT:
                r["task_id"] = "task-1"
        for e in case["events"]:
            if e["task_id"] == b.ROOT:
                e["task_id"] = "task-1"
        case["tasks"]["task-1"] = case["tasks"].pop(b.ROOT)
        case["tasks"][b.CHILD]["parent"] = "task-1"
        self.check(case, 1, "ULID")

    def test_edited_after_export_fails(self):
        d = b.write(b.default_case(), os.path.join(self.tmp.name, "ev"))
        with open(os.path.join(d, "records.jsonl"), "a") as f:
            f.write("\n")
        code, err = run(d)
        self.assertEqual(code, 1)
        self.assertIn("files_sha256", err)

    def test_missing_provenance_file(self):
        d = b.write(b.default_case(), os.path.join(self.tmp.name, "ev"))
        os.remove(os.path.join(d, "provenance.json"))
        code, err = run(d)
        self.assertEqual(code, 2)
        self.assertIn("provenance.json", err)

    def test_manifest_hash_mismatch(self):
        self.check(b.default_case(), 1, "manifest_hash", prov_over={"manifest_hash": "b" * 64})


class Negative(Base):
    def test_fake_success_real_api_verified_constant(self):
        # classifier は fallback だが real_api_verified=true の定数だけで成功を主張
        case = b.default_case()
        for run_id in ("r1", "r2", "r3", "r4"):
            b.set_event(case, run_id, jev_ok=False, real_api_verified=True)
        case["records"] = [r for r in case["records"] if not (r["record_type"] == "usage" and r["role"] == "jev")]
        err = self.check(case, 1, "fake-success")
        self.assertIn("auth", err)

    def test_jev_ok_without_usage_or_version(self):
        case = b.default_case()
        case["records"] = [r for r in case["records"] if not (r["record_type"] == "usage" and r["role"] == "jev")]
        self.check(case, 1, "Jev の消費行が無い")

    def test_missing_cost_not_counted_as_success(self):
        case = b.default_case()
        for r in case["records"]:
            if r["record_type"] == "usage" and r["role"] == "jev":
                r["list_cost_usd"] = r["cash_cost_usd"] = None
        self.check(case, 1, "費用が欠測")

    def test_missing_version(self):
        case = b.default_case()
        for r in case["records"]:
            if r["record_type"] == "trial":
                r["classifier_snapshot"] = None
        self.check(case, 1, "応答版")

    def test_pin_mismatch_in_grandchild(self):
        case = b.default_case()
        b.set_event(case, "r3", resolved=dict(b.RESOLVED, lower_policy="other"))
        self.check(case, 1, "root")

    def test_descendant_without_pin(self):
        case = b.default_case()
        for e in case["events"]:
            if e["run_id"] == "r4":
                del e["event"]["record"]["features"]
        self.check(case, 1, "routing_resolved")

    def test_missing_event_for_retry(self):
        case = b.default_case()
        case["events"] = [e for e in case["events"] if e["run_id"] != "r4"]
        self.check(case, 1, "routing_events")

    def test_missing_exec_model(self):
        case = b.default_case()
        for r in case["records"]:
            if r["record_type"] == "trial" and r["run_id"] == "r3":
                r["exec_model"] = None
        self.check(case, 1, "exec_model")

    def test_b1_c1_mixture_b_arm_with_jev(self):
        case = b.default_case()
        case["records"].append(b.usage("rb", b.BROOT, "jev", 0.01))
        self.check(case, 1, "B1")

    def test_arm_mode_mismatch(self):
        case = b.default_case()
        for r in case["records"]:
            if r["record_type"] == "trial" and r["run_id"] == "rb":
                r["routing_mode"] = "decision_assisted"
        self.check(case, 1, "mixed-arm")

    def test_primary_arm_A_mixed_in(self):
        case = b.default_case()
        for r in case["records"]:
            if r["record_type"] == "trial" and r["run_id"] == "rb":
                r["arm"] = "A"
        self.check(case, 1, "arm が B/C ではない")

    def test_other_campaign_rows_mixed(self):
        case = b.default_case()
        case["records"].append(b.usage("rx", "01M5A0000000000000000XX001", "worker", 1.0, campaign="other-campaign"))
        self.check(case, 1, "別 campaign")

    def test_other_manifest_hash_row(self):
        case = b.default_case()
        for r in case["records"]:
            if r["record_type"] == "trial" and r["run_id"] == "r2":
                r["manifest_hash"] = "c" * 64
        self.check(case, 1, "mixed-manifest")

    def test_c1_single_model_pin(self):
        case = b.default_case()
        for r in case["records"]:
            if r["record_type"] == "trial":
                r["proposed_candidates"] = b.CANDS[:1]
                r["proposed_first"] = b.CANDS[0]
                r["exec_model"] = "claude-sonnet-5-5"
        self.check(case, 1, "単一モデル")

    def test_c1_explicit_lane_pin(self):
        case = b.default_case()
        b.set_event(case, "r1", overridden_by="explicit")
        self.check(case, 1, "明示 lane")

    def test_evidence_table_used(self):
        case = b.default_case()
        b.set_event(case, "r1", resolved=dict(b.RESOLVED, evidence_mode="pinned", evidence_table_id="t1"))
        self.check(case, 1, "根拠表")

    def test_summary_cost_mismatch(self):
        case = b.default_case()
        s = b.build_summary(case["records"])
        s["by_role"]["jev"]["list_cost_usd"]["sum"] = 99.0
        self.check(case, 1, "role jev", summary=s)

    def test_no_retry_or_grandchild_coverage(self):
        case = b.default_case()
        keep = lambda r: r.get("run_id") not in ("r3", "r4")
        case["records"] = [r for r in case["records"] if keep(r)]
        case["events"] = [e for e in case["events"] if e["run_id"] not in ("r3", "r4")]
        self.check(case, 1, "孫")


if __name__ == "__main__":
    unittest.main()
