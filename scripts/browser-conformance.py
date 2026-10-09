#!/usr/bin/env python3
"""P4-C local fixture runner for real agent-browser 0.38.1.

The LLM driver may be scripted; the browser substrate is always the actual executable.
No fixture page contains external resources. The runner writes a ledger only after invoking
each named backend process and checking its browser events and fixture server observations.
"""

import argparse
import contextlib
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
from threading import Thread
import time
from urllib.parse import urlsplit
from datetime import datetime, timezone

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
        if self.path == "/clicked":
            # GET, not POST: the isolated runtime's egress forwards only GET/HEAD for plain
            # HTTP, so the fallback scenario could not observe a POST click.
            self.send_response(204)
            self.end_headers()
            return
        if self.path == "/download":
            body = b"celeris local fixture\n"
            kind = "application/octet-stream"
        elif self.path == "/":
            body = (b"<!doctype html><html><body>"
                    b"<button onclick=\"fetch('/clicked',{cache:'no-store'})\">Click fixture</button>"
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


# The shim (browser_cli.py) only talks to an action socket; in production the worker's
# daemon/launcher runtime serves it. The protocol pass has no worker runtime, so the runner
# serves the same request format on the host and runs the real agent-browser with the
# same upstream flags (action policy, domain filter, content boundaries) the shim used
# before the isolated runtime existed. The browser sandbox itself is not a P4-C case.
ACTION_VERBS = {"open": "navigate", "click": "click", "snapshot": "snapshot",
                "extract": "gettext", "screenshot": "screenshot", "download": "download",
                "scroll": "scroll", "close": "close"}
ACTION_ARTIFACT = re.compile(r"(screenshot|download)-[a-f0-9]{32}\.(png|bin)")


def action_argv(runtime, config, allow, request):
    """Upstream argv for one shim request, or ValueError. Mirrors browser_action.py."""
    verb, args, artifact = request.get("verb"), request.get("args"), request.get("artifact")
    if verb == "__version__" and args == []:
        return [config["executable"], "--version"]
    if (verb not in ACTION_VERBS or ACTION_VERBS[verb] not in allow or not isinstance(args, list)
            or len(args) > 2 or any(not isinstance(a, str) or len(a) > 4096 for a in args)):
        raise ValueError()
    ref = lambda value: re.fullmatch(r"@e[0-9]+", value) is not None
    target = None
    if verb in ("screenshot", "download"):
        if not isinstance(artifact, str) or not ACTION_ARTIFACT.fullmatch(artifact):
            raise ValueError()
        target = str(Path(config["output"]) / artifact)
    elif artifact is not None:
        raise ValueError()
    if verb == "open" and len(args) == 1 and urlsplit(args[0]).scheme in ("http", "https"):
        action = ["open", args[0]]
    elif verb == "click" and len(args) == 1 and ref(args[0]):
        action = ["click", args[0]]
    elif verb == "extract" and len(args) == 1 and ref(args[0]):
        action = ["get", "text", args[0]]
    elif verb == "download" and len(args) == 1 and ref(args[0]):
        action = ["download", args[0], target]
    elif verb == "snapshot" and not args:
        action = ["snapshot", "-i"]
    elif verb == "screenshot" and not args:
        action = ["screenshot", target]
    elif verb == "scroll" and len(args) == 2 and args[0] in ("up", "down") and args[1].isdigit():
        action = ["scroll", *args]
    elif verb == "close" and not args:
        action = ["close"]
    else:
        raise ValueError()
    hosts = sorted({urlsplit(d).hostname for d in config["allowed_domains"]})
    return [config["executable"], "--config", str(runtime / "upstream.json"),
            "--session", config["session_id"], "--action-policy", str(runtime / "policy.json"),
            "--allowed-domains", ",".join(hosts),
            "--content-boundaries", "--max-output", "16000", "--json", *action]


def serve_actions(runtime, path):
    """Serve the shim's action socket for one backend runtime until the socket is closed."""
    config = json.loads((runtime / "config.json").read_text())
    allow = set(json.loads((runtime / "policy.json").read_text())["allow"])
    env = {k: v for k, v in os.environ.items() if not k.startswith("AGENT_BROWSER_")}
    env.update(AGENT_BROWSER_NAMESPACE="celeris", HTTP_PROXY="", HTTPS_PROXY="", ALL_PROXY="",
               NO_PROXY="localhost,127.0.0.1")
    listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    listener.bind(str(path))
    listener.listen(4)

    def loop():
        while True:
            try:
                connection, _ = listener.accept()
            except OSError:
                return
            with connection:
                try:
                    data = b""
                    while chunk := connection.recv(65536):
                        data += chunk
                    argv = action_argv(runtime, config, allow, json.loads(data))
                    result = subprocess.run(argv, cwd=runtime, env=env, stdout=subprocess.PIPE,
                                            stderr=subprocess.DEVNULL, timeout=45, check=False)
                    response = {"status": result.returncode,
                                "stdout": result.stdout[:1048576].decode("utf-8", errors="replace")}
                except (OSError, ValueError, KeyError, TypeError, subprocess.TimeoutExpired):
                    response = {"status": 2, "stdout": ""}
                try:
                    connection.sendall(json.dumps(response).encode())
                except OSError:
                    pass

    Thread(target=loop, daemon=True).start()
    return listener


def prepare_runtime(backend, agent_browser, origin, root, socket_dir):
    """Write the shim, policy and config for one backend session; return (runtime, output, cli)."""
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
    # Canonical origin (scheme, host, port) as task-core and the shim compare it; a bare
    # "localhost" would mean https://localhost:443 and refuse the loopback fixture.
    (runtime / "config.json").write_text(json.dumps({
        "executable": str(agent_browser), "session_id": f"celeris-p4c-{backend}",
        "allowed_domains": [origin.rstrip("/")], "output": str(output),
        "policy_sha256": hashlib.sha256(encoded).hexdigest(),
        "credential_policy_ids": [], "credential_use": False,
        "action_socket": str(Path(socket_dir) / "a.sock"),
    }))
    return runtime, output, cli


def backend_run(backend, command, agent_browser, origin, root, protocol_scripted=False):
    first_request = len(Site.requests)
    # Short path: unix sun_path is 108 bytes and the output dir may be deep.
    socket_dir = Path(tempfile.mkdtemp(prefix="cbc-"))
    runtime, output, cli = prepare_runtime(backend, agent_browser, origin, root, socket_dir)
    action_socket = socket_dir / "a.sock"
    listener = serve_actions(runtime, action_socket)
    env = os.environ.copy()
    env.update(CELERIS_BROWSER_CLI=str(cli), CELERIS_BROWSER_ORIGIN=origin,
               CELERIS_BROWSER_BACKEND=backend, CELERIS_BROWSER_RUNTIME=str(runtime),
               HTTP_PROXY="", HTTPS_PROXY="", ALL_PROXY="", NO_PROXY="localhost,127.0.0.1")
    start = time.monotonic()
    process_ok = []
    try:
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
    finally:
        with contextlib.suppress(OSError):
            listener.shutdown(socket.SHUT_RDWR)
        listener.close()
        shutil.rmtree(socket_dir, ignore_errors=True)
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
    if event("click", "success") and ("GET", "/clicked") in requests:
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

# ADR 2026-10-09 D1. Keep these names aligned with task_core::browser_backend.
P4A_ISOLATION_TESTS = tuple(
    ("task-worker", target, name) for target, name in (
        ("browser_runtime_isolated", "probe_inside_runtime_cannot_reach_host_sockets_or_network"),
        ("browser_runtime_isolated", "real_browser_in_runtime_facts_and_restore_refused_on_same_uid"),
        ("browser_runtime_isolated", "controller_kill_leaves_no_runtime_processes"),
        ("browser_runtime_isolated", "restart_reaps_recorded_runtime_and_ignores_stale_records"),
        ("browser_runtime_supervisor", "runtime_processes_do_not_survive_controller_kill_restart_or_stop"),
        ("browser_launcher_ptrace", "launcher_chrome_denies_daemon_uid_ptrace"),
    )) + tuple(("task-core", "lib", f"browser_isolation::tests::{name}") for name in (
        "verified_runtime_is_isolated", "daemon_owned_userns_is_rejected", "unknown_userns_owner_is_rejected",
        "same_uid_and_root_are_rejected", "every_namespace_is_required",
        "root_must_be_readonly_and_writes_stay_in_session", "broker_and_host_ipc_are_not_visible",
        "cdp_must_not_be_on_tcp_or_outside_controller_dir", "privileges_and_pgid_are_checked",
        "bwrap_argv_unshares_everything_and_remounts_ro", "owner_check_alone_without_proof_is_rejected",
        "proof_does_not_override_isolation_violations", "launched_facts_reject_namespace_shared_with_daemon",
    ))
P4A_EGRESS_NEGATIVE_TESTS = tuple(
    ("task-worker", target, name) for target, name in (
        ("browser_egress_relay", "fixture_reachable_only_through_per_connection_egress_proxy"),
        ("browser_egress_process", "independent_proxy_refuses_worker_selected_private_or_proxy_destinations"),
        ("browser_egress_process", "malformed_or_oversized_policy_never_appears_in_process_output"),
        ("browser_egress_process", "missing_inherited_socket_is_refused"),
    )) + tuple(("task-worker", "lib", f"browser_egress::tests::{name}") for name in (
        "real_unix_transport_rejects_proxy_dns_and_http_bypasses_before_resolution",
        "actual_dns_transport_refuses_private_ipv6_and_rebinding",
        "denied_origin_never_reaches_even_the_configured_dns_socket", "oversized_header_is_bounded_and_denied",
        "malformed_dns_cannot_inject_an_address", "dns_cname_requires_terminal_owner_and_refuses_cycles",
        "malformed_dns_length_or_transaction_is_rejected_over_tcp",
        "browser_allowed_domains_egress_denies_outside_task_and_grant",
        "egress_get_switching_origin_on_the_same_connection_never_connects",
        "egress_get_denials_are_recorded_like_connect", "egress_test_loopback_connect_allowed_only_when_listed",
    )) + tuple(("task-core", "lib", f"browser_isolation::tests::{name}") for name in (
        "egress_rejects_private_ranges_and_rebinding", "egress_rejects_ipv6_private_and_disabled",
        "egress_rejects_ip_literals_and_unlisted_hosts", "egress_rejects_dns_bypass_and_proxy_chain",
        "egress_test_loopback_default_off_keeps_ip_literal",
        "egress_test_loopback_other_private_and_variants_stay_denied",
    ))


def credential_evidence(ledger_path, backends, output):
    output = Path(output)
    output.mkdir(parents=True, exist_ok=True)
    ledger = json.loads(Path(ledger_path).read_text())
    if ledger.get("schema") != 1 or ledger.get("source") != "celeris-browser-conformance":
        print("ledger is not a measured celeris-browser-conformance record", file=sys.stderr)
        return 1
    cargo = os.environ.get("CELERIS_CONFORMANCE_CARGO", "cargo")
    manifest = Path(__file__).resolve().parents[1] / "Cargo.toml"
    logs = output / "credential-logs"
    logs.mkdir(parents=True, exist_ok=True)
    all_evidence = []
    try:
        build = subprocess.run([cargo, "build", "--manifest-path", str(manifest), "-p", "task-worker",
                                "--bin", "celeris-browser-sandboxd", "--bin", "celeris-browser-egress"],
                               text=True, capture_output=True, timeout=600, check=False)
        (logs / "build.log").write_text(build.stdout + "\n" + build.stderr)
        build_ok = build.returncode == 0
    except (OSError, subprocess.TimeoutExpired) as error:
        build_ok = False
        (logs / "build.log").write_text(str(error))
    outcomes = {}
    for case, tests in (("isolation_suite", P4A_ISOLATION_TESTS),
                        ("egress_negative_suite", P4A_EGRESS_NEGATIVE_TESTS)):
        for package, target, test in tests:
            full_name = f"{package}:{target}::{test}"
            outcome = "not_run"
            if build_ok:
                target_args = ["--lib"] if target == "lib" else ["--test", target]
                env = os.environ.copy()
                env["CELERIS_USERNS_TESTS"] = "1"
                env["CELERIS_LAUNCHER_TESTS"] = "require"
                env.pop("CELERIS_ISOLATION_TESTS", None)
                tmpdir = None
                if len(env.get("TMPDIR", "")) > 40:
                    tmpdir = tempfile.mkdtemp(prefix="cel-ce-")
                    env["TMPDIR"] = tmpdir
                try:
                    done = subprocess.run([cargo, "test", "--manifest-path", str(manifest), "-p", package,
                                           *target_args, "--", "--exact", test, "--nocapture", "--test-threads=1"],
                                          text=True, capture_output=True, timeout=600, check=False, env=env)
                    combined = done.stdout + "\n" + done.stderr
                    if done.returncode and re.search(rf"^test {re.escape(test)} \.\.\. FAILED$", done.stdout, re.M):
                        outcome = "failed"
                    elif (done.returncode == 0 and re.search(r"^running 1 test$", done.stdout, re.M)
                          and re.search(rf"^test {re.escape(test)} \.\.\. ok$", done.stdout, re.M)
                          and re.search(r"^test result: ok\. 1 passed; 0 failed; 0 ignored", done.stdout, re.M)
                          and not re.search(r"SKIPPED|^SKIP:|\(not passed\)", combined, re.M)):
                        outcome = "passed"
                    (logs / f"{package}.{target}.{test.replace(':', '_')}.log").write_text(combined)
                except subprocess.TimeoutExpired as error:
                    (logs / f"{package}.{target}.{test.replace(':', '_')}.log").write_text(
                        (error.stdout or "") + "\n" + (error.stderr or "") + "\\nTIMEOUT")
                except OSError as error:
                    (logs / f"{package}.{target}.{test.replace(':', '_')}.log").write_text(str(error))
                finally:
                    if tmpdir:
                        shutil.rmtree(tmpdir, ignore_errors=True)
            outcomes[full_name] = outcome
            all_evidence.append({"case": case, "test": full_name, "outcome": outcome})
    complete = build_ok and all(row["outcome"] == "passed" for row in all_evidence)
    for result in ledger["results"]:
        if result["backend_id"] not in backends:
            continue
        retained = [row for row in result.get("evidence", [])
                    if row.get("case") not in ("isolation_suite", "egress_negative_suite")]
        result["evidence"] = retained + all_evidence
        passed = set(result.get("passed", [])) - {"isolation_suite", "egress_negative_suite"}
        if complete:
            passed.update(("isolation_suite", "egress_negative_suite"))
        result["passed"] = sorted(passed)
    write_ledger(output, ledger["results"], source=ledger["source"], generated_for=ledger.get("generated_for"))
    failed = any(row["outcome"] == "failed" for row in all_evidence)
    code = "build_failed" if not build_ok else ("tests_failed" if failed else ("ok" if complete else "tests_not_run"))
    report = {"complete": complete, "code": code,
              "reason": next((row["test"] for row in all_evidence if row["outcome"] != "passed"), ""),
              "evidence": all_evidence, "record": str(output / "conformance.json")}
    (output / "credential-evidence.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report))
    return 0 if complete else 1

AGENT_BROWSER_VERSION = "0.38.1"
CONFORMANCE_FIXTURE = "p4c-local-v1"


def write_ledger(output, results, *, source="celeris-browser-conformance", generated_for=None):
    """Atomically write the schema-1 conformance ledger and optional release provenance."""
    ledger = {"schema": 1, "source": source, "results": results}
    if generated_for is not None:
        ledger["generated_for"] = generated_for
    output = Path(output)
    output.mkdir(parents=True, exist_ok=True)
    temporary = output / "conformance.json.next"
    temporary.write_text(json.dumps(ledger, indent=2) + "\n")
    temporary.replace(output / "conformance.json")
    return output / "conformance.json"


def release_provenance(release, celeris_sha=None, *, generated_at=None):
    """Build the generated_for object shared with task-worker's GeneratedFor type."""
    generated_at = generated_at or datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")
    provenance = {
        "celeris_release": release,
        "agent_browser": AGENT_BROWSER_VERSION,
        "fixture": CONFORMANCE_FIXTURE,
        "generated_at": generated_at,
    }
    if celeris_sha is not None:
        provenance["celeris_sha"] = celeris_sha
    return provenance


def p4b_evidence(ledger_path, backends, output):
    """Run the real attack matrix and H3 e2e, then add their per-test results to a measured
    ledger. The P4-B cases enter `passed` only when every required test passed; otherwise the
    evidence is kept with its failed/not_run outcome and the cases stay out (fail closed)."""
    output = Path(output)
    output.mkdir(parents=True, exist_ok=True)
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
    # Preserve provenance from the original P4-C run while updating evidence.
    write_ledger(output, ledger["results"], source=ledger["source"],
                 generated_for=ledger.get("generated_for"))
    print(json.dumps({"record": str(output / "conformance.json"), "p4b_complete": complete,
                      "evidence": evidence}))
    return 0 if complete else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--credential-evidence", metavar="LEDGER",
                        help="add measured P4-A isolation and egress evidence")
    parser.add_argument("--credential-backend", action="append", default=[],
                        help="backend id that receives credential evidence (repeatable)")
    parser.add_argument("--p4b-evidence", metavar="LEDGER",
                        help="add P4-B attack/H3 test evidence to this measured ledger (ADR-0112)")
    parser.add_argument("--p4b-backend", action="append", default=[],
                        help="backend id that receives the P4-B evidence (repeatable)")
    parser.add_argument("--agent-browser", default="agent-browser")
    parser.add_argument("--celeris-release", help="release sha12 this conformance ledger certifies")
    parser.add_argument("--celeris-sha", help="full 40-character Celeris commit SHA")
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
    if args.celeris_sha and not re.fullmatch(r"[0-9a-fA-F]{40}", args.celeris_sha):
        parser.error("--celeris-sha must be a 40-character hexadecimal SHA")
    if args.celeris_release and not re.fullmatch(r"[0-9a-fA-F]{12}", args.celeris_release):
        parser.error("--celeris-release must be a 12-character hexadecimal SHA")
    if args.celeris_sha and not args.celeris_release:
        parser.error("--celeris-sha requires --celeris-release")
    if args.p4b_evidence and (args.celeris_release or args.celeris_sha):
        parser.error("--celeris-release/--celeris-sha apply only when generating a conformance ledger")
    if args.credential_evidence and (args.celeris_release or args.celeris_sha or args.p4b_evidence):
        parser.error("credential evidence cannot be combined with release provenance options or --p4b-evidence")
    if args.drive:
        return drive()
    if args.p4b_evidence:
        if not args.p4b_backend or not set(args.p4b_backend) <= set(BACKENDS):
            parser.error("--p4b-evidence needs one or more known --p4b-backend ids")
        output = Path(args.output_dir)
        output.mkdir(parents=True, exist_ok=True)
        return p4b_evidence(args.p4b_evidence, set(args.p4b_backend), output)
    if args.credential_evidence:
        if not args.credential_backend or not set(args.credential_backend) <= set(BACKENDS):
            parser.error("--credential-evidence needs one or more known --credential-backend ids")
        return credential_evidence(args.credential_evidence, set(args.credential_backend), args.output_dir)
    if args.credential_backend:
        parser.error("--credential-backend requires --credential-evidence")
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
        generated_for = (release_provenance(args.celeris_release, args.celeris_sha)
                         if args.celeris_release else None)
        ledger_path = write_ledger(output, results, source=source, generated_for=generated_for)
        temporary = output / "conformance.json"
        if args.protocol_scripted and all(run["accepted"] for run in comparison):
            env = os.environ.copy()
            env["CELERIS_BROWSER_CONFORMANCE_FILE"] = str(temporary)
            # The routing and fallback tests launch the worker's isolated runtime, which
            # needs these binaries next to the test executable; `cargo test --lib` does not
            # build them (isolated_runtime_unavailable otherwise).
            bins = subprocess.run(
                ["cargo", "build", "--manifest-path", str(manifest), "-p", "task-worker",
                 "--bin", "celeris-browser-sandboxd", "--bin", "celeris-browser-egress"],
                cwd=output, env=env, text=True, capture_output=True, timeout=900, check=False,
            )
            if bins.returncode:
                (output / "runtime-build.log").write_text(bins.stdout + "\n" + bins.stderr)
                temporary.unlink()
                print("isolated runtime binaries failed to build", file=sys.stderr)
                return 1
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
                    "alternate_clicked": ("GET", "/clicked") in observed,
                    "navigation_count": observed.count(("GET", "/")),
                }, indent=2) + "\n")
                if ("GET", "/clicked") not in observed or observed.count(("GET", "/")) < 2:
                    temporary.unlink()
                    print("fallback completion not observed by fixture", file=sys.stderr)
                    return 1
        (output / "same-task.json").write_text(json.dumps(comparison, indent=2) + "\n")
        print(json.dumps({"record": str(ledger_path), "comparison": comparison}))
        return 0 if all(run["accepted"] for run in comparison) else 1
    finally:
        server.shutdown()
        thread.join(timeout=5)


if __name__ == "__main__":
    sys.exit(main())
