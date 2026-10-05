"""Standard-library-only contract tests for the RouteLLM wrapper."""

import json
from pathlib import Path
import re
import signal
import socket
import subprocess
import sys
import unittest
from urllib.error import HTTPError
from urllib.request import Request, urlopen


HERE = Path(__file__).resolve().parent
FIXTURES = HERE.parents[1] / "crates/task-core/tests/fixtures/estimator_sidecar_v1"


def fixture(name):
    return json.loads((FIXTURES / name).read_text())


class SidecarTest(unittest.TestCase):
    def setUp(self):
        self.process = subprocess.Popen(
            [sys.executable, str(HERE / "routellm_sidecar.py"),
             "--host", "127.0.0.1", "--port", "0", "--router", "bert",
             "--weights-dir", "/missing-weights-fake-only",
             "--strong", "model-a", "--weak", "model-b", "--fake-classifier"],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        )
        ready = self.process.stdout.readline().strip()
        match = re.fullmatch(r"READY port=(\d+)", ready)
        self.assertIsNotNone(match, ready)
        self.port = int(match.group(1))
        self.base = f"http://127.0.0.1:{self.port}"

    def tearDown(self):
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGTERM)
        self.assertEqual(self.process.wait(timeout=5), 0, self.process.stderr.read())
        self.process.stdout.close()
        self.process.stderr.close()
        with socket.socket() as sock:
            sock.settimeout(1)
            self.assertNotEqual(sock.connect_ex(("127.0.0.1", self.port)), 0)

    def post(self, payload):
        request = Request(self.base + "/estimate", json.dumps(payload).encode(),
                          {"Content-Type": "application/json"})
        with urlopen(request, timeout=2) as response:
            return json.load(response)

    def test_health_estimate_and_unknown_candidate(self):
        with urlopen(self.base + "/healthz", timeout=2) as response:
            health = json.load(response)
        self.assertEqual(health["status"], "ready")
        self.assertTrue(health["needs_prompt"])
        self.assertEqual(health["protocol_version"], 1)
        request = fixture("request_valid.json")
        request["optional_prompt"] = "A sample routing prompt"
        request["candidates"].append({"model_profile_id": "model-c"})
        response = self.post(request)
        self.assertEqual(response["request_id"], request["request_id"])
        self.assertEqual(response["dependencies"], fixture("descriptor_valid.json")["dependencies"])
        self.assertEqual([item["model_profile_id"] for item in response["estimates"]],
                         [item["model_profile_id"] for item in request["candidates"]])
        self.assertTrue(all(item["index"] is None for item in response["estimates"]))
        self.assertIn("raw_pair_win_rate=0.75", response["estimates"][0]["reasons"])
        self.assertIn("outside_configured_pair", response["estimates"][2]["reasons"])

    def test_missing_prompt_and_pair_are_unknown(self):
        response = self.post(fixture("request_valid.json"))
        self.assertTrue(all("prompt_required" in item["reasons"] for item in response["estimates"]))
        request = fixture("request_valid.json")
        request["optional_prompt"] = "sample"
        request["candidates"] = request["candidates"][:1]
        response = self.post(request)
        self.assertIn("pair_incomplete", response["estimates"][0]["reasons"])

    def test_duplicate_candidate_is_rejected(self):
        request = fixture("request_valid.json")
        request["candidates"].append(request["candidates"][0])
        with self.assertRaises(HTTPError) as error:
            self.post(request)
        self.assertEqual(error.exception.code, 400)


if __name__ == "__main__":
    unittest.main()
