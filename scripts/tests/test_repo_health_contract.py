"""repo health 契約（report-v1）: 検証器は examples を全件通し、bad-examples を 1 件ずつ落とす。"""
from pathlib import Path
import json
import subprocess
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
TOOL = ROOT / "scripts/dev/repo-health/validate_report.py"
SCHEMA_DIR = ROOT / "scripts/dev/repo-health/schema"
STATUSES = {"pass", "fail", "error", "unknown", "not_applicable", "unsupported"}


def run(*paths: Path) -> subprocess.CompletedProcess:
    return subprocess.run([sys.executable, "-I", str(TOOL), *map(str, paths)], capture_output=True, text=True)


class RepoHealthContractTests(unittest.TestCase):
    def test_examples_all_valid(self):
        examples = sorted((SCHEMA_DIR / "examples").glob("*.json"))
        self.assertGreaterEqual(len(examples), 8)
        proc = run(*examples)
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)

    def test_bad_examples_each_invalid_with_location(self):
        bad = sorted((SCHEMA_DIR / "bad-examples").glob("*.json"))
        self.assertGreaterEqual(len(bad), 10)
        for path in bad:
            proc = run(path)
            self.assertEqual(proc.returncode, 1, f"{path.name} は exit 1 のはず\n{proc.stdout}")
            self.assertIn(f"{path}:/", proc.stdout, f"{path.name}: 違反の場所（JSON pointer）が無い")
        # 妥当な 1 件と不正な 1 件を混ぜても exit 1。
        mixed = run(SCHEMA_DIR / "examples" / "pass.json", bad[0])
        self.assertEqual(mixed.returncode, 1)
        self.assertIn("1 valid, 1 invalid", mixed.stdout)

    def test_examples_cover_contract_cases(self):
        seen_status, modes, shallow = set(), set(), False
        for path in (SCHEMA_DIR / "examples").glob("*.json"):
            rep = json.loads(path.read_text(encoding="utf-8"))
            seen_status |= {r["status"] for r in rep["rules"]}
            modes.add(rep["inputs"]["mode"])
            shallow |= rep["snapshot"]["shallow"] and not rep["snapshot"]["base"]["reachable"]
        self.assertEqual(seen_status, STATUSES)
        self.assertEqual(modes, {"full", "doc-only"})
        self.assertTrue(shallow, "shallow（base に届かない）の例が無い")

    def test_usage_error_and_unreadable_file(self):
        self.assertEqual(run().returncode, 1)
        proc = run(SCHEMA_DIR / "no-such-report.json")
        self.assertEqual(proc.returncode, 1)
        self.assertIn("読めない", proc.stdout)


if __name__ == "__main__":
    unittest.main()
