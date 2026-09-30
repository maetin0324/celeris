#!/usr/bin/env python3
"""Celeris transport/observability shim for agent-browser 0.38.1, not a browser agent.

Credential requests stop the browser run and contain no secret. This same-UID shim is not a
security sandbox. The substrate enforces navigation/network and action policies.
"""
import fcntl
import contextlib
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import socket
import sys
import uuid
from urllib.parse import urlsplit

os.umask(0o077)
ROOT = Path(__file__).resolve().parent


class PolicyBlocked(ValueError):
    pass


# Must equal task-core's HARNESS_UPSTREAM_ACTIONS. Upstream 0.38.1 treats a missing,
# unparsable or empty-allow policy as "no policy" (fail-open), so never invoke it then.
ACTIONS = {"launch", "navigate", "click", "snapshot", "gettext", "screenshot", "download", "scroll", "close"}
# Shim verb -> upstream internal action checked against the generated allow list.
VERB_ACTIONS = {"open": "navigate", "click": "click", "snapshot": "snapshot", "extract": "gettext",
                "screenshot": "screenshot", "download": "download", "scroll": "scroll", "close": "close"}
HOST = re.compile(r"(\*\.)?[a-z0-9-]+(\.[a-z0-9-]+)+|[a-z0-9-]+")


def load_policy(config):
    """The generated policy, or None unless it and the domain filter are intact."""
    try:
        raw = (ROOT / "policy.json").read_bytes()
        policy = json.loads(raw)
    except (OSError, ValueError):
        return None
    domains = config.get("allowed_domains")
    digest = config.get("policy_sha256")
    ok = (isinstance(digest, str) and hashlib.sha256(raw).hexdigest() == digest
          and isinstance(policy, dict) and set(policy) == {"default", "allow"}
          and policy["default"] == "deny" and isinstance(policy["allow"], list)
          and {"launch", "close"} < set(policy["allow"])
          and len(set(policy["allow"])) == len(policy["allow"])
          and all(isinstance(a, str) and a in ACTIONS for a in policy["allow"])
          and isinstance(domains, list) and len(domains) > 0
          and all(isinstance(d, str) and HOST.fullmatch(d) for d in domains))
    return policy if ok else None


def host_allowed(host, domains):
    """Same containment rule as task-core: `*.base` admits subdomains, never the apex."""
    host = host.lower() if host else ""
    return any(host.endswith(d[1:]) if d.startswith("*.") else host == d for d in domains)


def audit(operation, status, artifact=None):
    event = {"operation": operation, "status": status}
    if artifact:
        event["artifact"] = artifact
    with (ROOT / "events.jsonl").open("a") as out:
        out.write(json.dumps(event) + "\n")


def plan(args, output):
    """Restrict CLI grammar, not DOM semantics. Never accept arbitrary flags or paths."""
    if not args:
        raise ValueError()
    verb, *rest = args
    ref = lambda value: re.fullmatch(r"@e[0-9]+", value) is not None
    if verb == "open" and len(rest) == 1:
        url = urlsplit(rest[0])
        if (url.scheme not in ("http", "https") or not url.hostname or url.username
                or url.password or url.query or url.fragment or "\\" in rest[0]
                or any(ord(c) <= 32 for c in rest[0])):
            raise ValueError()
        return "navigate", ["open", rest[0]], None
    if verb == "click" and len(rest) == 1 and ref(rest[0]):
        return "click", args, None
    if verb == "snapshot" and not rest:
        return "extract", ["snapshot", "-i"], None
    if verb == "extract" and len(rest) == 1 and ref(rest[0]):
        name = "extract-" + uuid.uuid4().hex + ".json"
        return "extract", ["get", "text", rest[0]], output / name
    if verb == "screenshot" and not rest:
        path = output / ("screenshot-" + uuid.uuid4().hex + ".png")
        return "screenshot", ["screenshot", str(path)], path
    if verb == "download" and len(rest) == 1 and ref(rest[0]):
        path = output / ("download-" + uuid.uuid4().hex + ".bin")
        return "download", ["download", rest[0], str(path)], path
    if verb == "scroll" and len(rest) == 2 and rest[0] in ("up", "down") and rest[1].isdigit() and 1 <= int(rest[1]) <= 2000:
        return "scroll", args, None
    if verb == "close" and not rest:
        return "close", args, None
    raise ValueError()


