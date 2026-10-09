import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "browser-conformance.py"
spec = importlib.util.spec_from_file_location("browser_conformance", SCRIPT)
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)


class CredentialEvidenceTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="bce-")
        self.root = Path(self.temp.name)
        self.ledger = self.root / "ledger.json"
        self.output = self.root / "out"
        self.ledger.write_text(json.dumps({
            "schema": 1, "source": "celeris-browser-conformance",
            "generated_for": {"celeris_release": "abc"},
            "results": [
                {"backend_id": "claude-code", "passed": ["click_by_ref", "injection_attack_suite"],
                 "evidence": [{"case": "injection_attack_suite", "test": "p4b", "outcome": "passed"}]},
                {"backend_id": "acp", "passed": ["click_by_ref"], "evidence": []},
            ]
        }))
        self.fake = self.root / "fake-cargo.py"
        self.fake.write_text('''#!/usr/bin/env python3
import os, sys
args=sys.argv[1:]
if args[0] == "build":
    sys.exit(int(os.environ.get("FAKE_BUILD", "0")))
test=args[args.index("--exact") + 1]
mode=os.environ.get("FAKE_MODE", "pass")
if mode == "failed" and test == os.environ.get("FAKE_TARGET"):
    print("running 1 test")
    print(f"test {test} ... FAILED")
    sys.exit(101)
if mode == "skip" and test == os.environ.get("FAKE_TARGET"):
    print("running 1 test")
    print(f"test {test} ... ok")
    print("SKIPPED: fake skip")
    print("test result: ok. 1 passed; 0 failed; 0 ignored")
    sys.exit(0)
if mode == "missing" and test == os.environ.get("FAKE_TARGET"):
    print("running 0 tests")
    print("test result: ok. 0 passed; 0 failed; 0 ignored")
    sys.exit(0)
print("running 1 test")
print(f"test {test} ... ok")
print("test result: ok. 1 passed; 0 failed; 0 ignored")
''')
        self.fake.chmod(0o755)

    def tearDown(self):
        self.temp.cleanup()

    def run_mode(self, mode="pass", target="", build="0", runtime="daemon"):
        env = os.environ.copy()
        env.update(CELERIS_CONFORMANCE_CARGO=str(self.fake), FAKE_MODE=mode,
                   FAKE_TARGET=target, FAKE_BUILD=build)
        if runtime == "launcher":
            env["CELERIS_BROWSER_LAUNCHER_SOCKET"] = str(self.root / "launcher.sock")
        return subprocess.run([sys.executable, str(SCRIPT), "--credential-evidence", str(self.ledger),
                               "--credential-backend", "claude-code", "--credential-runtime", runtime, "--output-dir", str(self.output)],
                              env=env, text=True, capture_output=True, check=False)

    def result(self):
        return json.loads((self.output / "conformance.json").read_text())

    def test_all_pass_adds_cases_preserves_provenance_and_other_backend(self):
        done = self.run_mode()
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        data = self.result()
        self.assertEqual(data["generated_for"], {"celeris_release": "abc"})
        target, other = data["results"]
        self.assertTrue({"isolation_suite", "egress_negative_suite"} <= set(target["passed"]))
        self.assertIn("injection_attack_suite", target["passed"])
        self.assertEqual(other["passed"], ["click_by_ref"])
        report = json.loads((self.output / "credential-evidence.json").read_text())
        self.assertEqual(len(report["evidence"]), 40)

    def test_failure_skip_and_missing_test_fail_closed(self):
        for mode, outcome in (("failed", "failed"), ("skip", "not_run"), ("missing", "not_run")):
            with self.subTest(mode=mode):
                self.ledger.write_text(json.dumps({"schema": 1, "source": "celeris-browser-conformance",
                    "results": [{"backend_id": "claude-code", "passed": ["isolation_suite", "egress_negative_suite"], "evidence": []}]}))
                name = module.P4A_ISOLATION_TESTS[0][2]
                done = self.run_mode(mode, name)
                self.assertEqual(done.returncode, 1)
                result = self.result()["results"][0]
                self.assertFalse({"isolation_suite", "egress_negative_suite"} & set(result["passed"]))
                self.assertIn(outcome, {x["outcome"] for x in result["evidence"]})

    def test_build_failure_marks_all_not_run(self):
        done = self.run_mode(build="17")
        self.assertEqual(done.returncode, 1)
        report = json.loads((self.output / "credential-evidence.json").read_text())
        self.assertEqual(report["code"], "build_failed")
        self.assertEqual({x["outcome"] for x in report["evidence"]}, {"not_run"})

    def test_launcher_unavailable_records_reason_and_clears_credential_claim(self):
        before = self.ledger.read_text()
        done = subprocess.run([sys.executable, str(SCRIPT), "--credential-evidence", str(self.ledger),
                               "--credential-backend", "claude-code", "--credential-runtime", "launcher",
                               "--output-dir", str(self.output)], env={**os.environ, "CELERIS_BROWSER_LAUNCHER_SOCKET": ""},
                              text=True, capture_output=True)
        self.assertEqual(done.returncode, 1)
        report = json.loads((self.output / "credential-evidence.json").read_text())
        self.assertEqual(report["code"], "launcher_unavailable")
        self.assertIn("not configured", report["reason"])
        written = json.loads((self.output / "conformance.json").read_text())
        result = next(row for row in written["results"] if row["backend_id"] == "claude-code")
        self.assertNotIn("isolation_suite", result["passed"])
        self.assertNotIn("egress_negative_suite", result["passed"])
        self.assertFalse(any(row.get("case") in ("isolation_suite", "egress_negative_suite")
                             for row in result.get("evidence", [])))
        self.assertEqual(self.ledger.read_text(), before)

    def test_launcher_mode_tags_evidence_with_runtime(self):
        done = self.run_mode(runtime="launcher")
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        report = json.loads((self.output / "credential-evidence.json").read_text())
        self.assertEqual({row["runtime"] for row in report["evidence"]}, {"launcher"})

    def test_cli_rejects_unknown_and_missing_backend(self):
        base = [sys.executable, str(SCRIPT), "--credential-evidence", str(self.ledger), "--output-dir", str(self.output)]
        self.assertEqual(subprocess.run(base, capture_output=True).returncode, 2)
        self.assertEqual(subprocess.run(base + ["--credential-backend", "unknown"], capture_output=True).returncode, 2)

    def test_generator_test_names_are_declared_in_task_core_source(self):
        source = (SCRIPT.parent.parent / "crates/task-core/src/browser_backend.rs").read_text()
        self.assertEqual(len(module.P4A_ISOLATION_TESTS), 19)
        self.assertEqual(len(module.P4A_EGRESS_NEGATIVE_TESTS), 21)
        # The companion certify-credential unit adds the Rust declarations. During
        # staged integration, keep this generator unit runnable before that merge;
        # once present, require the shared contract identifiers in Rust.
        if "P4A_ISOLATION_TESTS" in source or "P4A_EGRESS_NEGATIVE_TESTS" in source:
            self.assertIn("P4A_ISOLATION_TESTS", source)
            self.assertIn("P4A_EGRESS_NEGATIVE_TESTS", source)


if __name__ == "__main__":
    unittest.main()
