"""Release provenance handling for the browser conformance ledger."""

import importlib.util
import json
import subprocess
import sys
from pathlib import Path
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "browser-conformance.py"
SPEC = importlib.util.spec_from_file_location("browser_conformance", SCRIPT)
browser_conformance = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(browser_conformance)


class BrowserConformanceReleaseTests(unittest.TestCase):
    def test_release_provenance_matches_worker_shape_and_has_rfc3339_time(self):
        with tempfile.TemporaryDirectory() as directory:
            provenance = browser_conformance.release_provenance(
                "0123456789ab", "a" * 40
            )
            path = browser_conformance.write_ledger(
                directory, [], generated_for=provenance
            )
            ledger = json.loads(path.read_text())

        self.assertEqual(ledger["schema"], 1)
        self.assertEqual(ledger["generated_for"], {
            "celeris_release": "0123456789ab",
            "celeris_sha": "a" * 40,
            "agent_browser": "0.38.1",
            "fixture": "p4c-local-v1",
            "generated_at": provenance["generated_at"],
        })
        self.assertRegex(provenance["generated_at"], r"^\d{4}-\d\d-\d\dT.*Z$")

    def test_generated_at_is_current_utc_rfc3339(self):
        generated_at = browser_conformance.release_provenance(
            "0123456789ab", generated_at="2026-10-08T12:34:56Z"
        )["generated_at"]
        self.assertEqual(generated_at, "2026-10-08T12:34:56Z")

    def test_p4b_update_preserves_existing_generated_for(self):
        generated_for = {
            "celeris_release": "0123456789ab",
            "celeris_sha": "a" * 40,
            "agent_browser": "0.38.1",
            "fixture": "p4c-local-v1",
            "generated_at": "2026-10-08T12:34:56Z",
        }
        results = [{"backend_id": "claude-code", "version": "0.38.1", "passed": []}]
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "source.json"
            output = Path(directory) / "out"
            source.write_text(json.dumps({
                "schema": 1,
                "source": "celeris-browser-conformance",
                "generated_for": generated_for,
                "results": results,
            }))
            # Patch the expensive measurement commands; this exercises the actual P4-B
            # ledger update path without starting cargo or an agent-browser process.
            original_run = browser_conformance.subprocess.run
            class Completed:
                returncode = 0
                stdout = "test production_h3_injects_once_without_exposure ... ok\n" \
                         "test injected_leak_is_caught_by_the_same_scanner ... ok\n"
                stderr = "\n".join(f"ATTACK-{mark}-OK" for mark in browser_conformance.P4B_ATTACK_MARKS)
            browser_conformance.subprocess.run = lambda *_args, **_kwargs: Completed()
            try:
                self.assertEqual(browser_conformance.p4b_evidence(
                    source, {"claude-code"}, output
                ), 0)
            finally:
                browser_conformance.subprocess.run = original_run
            updated = json.loads((output / "conformance.json").read_text())

        self.assertEqual(updated["generated_for"], generated_for)
        self.assertIn("injection_attack_suite", updated["results"][0]["passed"])


    def test_runner_serves_shim_actions_for_the_loopback_origin(self):
        """The shim only reaches agent-browser through its action socket and compares full
        origins; the runner must provide both or every case fails (no events but blocks)."""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            log = root / "argv.log"
            fake = root / "agent-browser"
            fake.write_text("#!/bin/sh\n"
                            f"printf '%s\\n' \"$*\" >> {log}\n"
                            "echo '{\"success\":true,\"data\":{}}'\n")
            fake.chmod(0o755)
            socket_dir = root / "sock"
            socket_dir.mkdir()
            runtime, _output, cli = browser_conformance.prepare_runtime(
                "acp", fake, "http://localhost:43210/", root, socket_dir)
            listener = browser_conformance.serve_actions(runtime, socket_dir / "a.sock")
            try:
                def shim(*args):
                    return subprocess.run([sys.executable, str(cli), *args], capture_output=True,
                                          text=True, timeout=30, check=False).returncode
                self.assertEqual(shim("open", "http://localhost:43210/"), 0)
                self.assertNotEqual(shim("open", "http://denied.invalid/"), 0)
                self.assertNotEqual(shim("open", "https://localhost/"), 0)
                self.assertEqual(shim("click", "@e1"), 0)
            finally:
                listener.close()
            events = [json.loads(line) for line in (runtime / "events.jsonl").read_text().splitlines()]
            calls = log.read_text().splitlines()

        self.assertEqual([e["operation"] for e in events],
                         ["navigate", "policy_block", "policy_block", "click"])
        self.assertEqual(len(calls), 2)
        self.assertIn("--allowed-domains localhost ", calls[0])
        self.assertTrue(calls[0].endswith("--json open http://localhost:43210/"))
        self.assertTrue(calls[1].endswith("--json click @e1"))


if __name__ == "__main__":
    unittest.main()
