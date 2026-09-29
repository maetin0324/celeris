#!/usr/bin/env python3
"""Scripted LLM speaking the actual ACP or Claude CLI protocol for P4-C."""

import json
import os
from pathlib import Path
import subprocess
import sys


def act():
    runner = Path(__file__).with_name("browser-conformance.py")
    result = subprocess.run(
        [sys.executable, str(runner), "--drive", "--output-dir", os.environ["CELERIS_BROWSER_RUNTIME"]],
        check=False,
    )
    return result.returncode == 0


def main():
    backend = os.environ["CELERIS_BROWSER_BACKEND"]
    if backend in ("acp", "browser-specialist"):
        for line in sys.stdin:
            request = json.loads(line)
            method = request.get("method")
            if method == "initialize":
                result = {"protocolVersion": 1, "agentCapabilities": {}}
            elif method == "session/new":
                result = {"sessionId": "p4c-scripted", "configOptions": []}
            elif method == "session/prompt":
                if not act():
                    return 1
                Path("artifacts").mkdir(exist_ok=True)
                Path("artifacts/result.json").write_text('{"summary":"fixture passed","evidence":[]}')
                result = {"stopReason": "end_turn"}
            else:
                continue
            print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
            if method == "session/prompt":
                return 0
        return 1
    if backend == "claude-code":
        if not act():
            return 1
        Path("artifacts").mkdir(exist_ok=True)
        Path("artifacts/result.json").write_text('{"summary":"fixture passed","evidence":[]}')
        print('{"type":"result","subtype":"success","is_error":false}', flush=True)
        return 0
    return 2


if __name__ == "__main__":
    sys.exit(main())
