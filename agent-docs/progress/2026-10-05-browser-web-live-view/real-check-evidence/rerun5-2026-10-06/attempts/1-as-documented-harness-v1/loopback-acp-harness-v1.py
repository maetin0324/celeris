#!/usr/bin/env python3
"""Local scripted ACP harness for scripts/dev/browser-web-live-check.sh (no LLM, no network).

Rerun 5 (2026-10-06): follows the task objective written by the script at f5b9dc4e:
"Open <page>, take a snapshot, read #inside, then click #forbidden-link using its @e ref".
The page is <p id="inside">inside</p><a id="forbidden-link" href=...>forbidden</a>.
The harness opens the allowed loopback page, snapshots it, reads #inside (extract by its @e
ref when the snapshot gives the paragraph a ref, otherwise from the snapshot text), clicks the
link named "forbidden" by its snapshot @e ref so Chrome itself follows it to the out-of-scope
origin (the harness never opens the forbidden URL with the wrapper), then poll a read-only action every few seconds for
REAL_CHECK_HOLD_SECS so the run stays RUNNING while the check takes/returns the control lease.
Every shim call (time, args, exit, output tail) is logged.
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
HOLD = int(os.environ.get("REAL_CHECK_HOLD_SECS", "240"))
LINK_RE = re.compile(r'link "forbidden"')
INSIDE_RE = re.compile(r'(?:paragraph|text|StaticText)[^\n]*"?inside"?')


def log(entry):
    entry = {"t": round(time.time(), 3), **entry}
    with LOG.open("a") as f:
        f.write(json.dumps(entry, ensure_ascii=False) + "\n")


def shim(cli, *args, timeout=150, keep=600):
    start = time.monotonic()
    try:
        r = subprocess.run([sys.executable, cli, *args], text=True, capture_output=True,
                           timeout=timeout, check=False)
        code, out, err = r.returncode, r.stdout, r.stderr
    except subprocess.TimeoutExpired:
        code, out, err = "timeout", "", ""
    log({"call": list(args), "exit": code, "secs": round(time.monotonic() - start, 2),
         "stdout_tail": out[-keep:], "stderr_tail": err[-600:]})
    return code, out


def flatten(text):
    """Snapshot output may be JSON with the tree as an escaped string; search both."""
    parts = [text]
    try:
        value = json.loads(text)
    except ValueError:
        return text
    def walk(v):
        if isinstance(v, str): parts.append(v)
        elif isinstance(v, dict): [walk(x) for x in v.values()]
        elif isinstance(v, list): [walk(x) for x in v]
    walk(value)
    return "\n".join(parts)


def link_ref(snapshot, pattern=None):
    pattern = pattern or LINK_RE
    for line in flatten(snapshot).splitlines():
        if pattern.search(line):
            m = re.search(r"(?:ref=|@)(e[0-9]+)", line)
            if m:
                return "@" + m.group(1)
    return None


def act(prompt):
    m = re.search(r'Use `python3 "((?:[^"\\]|\\.)+)" <command>`', prompt)
    if not m:
        log({"error": "browser shim path not found in prompt"})
        return False
    cli = json.loads('"' + m.group(1) + '"')
    log({"cli": cli})
    opened = clicked = None
    ref = None
    for attempt in range(8):
        opened, _ = shim(cli, "open", ALLOWED + "/")
        if opened != 0:
            time.sleep(3)
            continue
        snap_code, snap = shim(cli, "snapshot", keep=3000)
        ref = link_ref(snap) if snap_code == 0 else None
        if ref is None:
            log({"attempt": attempt, "snapshot_exit": snap_code, "link_ref": None})
            time.sleep(3)
            continue
        inside_ref = link_ref(snap, INSIDE_RE)
        if inside_ref:
            ex_code, ex_out = shim(cli, "extract", inside_ref)
            log({"read_inside": "extract", "ref": inside_ref, "exit": ex_code, "has_inside": "inside" in ex_out})
        else:
            log({"read_inside": "snapshot_text", "has_inside": "inside" in flatten(snap)})
        clicked, _ = shim(cli, "click", ref)
        log({"attempt": attempt, "link_ref": ref, "click_exit": clicked})
        if clicked == 0:
            break
        time.sleep(3)
    after_code, after = shim(cli, "snapshot", keep=1500)
    log({"summary": {"allowed_open_exit": opened, "link_ref": ref, "click_exit": clicked,
                     "snapshot_after_click_exit": after_code}})
    deadline = time.monotonic() + HOLD
    while time.monotonic() < deadline:
        shim(cli, "snapshot", timeout=30, keep=200)
        time.sleep(3)
    return opened == 0 and clicked == 0


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
