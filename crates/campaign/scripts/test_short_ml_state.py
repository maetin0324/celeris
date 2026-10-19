"""verify-short-ml-case.py の状態検査の unit test（標準ライブラリのみ）。
実行: python3 -m unittest crates/campaign/scripts/test_short_ml_state.py
"""
import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCRIPT = HERE / "verify-short-ml-case.py"
spec = importlib.util.spec_from_file_location("vsmc", SCRIPT)
vsmc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(vsmc)


def run(*args):
    return subprocess.run([sys.executable, str(SCRIPT), *args], capture_output=True, text=True)


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = Path(self.tmp.name)
        saved = {k: ["x"] for k in vsmc.SAVED_KEYS}
        (self.dir / "re.json").write_text(json.dumps(
            {"withdrawn_decisions": [{"saved": {"status_json": saved}}]}))
        (self.dir / "rv.md").write_text("## 撤回した判断\n")
        self.campaign = self.dir / "campaign"
        (self.campaign / "optional" / "short-ml-campaign").mkdir(parents=True)
        self.path = self.campaign / "optional" / "short-ml-campaign" / "status.json"

    def status(self, **over):
        s = {
            "schema": vsmc.SCHEMA, "task_id": vsmc.TASK_ID, "status": "pending", "label": "dev",
            "confirm_at": "2026-10-22T00:00:00Z",
            "claims": {"final_generalization": False, "real_pilot_confirmed": False, "adopted": False},
            "external_refs": {"reassessment": str(self.dir / "re.json"), "review": str(self.dir / "rv.md")},
        }
        s.update(over)
        for k in [k for k, v in s.items() if v is None]:
            del s[k]
        self.path.write_text(json.dumps(s))

    def errs(self):
        return vsmc.check_state(self.path, self.campaign)

    def placement(self):
        return vsmc.check_placement(self.path)


class State(Base):
    def test_pending_ok(self):
        self.status()
        self.assertEqual(self.errs(), [])

    def test_pending_requires_confirm_at(self):
        self.status(confirm_at="2026-10-22")
        self.assertTrue(self.errs())

    def test_deferred_and_adopted_structure(self):
        self.status(status="deferred", confirm_at=None, decided_at="2026-10-22")
        self.assertEqual(self.errs(), [])
        self.status(status="adopted", confirm_at=None, decided_at="2026-10-22", case_dir="c",
                    claims={"final_generalization": False, "real_pilot_confirmed": False, "adopted": True})
        self.assertTrue(any("case_dir" in e for e in self.errs()))
        c = self.campaign / "cases" / "dev" / "c"
        c.mkdir(parents=True)
        (c / "case.json").write_text("{}")
        self.assertEqual(self.errs(), [])  # 構造のみ。成立の受入ではない

    def test_adopted_claim_mismatch(self):
        self.status(claims={"final_generalization": False, "real_pilot_confirmed": False, "adopted": True})
        self.assertTrue(any("claims.adopted" in e for e in self.errs()))

    def test_generalization_claim_rejected(self):
        self.status(claims={"final_generalization": True, "real_pilot_confirmed": False, "adopted": False})
        self.assertTrue(any("final_generalization" in e for e in self.errs()))

    def test_bad_state(self):
        self.status(status="maybe")
        self.assertTrue(self.errs())
        self.status(label="final")
        self.assertTrue(self.errs())
        self.status(schema="celeris.short-ml-case-status/1")
        self.assertTrue(self.errs())
        self.path.write_text("{not json")
        self.assertTrue(self.errs())

    def test_external_missing_or_invalid_not_swallowed(self):
        self.status()
        (self.dir / "re.json").unlink()
        self.assertTrue(any("reassessment" in e for e in self.errs()))
        (self.dir / "re.json").write_text("garbage")
        self.assertTrue(self.errs())
        (self.dir / "re.json").write_text(json.dumps({"withdrawn_decisions": []}))
        self.assertTrue(any("withdrawn_decisions" in e for e in self.errs()))
        (self.dir / "re.json").write_text(json.dumps(
            {"withdrawn_decisions": [{"saved": {"status_json": {"reasons": ["x"]}}}]}))
        self.assertTrue(any("not preserved" in e for e in self.errs()))

    def test_review_without_history_or_missing(self):
        self.status()
        (self.dir / "rv.md").write_text("nothing")
        self.assertTrue(self.errs())
        (self.dir / "rv.md").unlink()
        self.assertTrue(self.errs())

    def test_refs_must_be_absolute(self):
        self.status(external_refs={"reassessment": "re.json", "review": "rv.md"})
        self.assertTrue(self.errs())

    def test_stray_dir_rejected_unless_adopted(self):
        self.status()
        (self.campaign / "cases" / "dev" / "short-ml-campaign").mkdir(parents=True)
        self.assertTrue(self.errs())


class Placement(Base):
    def test_clean(self):
        self.status()
        self.assertEqual(self.placement(), [])

    def test_judgment_fields_remain(self):
        for k in ("reasons", "inventory_evidence", "resume_conditions", "unverified", "pins"):
            self.status(**{k: ["x"]})
            self.assertTrue(any(k in e for e in self.placement()), k)

    def test_nested_judgment_field(self):
        self.status(external_refs={"reassessment": "a", "review": "b", "reasons": []})
        self.assertTrue(any("reasons" in e for e in self.placement()))

    def test_key_not_allowed_for_status(self):
        self.status(decided_at="2026-10-22")  # pending に decided_at は不可
        self.assertTrue(any("decided_at" in e for e in self.placement()))


class Cli(unittest.TestCase):
    def test_repo_check_and_placement_pass(self):
        for flag in ("--check", "--placement-check"):
            r = run(flag)
            self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("not an adoption pass", run("--check").stdout)

    def test_old_cli_usage_exit_codes(self):
        for args in ([], ["--check", "x"], ["--bogus"], ["--acceptance"]):
            r = run(*args)
            self.assertEqual(r.returncode, 2, args)
            self.assertIn("usage", r.stderr)

    def test_repo_status_is_pending_without_judgment(self):
        s = json.loads(vsmc.STATUS_PATH.read_text())
        self.assertEqual(s["status"], "pending")
        self.assertEqual(s["confirm_at"], "2026-10-22T00:00:00Z")
        self.assertFalse(any(s["claims"].values()))
        self.assertFalse(set(vsmc.walk_keys(s)) & vsmc.JUDGMENT_KEYS)


if __name__ == "__main__":
    unittest.main()
