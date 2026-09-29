#!/usr/bin/env python3
"""P4-C local fixture runner for real agent-browser 0.38.1.

The LLM driver may be scripted; the browser substrate is always the actual executable.
No fixture page contains external resources. The runner writes a ledger only after invoking
each named backend process and checking its browser events and fixture server observations.
"""

import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import time

BACKENDS = ("acp", "claude-code", "browser-specialist")
CASES = (
    "open_allowed_origin", "refuse_denied_origin", "resume_after_crash",
    "snapshot_has_refs", "click_by_ref", "screenshot_artifact",
    "download_to_artifacts",
)


class Site(BaseHTTPRequestHandler):
    requests = []

    def log_message(self, *_args):
        pass

    def do_GET(self):
        self.requests.append(("GET", self.path))
        if self.path == "/download":
            body = b"celeris local fixture\n"
            kind = "application/octet-stream"
        elif self.path == "/":
            body = (b"<!doctype html><html><body>"
                    b"<button onclick=\"fetch('/clicked',{method:'POST'})\">Click fixture</button>"
                    b"<a href='/download' download='fixture.bin'>Download fixture</a>"
                    b"</body></html>")
            kind = "text/html; charset=utf-8"
        else:
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        self.requests.append(("POST", self.path))
        self.send_response(204 if self.path == "/clicked" else 404)
        self.end_headers()


def call(cli, *args):
    result = subprocess.run([sys.executable, str(cli), *args], text=True,
                            capture_output=True, timeout=60, check=False)
    return result.returncode, result.stdout


def drive():
    cli = Path(os.environ["CELERIS_BROWSER_CLI"])
    phase = os.environ["CELERIS_BROWSER_PHASE"]
    if phase == "open":
        opened = call(cli, "open", os.environ["CELERIS_BROWSER_ORIGIN"])
        denied = call(cli, "open", "http://denied.invalid/")
        if opened[0] == 0 and denied[0] != 0:
            os.kill(os.getpid(), signal.SIGKILL)  # simulated harness crash after real browser use
        return 1
    if phase != "resume":
        return 2
    code, snapshot = call(cli, "snapshot")
    (cli.parent / "snapshot-output.txt").write_text(snapshot)
    if code:
        return 1
    refs = ["@" + ref for ref in dict.fromkeys(re.findall(r"\bref=(e[0-9]+)\b", snapshot))]
    if not refs:
        return 1
    if call(cli, "click", refs[0])[0]:
        return 1
    if call(cli, "screenshot")[0]:
        return 1
    if len(refs) < 2 or call(cli, "download", refs[1])[0]:
        return 1
    return 0 if call(cli, "close")[0] == 0 else 1


