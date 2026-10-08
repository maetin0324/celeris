"""Release provenance handling for the browser conformance ledger."""

import importlib.util
import json
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


if __name__ == "__main__":
    unittest.main()