def main(args):
    config = json.loads((ROOT / "config.json").read_text())
    output = Path(config["output"])
    if args and args[0] == "request-credential":
        policy = load_policy(config)
        try:
            if policy is None or not config.get("credential_use") or len(args) != 4:
                raise ValueError()
            policy_id, origin, purpose = args[1:]
            parsed = urlsplit(origin)
            if (policy_id not in config.get("credential_policy_ids", [])
                    or parsed.scheme != "https" or not parsed.hostname
                    or parsed.username or parsed.password or parsed.path or parsed.query or parsed.fragment
                    or parsed.netloc.lower() != parsed.netloc
                    or not host_allowed(parsed.hostname, config["allowed_domains"])
                    or not 1 <= len(purpose) <= 500 or any(ord(c) < 32 for c in purpose)):
                raise ValueError()
            with (ROOT / "credential-request.json").open("x") as request:
                json.dump({"policy_id": policy_id, "origin": origin, "purpose": purpose}, request)
        except (ValueError, OSError):
            audit("policy_block", "blocked")
            print('{"success":false,"error":"credential request blocked"}')
            return 2
        audit("credential_request", "success")
        print('{"success":true,"status":"waiting_for_auth"}')
        return 0
    try:
        operation, command, artifact = plan(args, output)
    except (ValueError, OverflowError):
        audit("policy_block", "blocked")
        print('{"success":false,"error":"command blocked by browser capability"}')
        return 2
    policy = load_policy(config)
    if policy is None:
        audit("policy_block", "blocked")
        print('{"success":false,"error":"browser policy unavailable; refusing to run"}')
        return 2
    # Defence in depth ahead of the substrate: actions and hosts outside the task policy
    # never reach agent-browser, whatever page content asked for.
    if (VERB_ACTIONS[args[0]] not in policy["allow"]
            or (args[0] == "open" and not host_allowed(urlsplit(args[1]).hostname, config["allowed_domains"]))):
        audit("policy_block", "blocked")
        print('{"success":false,"error":"command not permitted by the task browser policy"}')
        return 2
    try:
        # Serialize calls within a session so refs/artifact/event order stays meaningful.
        with (ROOT / "lock").open("w") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
                connection.settimeout(50)
                connection.connect(config["action_socket"])
                request = {"verb": args[0], "args": args[1:],
                           "artifact": artifact.name if artifact and args[0] in ("screenshot", "download") else None}
                connection.sendall(json.dumps(request).encode())
                connection.shutdown(socket.SHUT_WR)
                response = connection.recv(1024 * 1024 + 4096)
            result = json.loads(response)
        if len(result.get("stdout", "")) > 1024 * 1024:
            raise ValueError()
        data = json.loads(result["stdout"])
        if isinstance(data, dict) and data.get("success") is False:
            error = data.get("error")
            if isinstance(error, str) and any(marker in error.lower() for marker in
                    ("denied by policy", "allowed domains", "not allowed by domain filter")):
                raise PolicyBlocked()
        if result["status"] or not isinstance(data, dict) or data.get("success") is not True:
            # Error text can contain URL credentials or reflected page content.
            raise ValueError()
        if artifact and operation == "extract":
            artifact.write_text(json.dumps({"untrusted": True, "data": data.get("data")}))
        if artifact:
            if artifact.is_symlink() or not artifact.is_file() or artifact.stat().st_size > 10 * 1024 * 1024:
                raise ValueError()
        audit(operation, "success", artifact.name if artifact else None)
        if operation == "extract":
            print("<untrusted_browser_content>")
            # Keep upstream's nonce/origin boundary instead of inventing a replacement protocol.
            print(json.dumps({"_boundary": data.get("_boundary"), "data": data.get("data")}))
            print("</untrusted_browser_content>")
        else:
            print(json.dumps({"success": True, "artifact": artifact.name if artifact else None}))
        return 0
    except (OSError, KeyError, ValueError, TypeError) as error:
        if artifact:
            with contextlib.suppress(OSError):
                artifact.unlink(missing_ok=True)
        # All substrate failures are opaque. Never echo stdout/stderr/arguments.
        if isinstance(error, PolicyBlocked):
            audit("policy_block", "blocked")
        else:
            audit(operation, "failure")
        print('{"success":false,"error":"browser action failed or was blocked"}')
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
