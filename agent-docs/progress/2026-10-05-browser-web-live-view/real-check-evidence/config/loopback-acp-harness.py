#!/usr/bin/env python3
"""Local scripted ACP harness for scripts/dev/browser-web-live-check.sh (no LLM, no network).

Speaks ACP JSON-RPC on stdio. On session/prompt it uses only the Celeris browser shim named in
the prompt: opens the allowed loopback page, reads it, attempts the out-of-scope loopback origin,
records the denial (strict JSON demanded by docs/ops/browser-web-live-check.md), then polls a
read-only action every few seconds for REAL_CHECK_HOLD_SECS so the run stays RUNNING while the
check takes/returns the control lease. Every shim call (time, args, exit, output tail) is logged.
"""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

LOG = Path(os.environ["REAL_CHECK_LOG"])
ALLOWED = os.environ["REAL_CHECK_ALLOWED"]
DENIED = os.environ["REAL_CHECK_DENIED"]
DENIAL = Path(os.environ["REAL_CHECK_DENIAL_FILE"])
HOLD = int(os.environ.get("REAL_CHECK_HOLD_SECS", "240"))


def log(entry):
    entry = {"t": round(time.time(), 3), **entry}
    with LOG.open("a") as f:
        f.write(json.dumps(entry, ensure_ascii=False) + "\n")


def shim(cli, *args, timeout=150):
    start = time.monotonic()
    try:
        r = subprocess.run([sys.executable, cli, *args], text=True, capture_output=True,
                           timeout=timeout, check=False)
        code, out, err = r.returncode, r.stdout, r.stderr
    except subprocess.TimeoutExpired:
        code, out, err = "timeout", "", ""
    log({"call": list(args), "exit": code, "secs": round(time.monotonic() - start, 2),
         "stdout_tail": out[-600:], "stderr_tail": err[-600:]})
    return code, out


def act(prompt):
    m = re.search(r'Use `python3 "((?:[^"\\]|\\.)+)" <command>`', prompt)
    if not m:
        log({"error": "browser shim path not found in prompt"})
        return False
    cli = json.loads('"' + m.group(1) + '"')
    log({"cli": cli})
    opened, _ = shim(cli, "open", ALLOWED + "/")
    snap_code, snap = shim(cli, "snapshot")
    denied_code, _ = shim(cli, "open", DENIED + "/")
    log({"summary": {"allowed_open_exit": opened, "snapshot_exit": snap_code,
                     "snapshot_has_inside": "inside" in snap, "denied_open_exit": denied_code}})
    if opened == 0 and denied_code != 0:
        DENIAL.write_text(json.dumps({"allowed_origin": ALLOWED, "attempted_origin": DENIED,
                                      "denied": True, "source": "agent-browser"}))
    deadline = time.monotonic() + HOLD
    while time.monotonic() < deadline:
        shim(cli, "snapshot", timeout=30)
        time.sleep(3)
    return opened == 0


def main():
    log({"started": True, "argv": sys.argv[1:], "cwd": os.getcwd()})
    for line in sys.stdin:
        try:
            request = json.loads(line)
        except ValueError:
            continue
        method = request.get("method")
        log({"rpc": method})
        if "id" not in request or method is None:
            continue
        if method == "initialize":
            result = {"protocolVersion": 1, "agentCapabilities": {}}
        elif method == "session/new":
            result = {"sessionId": "loopback-real-check", "configOptions": []}
        elif method == "session/prompt":
            text = "".join(b.get("text", "") for b in request.get("params", {}).get("prompt", [])
                           if isinstance(b, dict))
            ok = act(text)
            Path("artifacts").mkdir(exist_ok=True)
            Path("artifacts/result.json").write_text(json.dumps(
                {"summary": "loopback browser check " + ("completed" if ok else "failed"),
                 "evidence": []}))
            result = {"stopReason": "end_turn"}
        else:
            print(json.dumps({"jsonrpc": "2.0", "id": request["id"],
                              "error": {"code": -32601, "message": "not supported"}}), flush=True)
            continue
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
        if method == "session/prompt":
            return 0
    return 1


if __name__ == "__main__":
    sys.exit(main())