def backend_run(backend, command, agent_browser, origin, root):
    first_request = len(Site.requests)
    runtime = root / backend
    runtime.mkdir(mode=0o700)
    output = runtime / "output"
    output.mkdir(mode=0o700)
    cli = runtime / "celeris-browser.py"
    shutil.copyfile(Path(__file__).resolve().parents[1] / "crates/task-worker/src/browser_cli.py", cli)
    policy = {"default": "deny", "allow": ["launch", "close", "navigate", "snapshot",
                                           "click", "screenshot", "download"]}
    encoded = json.dumps(policy, separators=(",", ":")).encode()
    (runtime / "policy.json").write_bytes(encoded)
    (runtime / "upstream.json").write_text('{"idleTimeout":"5m","noWebmcp":true}')
    (runtime / "config.json").write_text(json.dumps({
        "executable": str(agent_browser), "session_id": f"celeris-p4c-{backend}",
        "allowed_domains": ["localhost"], "output": str(output),
        "policy_sha256": hashlib.sha256(encoded).hexdigest(),
        "credential_policy_ids": [], "credential_use": False,
    }))
    env = os.environ.copy()
    env.update(CELERIS_BROWSER_CLI=str(cli), CELERIS_BROWSER_ORIGIN=origin,
               HTTP_PROXY="", HTTPS_PROXY="", ALL_PROXY="", NO_PROXY="localhost,127.0.0.1")
    start = time.monotonic()
    process_ok = []
    for phase in ("open", "resume"):
        env["CELERIS_BROWSER_PHASE"] = phase
        result = subprocess.run(command, cwd=runtime, env=env, text=True,
                                capture_output=True, timeout=180, check=False)
        process_ok.append(result.returncode == (-signal.SIGKILL if phase == "open" else 0))
        (runtime / f"{phase}-driver.json").write_text(json.dumps({
            "exit": result.returncode, "stdout_tail": result.stdout[-2000:],
            "stderr_tail": result.stderr[-2000:],
        }))
        if not process_ok[-1]:
            break
    events = []
    event_file = runtime / "events.jsonl"
    if event_file.exists():
        for line in event_file.read_text().splitlines():
            try:
                events.append(json.loads(line))
            except ValueError:
                pass
    def event(operation, status):
        return any(e.get("operation") == operation and e.get("status") == status for e in events)
    requests = list(Site.requests[first_request:])
    passed = set()
    if process_ok and process_ok[0] and ("GET", "/") in requests and event("navigate", "success"):
        passed.add("open_allowed_origin")
    if event("policy_block", "blocked"):
        passed.add("refuse_denied_origin")
    if len(process_ok) == 2 and all(process_ok) and event("extract", "success"):
        passed.add("resume_after_crash")
    snapshot = runtime / "snapshot-output.txt"
    if snapshot.exists() and re.search(r"\bref=e[0-9]+\b", snapshot.read_text()) and event("extract", "success"):
        passed.add("snapshot_has_refs")
    if event("click", "success") and ("POST", "/clicked") in requests:
        passed.add("click_by_ref")
    if event("screenshot", "success") and any(output.glob("screenshot-*.png")):
        passed.add("screenshot_artifact")
    if event("download", "success") and any(output.glob("download-*.bin")):
        passed.add("download_to_artifacts")
    return {"backend_id": backend, "version": "0.38.1", "passed": sorted(passed)}, {
        "backend_id": backend, "task_fixture": "p4c-local-v1", "accepted": len(passed) == len(CASES),
        "policy_violations": 0, "recoveries": 1 if len(process_ok) == 2 and all(process_ok) else 0,
        "cost_usd": 0.0, "wall_secs": round(time.monotonic() - start),
        "observed_cases": sorted(passed), "driver_exits": process_ok,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--agent-browser", default="agent-browser")
    parser.add_argument("--output-dir", required=True)
    parser.add_argument("--backend-command", action="append", default=[],
                        help='ID=["command","arg",...] (one per backend)')
    parser.add_argument("--scripted", action="store_true",
                        help="use this file as a scripted LLM driver for all three backends")
    parser.add_argument("--drive", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.drive:
        return drive()
    command = {}
    for item in args.backend_command:
        backend, sep, argv = item.partition("=")
        if not sep or backend not in BACKENDS or backend in command:
            parser.error("each backend command must have a unique known ID")
        parsed = json.loads(argv)
        if not isinstance(parsed, list) or not parsed or not all(isinstance(a, str) for a in parsed):
            parser.error("backend command must be a nonempty JSON string array")
        command[backend] = parsed
    if args.scripted and command:
        parser.error("choose scripted or explicit backend commands")
    if args.scripted:
        command = {backend: [sys.executable, str(Path(__file__).resolve()),
                             "--drive", "--output-dir", args.output_dir] for backend in BACKENDS}
    if set(command) != set(BACKENDS):
        parser.error("ACP, claude-code and browser-specialist commands are all required")
    executable = shutil.which(args.agent_browser)
    if executable is None:
        print("agent-browser executable unavailable", file=sys.stderr)
        return 2
    version = subprocess.run([executable, "--version"], text=True, capture_output=True,
                             timeout=10, check=False)
    if version.returncode or version.stdout.strip() != "agent-browser 0.38.1":
        print("agent-browser 0.38.1 is required", file=sys.stderr)
        return 2
    output = Path(args.output_dir).resolve()
    output.mkdir(parents=True, exist_ok=True)
    Site.requests = []
    server = ThreadingHTTPServer(("127.0.0.1", 0), Site)
    from threading import Thread
    thread = Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        origin = f"http://localhost:{server.server_port}/"
        results, comparison = [], []
        for backend in BACKENDS:
            result, same_task = backend_run(backend, command[backend], executable, origin, output)
            results.append(result)
            comparison.append(same_task)
        # Scripted mode measures the real substrate but does not exercise ACP/Claude harness
        # protocols. The worker deliberately refuses this ledger as backend certification.
        source = "celeris-browser-conformance-scripted" if args.scripted else "celeris-browser-conformance"
        ledger = {"schema": 1, "source": source, "results": results}
        temporary = output / "conformance.json.next"
        temporary.write_text(json.dumps(ledger, indent=2) + "\n")
        temporary.replace(output / "conformance.json")
        (output / "same-task.json").write_text(json.dumps(comparison, indent=2) + "\n")
        print(json.dumps({"record": str(output / "conformance.json"), "comparison": comparison}))
        return 0 if all(run["accepted"] for run in comparison) else 1
    finally:
        server.shutdown()
        thread.join(timeout=5)


if __name__ == "__main__":
    sys.exit(main())
