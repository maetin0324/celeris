"""ops-r2-poll.sh・ops-r2-human-span.sh の offline 試験。測定 daemon は 127.0.0.1 の一時 HTTP server、
celeris-campaign は引数を書き出す stub（network・LLM・daemon なし）。session-map と取り込みの試験は
crates/campaign/tests/ops_r2_session.rs。"""
import http.server
import json
import os
import shutil
import stat
import subprocess
import tempfile
import threading
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SHELL = shutil.which("dash") or "/bin/sh"


class Daemon(http.server.BaseHTTPRequestHandler):
    statuses = []
    seen_auth = []

    def do_GET(self):  # noqa: N802
        Daemon.seen_auth.append(self.headers.get("Authorization"))
        if not self.path.startswith("/api/v1/tasks/T1") or not Daemon.statuses:
            self.send_response(404)
            self.end_headers()
            return
        body = json.dumps({"id": "T1", "status": Daemon.statuses.pop(0)}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *a):
        pass


class OpsR2SessionScripts(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = self._tmp.name
        self.log = os.path.join(self.root, "calls.jsonl")
        self.bin = os.path.join(self.root, "celeris-campaign")
        with open(self.bin, "w") as f:
            f.write("#!/usr/bin/env python3\nimport json, sys\n"
                    f"open({self.log!r}, 'a').write(json.dumps(sys.argv[1:]) + '\\n')\n")
        os.chmod(self.bin, stat.S_IRWXU)
        self.token = os.path.join(self.root, "token")
        with open(self.token, "w") as f:
            f.write("op-token\n")
        self.server = http.server.HTTPServer(("127.0.0.1", 0), Daemon)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        Daemon.seen_auth = []

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self._tmp.cleanup()

    def calls(self):
        if not os.path.exists(self.log):
            return []
        with open(self.log) as f:
            return [json.loads(l) for l in f]

    def poll(self, *extra):
        api = f"http://127.0.0.1:{self.server.server_port}"
        return subprocess.run([SHELL, os.path.join(HERE, "ops-r2-poll.sh"), "--api", api, "--token-file", self.token,
                               "--task", "T1", "--human-ops", os.path.join(self.root, "h.jsonl"),
                               "--campaign-bin", self.bin, "--interval", "0", *extra,
                               "--", "--campaign", "c1", "--condition", "B-direct"],
                              capture_output=True, text=True, check=False)

    def test_poll_records_each_status_check_and_stops_at_terminal(self):
        Daemon.statuses = ["running", "running", "done"]
        p = self.poll()
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertEqual(json.loads(p.stdout), {"stop": "terminal", "status": "done", "polls": 3})
        calls = self.calls()
        self.assertEqual(len(calls), 3)
        for c in calls:
            self.assertEqual(c[:2], ["human-op", "record"])
            for k, v in [("--op-kind", "status_check"), ("--actor-kind", "script"), ("--mode", "automatic"),
                         ("--condition", "B-direct")]:
                self.assertEqual(c[c.index(k) + 1], v)
        self.assertEqual(calls[-1][calls[-1].index("--note") + 1], "status=done")
        self.assertEqual(Daemon.seen_auth, ["Bearer op-token"] * 3)
        self.assertFalse(any("op-token" in a for c in calls for a in c))

    def test_poll_stops_for_attention_and_limits(self):
        Daemon.statuses = ["blocked"]
        self.assertEqual(self.poll().returncode, 10)
        Daemon.statuses = ["running", "running"]
        p = self.poll("--max-polls", "2")
        self.assertEqual(p.returncode, 12)
        p = self.poll("--cutoff", "2000-01-01T00:00:00Z")
        self.assertEqual((p.returncode, json.loads(p.stdout)), (11, {"stop": "cutoff"}))
        Daemon.statuses = []
        self.assertEqual(self.poll().returncode, 2)

    def span(self, *args, now):
        env = dict(os.environ, CELERIS_HUMAN_OPS_NOW=now)
        return subprocess.run([SHELL, os.path.join(HERE, "ops-r2-human-span.sh"), *args],
                              capture_output=True, text=True, check=False, env=env)

    def test_human_span_records_one_interval_with_both_ends(self):
        state = os.path.join(self.root, "spans")
        common = ["--state", state, "--logical-op", "approve-1"]
        end = ["end", *common, "--campaign-bin", self.bin, "--file", "h.jsonl", "--actor", "human:rmaeda",
               "--", "--op-kind", "safety_approval", "--intent", "safety"]
        # begin の無い end は区間を作らない。
        self.assertNotEqual(self.span(*end, now="2026-10-30T00:05:00Z").returncode, 0)
        self.assertEqual(self.span("begin", *common, now="2026-10-30T00:01:00Z").returncode, 0)
        self.assertNotEqual(self.span("begin", *common, now="2026-10-30T00:02:00Z").returncode, 0)
        p = self.span(*end, now="2026-10-30T00:05:00Z")
        self.assertEqual(p.returncode, 0, p.stderr)
        (c,) = self.calls()
        for k, v in [("--actor-kind", "human"), ("--actor", "rmaeda"), ("--started-at", "2026-10-30T00:01:00Z"),
                     ("--ended-at", "2026-10-30T00:05:00Z"), ("--time-basis", "evaluator_live"),
                     ("--decided-by", "human:rmaeda"), ("--op-kind", "safety_approval")]:
            self.assertEqual(c[c.index(k) + 1], v)
        self.assertFalse(os.path.exists(os.path.join(state, "approve-1.start")))
        bad = ["end", *common, "--campaign-bin", self.bin, "--file", "h.jsonl", "--actor", "operator:x"]
        self.span("begin", *common, now="2026-10-30T00:06:00Z")
        self.assertNotEqual(self.span(*bad, now="2026-10-30T00:07:00Z").returncode, 0)


if __name__ == "__main__":
    unittest.main()
