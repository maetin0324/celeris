"""jev-live-evidence.py --check-approval-run-dir: 証跡未取得は非 0 + 不足一覧、揃えば 0（live の判定はしない）。"""
import json
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "..", "..", "jev-live-evidence.py")
SHA = "a" * 64
MID = "jev-b1c1-m1"


def put(d, name, text):
    p = os.path.join(d, name)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "w", encoding="utf-8") as f:
        f.write(text)


def full(d, approval=None):
    put(d, "launcher.meta", f"starttime=1\nconfig=/m/measure.toml\nrelease=abc\nexe=/m/celeris\nexe_sha256={SHA}\n")
    put(d, "config-identity.json", json.dumps({"identity": {"pid": 1, "release": "abc"}}))
    put(d, "daemon-image.sha256", f"{SHA}  /proc/1/exe\n")
    put(d, "state-dir-db.txt", "/local/celeris/data/measure/state/celeris.db\n")
    for f in ["records.jsonl", "summary.json", "provenance.json", "routing_events.jsonl"]:
        put(d, f"export/{f}", "{}\n")
    put(d, "operator-approval.md", approval or f"measurement_id: {MID}\napproved_by: op\napproved_at: 2026-10-19T00:00:00Z\ndecision: live-approved\n")


def run(d):
    p = subprocess.run([sys.executable, SCRIPT, "--check-approval-run-dir", d, "--measurement-id", MID], capture_output=True, text=True)
    return p.returncode, json.loads(p.stdout), p.stderr


class ApprovalEvidence(unittest.TestCase):
    def setUp(self):
        self.t = tempfile.TemporaryDirectory()
        self.addCleanup(self.t.cleanup)
        self.d = os.path.join(self.t.name, "run")

    def test_absent_run_dir_is_nonzero_with_missing_list(self):
        code, rep, err = run(self.d)
        self.assertEqual(code, 1)
        self.assertTrue(rep["missing"])
        self.assertIn("live 未確認", err)

    def test_empty_dir_lists_every_missing_item(self):
        os.makedirs(self.d)
        code, rep, _ = run(self.d)
        self.assertEqual(code, 1)
        names = " ".join(rep["missing"])
        for n in ("launcher.meta", "config-identity.json", "daemon-image.sha256", "state-dir-db.txt", "export/", "operator-approval.md"):
            self.assertIn(n, names)

    def test_complete_evidence_exits_zero_without_claiming_live(self):
        full(self.d)
        code, rep, err = run(self.d)
        self.assertEqual((code, rep["missing"]), (0, []))
        self.assertNotIn("live 合格", err)

    def test_unapproved_or_mismatched_or_inconsistent_is_nonzero(self):
        full(self.d, f"measurement_id: {MID}\napproved_by: op\napproved_at: t\ndecision: pending\n")
        self.assertEqual(run(self.d)[0], 1)
        full(self.d, f"measurement_id: other\napproved_by: op\napproved_at: t\ndecision: live-approved\n")
        self.assertEqual(run(self.d)[0], 1)
        full(self.d)
        put(self.d, "daemon-image.sha256", "b" * 64 + "\n")
        code, rep, _ = run(self.d)
        self.assertEqual(code, 1)
        self.assertTrue(any("一致しない" in m for m in rep["missing"]))


if __name__ == "__main__":
    unittest.main()
