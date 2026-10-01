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
    if phase in ("open", "fallback"):
        opened = call(cli, "open", os.environ["CELERIS_BROWSER_ORIGIN"])
        denied = call(cli, "open", "http://denied.invalid/")
        if phase == "fallback" and opened[0] == 0 and denied[0] != 0:
            phase = "resume"
        elif phase == "fallback":
            return 1
        else:
            if opened[0] == 0 and denied[0] != 0:
                os.kill(os.getpid(), signal.SIGKILL)
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


def backend_run(backend, command, agent_browser, origin, root, protocol_scripted=False):
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
               CELERIS_BROWSER_BACKEND=backend, CELERIS_BROWSER_RUNTIME=str(runtime),
               HTTP_PROXY="", HTTPS_PROXY="", ALL_PROXY="", NO_PROXY="localhost,127.0.0.1")
    start = time.monotonic()
    process_ok = []
    for phase in ("open", "resume"):
        env["CELERIS_BROWSER_PHASE"] = phase
        result = subprocess.run(command, cwd=runtime, env=env, text=True,
                                capture_output=True, timeout=180, check=False)
        expected = 0 if protocol_scripted else (-signal.SIGKILL if phase == "open" else 0)
        process_ok.append(result.returncode == expected)
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


# ADR-0112: P4-B trusted injection evidence. Mirrors task_core::browser_backend.
P4B_ATTACK_MARKS = (
    "A0", "A1", "A2", "A3", "A3b", "A4", "A5", "A6", "A7", "A7a", "A8", "A8n", "A9", "A9a",
    "A10", "A11", "A12", "A13", "A14", "A15", "A16", "A17",
)
P4B_H3_TESTS = (
    "production_h3_injects_once_without_exposure",
    "injected_leak_is_caught_by_the_same_scanner",
)


def p4b_evidence(ledger_path, backends, output):
    """Run the real attack matrix and H3 e2e, then add their per-test results to a measured
    ledger. The P4-B cases enter `passed` only when every required test passed; otherwise the
    evidence is kept with its failed/not_run outcome and the cases stay out (fail closed)."""
    manifest = Path(__file__).resolve().parents[1] / "Cargo.toml"
    ledger = json.loads(Path(ledger_path).read_text())
    if ledger.get("schema") != 1 or ledger.get("source") != "celeris-browser-conformance":
        print("ledger is not a measured celeris-browser-conformance record", file=sys.stderr)
        return 1
    runs = {}
    for name, cmd in (
        ("attacks", ["-p", "task-worker", "--test", "browser_injection_attacks",
                     "real_browser_injection_attack_matrix", "--", "--exact", "--nocapture"]),
        ("h3", ["-p", "task-api", "--test", "browser_h3_injection", "--", "--nocapture"]),
    ):
        done = subprocess.run(["cargo", "test", "--manifest-path", str(manifest), *cmd],
                              text=True, capture_output=True, timeout=1800, check=False)
        runs[name] = done
        (output / f"p4b-{name}.log").write_text(done.stdout + "\n" + done.stderr)
    attacks = runs["attacks"]
    evidence = []
    for mark in P4B_ATTACK_MARKS:
        ok = attacks.returncode == 0 and f"ATTACK-{mark}-OK" in attacks.stderr
        evidence.append({
            "case": "injection_attack_suite",
            "test": f"browser_injection_attacks::real_browser_injection_attack_matrix#{mark}",
            "outcome": "passed" if ok else ("failed" if attacks.returncode else "not_run"),
        })
    h3 = runs["h3"]
    for test in P4B_H3_TESTS:
        line = re.search(rf"^test {re.escape(test)} \.\.\. (ok|FAILED|ignored)$",
                         h3.stdout, re.MULTILINE)
        outcome = {"ok": "passed", "FAILED": "failed"}.get(line.group(1) if line else "", "not_run")
        evidence.append({"case": "auth_section_observation_stop",
                         "test": f"browser_h3_injection::{test}", "outcome": outcome})
    complete = all(e["outcome"] == "passed" for e in evidence)
    for result in ledger["results"]:
        if result["backend_id"] not in backends:
            continue
        result["evidence"] = evidence
        passed = set(result["passed"]) - {"injection_attack_suite", "auth_section_observation_stop"}
        if complete:
            passed |= {"injection_attack_suite", "auth_section_observation_stop"}
        result["passed"] = sorted(passed)
    temporary = output / "conformance.json.tmp"
    temporary.write_text(json.dumps(ledger, indent=2) + "\n")
    temporary.replace(output / "conformance.json")
    print(json.dumps({"record": str(output / "conformance.json"), "p4b_complete": complete,
                      "evidence": evidence}))
    return 0 if complete else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--p4b-evidence", metavar="LEDGER",
                        help="add P4-B attack/H3 test evidence to this measured ledger (ADR-0112)")
    parser.add_argument("--p4b-backend", action="append", default=[],
                        help="backend id that receives the P4-B evidence (repeatable)")
    parser.add_argument("--agent-browser", default="agent-browser")
    parser.add_argument("--output-dir", required=True)
    parser.add_argument("--backend-command", action="append", default=[],
                        help='ID=["command","arg",...] (one per backend)')
    parser.add_argument("--scripted", action="store_true",
                        help="use this file as a scripted LLM driver for all three backends")
    parser.add_argument("--protocol-scripted", action="store_true",
                        help="run the Rust ACP, Claude and specialist adapters against a scripted LLM")
    parser.add_argument("--fallback-scenario", action="store_true",
                        help="after protocol conformance, exercise worker fallback with the real browser")
    parser.add_argument("--drive", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.drive:
        return drive()
    if args.p4b_evidence:
        if not args.p4b_backend or not set(args.p4b_backend) <= set(BACKENDS):
            parser.error("--p4b-evidence needs one or more known --p4b-backend ids")
        output = Path(args.output_dir)
        output.mkdir(parents=True, exist_ok=True)
        return p4b_evidence(args.p4b_evidence, set(args.p4b_backend), output)
    command = {}
    for item in args.backend_command:
        backend, sep, argv = item.partition("=")
        if not sep or backend not in BACKENDS or backend in command:
            parser.error("each backend command must have a unique known ID")
        parsed = json.loads(argv)
        if not isinstance(parsed, list) or not parsed or not all(isinstance(a, str) for a in parsed):
            parser.error("backend command must be a nonempty JSON string array")
        command[backend] = parsed
    if (args.scripted or args.protocol_scripted) and command:
        parser.error("choose a scripted mode or explicit backend commands")
    if args.scripted and args.protocol_scripted:
        parser.error("choose only one scripted mode")
    if args.fallback_scenario and not args.protocol_scripted:
        parser.error("--fallback-scenario requires --protocol-scripted")
    if args.scripted:
        command = {backend: [sys.executable, str(Path(__file__).resolve()),
                             "--drive", "--output-dir", args.output_dir] for backend in BACKENDS}
    if args.protocol_scripted:
        manifest = Path(__file__).resolve().parents[1] / "Cargo.toml"
        rust_command = ["cargo", "test", "--manifest-path", str(manifest), "-p", "task-worker",
                        "--lib", "p4c_conformance_backend_protocol", "--", "--ignored", "--nocapture"]
        command = {backend: rust_command for backend in BACKENDS}
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
            result, same_task = backend_run(backend, command[backend], executable, origin, output,
                                            args.protocol_scripted)
            results.append(result)
            comparison.append(same_task)
        # Legacy direct-scripted mode skips harness protocols. The worker deliberately
        # refuses its ledger as backend certification.
        source = "celeris-browser-conformance-scripted" if args.scripted else "celeris-browser-conformance"
        ledger = {"schema": 1, "source": source, "results": results}
        temporary = output / "conformance.json.next"
        temporary.write_text(json.dumps(ledger, indent=2) + "\n")
        if args.protocol_scripted and all(run["accepted"] for run in comparison):
            env = os.environ.copy()
            env["CELERIS_BROWSER_CONFORMANCE_FILE"] = str(temporary)
            route = subprocess.run(
                ["cargo", "test", "--manifest-path", str(manifest), "-p", "task-worker", "--lib",
                 "p4c_runner_record_routes_and_falls_back", "--", "--ignored", "--nocapture"],
                cwd=output, env=env, text=True, capture_output=True, timeout=180, check=False,
            )
            (output / "routing-test.json").write_text(json.dumps({
                "exit": route.returncode, "stdout_tail": route.stdout[-2000:],
                "stderr_tail": route.stderr[-2000:],
            }))
            if route.returncode:
                temporary.unlink()
                print("routing test rejected runner ledger", file=sys.stderr)
                return 1
            if args.fallback_scenario:
                first_fallback_request = len(Site.requests)
                browser_bin = output / "runner-bin"
                browser_bin.mkdir(mode=0o700)
                (browser_bin / "agent-browser").symlink_to(executable)
                env["PATH"] = str(browser_bin) + os.pathsep + env.get("PATH", "")
                env["CELERIS_BROWSER_FALLBACK_ROOT"] = str(output / "fallback")
                env["CELERIS_BROWSER_EXECUTABLE"] = executable
                env["CELERIS_BROWSER_ORIGIN"] = origin
                fallback = subprocess.run(
                    ["cargo", "test", "--manifest-path", str(manifest), "-p", "task-worker", "--lib",
                     "p4c_fallback_real_harness_scenario", "--", "--ignored", "--nocapture"],
                    cwd=output, env=env, text=True, capture_output=True, timeout=300, check=False,
                )
                (output / "fallback-test.json").write_text(json.dumps({
                    "exit": fallback.returncode, "stdout_tail": fallback.stdout[-3000:],
                    "stderr_tail": fallback.stderr[-3000:],
                }))
                if fallback.returncode:
                    temporary.unlink()
                    print("real browser fallback scenario failed", file=sys.stderr)
                    return 1
                observed = Site.requests[first_fallback_request:]
                (output / "fallback-fixture.json").write_text(json.dumps({
                    "requests": observed,
                    "alternate_clicked": ("POST", "/clicked") in observed,
                    "navigation_count": observed.count(("GET", "/")),
                }, indent=2) + "\n")
                if ("POST", "/clicked") not in observed or observed.count(("GET", "/")) < 2:
                    temporary.unlink()
                    print("fallback completion not observed by fixture", file=sys.stderr)
                    return 1
        temporary.replace(output / "conformance.json")
        (output / "same-task.json").write_text(json.dumps(comparison, indent=2) + "\n")
        print(json.dumps({"record": str(output / "conformance.json"), "comparison": comparison}))
        return 0 if all(run["accepted"] for run in comparison) else 1
    finally:
        server.shutdown()
        thread.join(timeout=5)


if __name__ == "__main__":
    sys.exit(main())
