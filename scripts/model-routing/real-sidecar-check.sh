#!/bin/sh
# Opt-in real weights check. Exit 2 means NOT RUN, never a passing skip.
set -eu

if [ "${CELERIS_ROUTELLM_REAL:-}" != 1 ] || [ -z "${CELERIS_ROUTELLM_WEIGHTS_DIR:-}" ] || [ ! -d "$CELERIS_ROUTELLM_WEIGHTS_DIR" ]; then
  echo 'not run: set CELERIS_ROUTELLM_REAL=1 and CELERIS_ROUTELLM_WEIGHTS_DIR to approved local weights' >&2
  exit 2
fi

if [ "$#" -lt 1 ]; then
  echo 'usage: real-sidecar-check.sh start-stop | shadow --dataset <dir> --max-requests <n> --out <dir>' >&2
  exit 2
fi
mode=$1
shift
dataset=''
max_requests=''
out=''
while [ "$#" -gt 0 ]; do
  case "$1" in
    --dataset) dataset=${2:?}; shift 2 ;;
    --max-requests) max_requests=${2:?}; shift 2 ;;
    --out) out=${2:?}; shift 2 ;;
    *) echo "unexpected argument: $1" >&2; exit 2 ;;
  esac
done
case "$mode" in
  start-stop) [ -z "$dataset$max_requests$out" ] || exit 2 ;;
  shadow) [ -n "$dataset" ] && [ -n "$max_requests" ] && [ -n "$out" ] || exit 2 ;;
  *) echo "unknown mode: $mode" >&2; exit 2 ;;
esac

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
exec python3 - "$script_dir" "$mode" "$dataset" "$max_requests" "$out" <<'PY'
import hashlib
import json
import os
from pathlib import Path
import re
import select
import socket
import subprocess
import sys
import time
from urllib.request import Request, urlopen

script_dir, mode, dataset, max_requests, out = sys.argv[1:]
strong = os.environ.get("CELERIS_ROUTELLM_STRONG", "model-a")
weak = os.environ.get("CELERIS_ROUTELLM_WEAK", "model-b")
if strong == weak:
    raise SystemExit("strong and weak must differ")

files = []
if mode == "shadow":
    if not max_requests.isdecimal() or int(max_requests) < 1:
        raise SystemExit("max-requests must be a positive integer")
    source = Path(dataset)
    if not source.is_dir():
        raise SystemExit("dataset must be a directory")
    files = sorted(source.glob("*.json"))
    if not 1 <= len(files) <= int(max_requests):
        raise SystemExit("dataset count must be nonzero and no greater than max-requests")
    output = Path(out)
    if output.exists() and any(output.iterdir()):
        raise SystemExit("out directory must be empty")

command = [sys.executable, str(Path(script_dir) / "routellm_sidecar.py"),
           "--host", "127.0.0.1", "--port", "0", "--router", "bert",
           "--weights-dir", os.environ["CELERIS_ROUTELLM_WEIGHTS_DIR"],
           "--strong", strong, "--weak", weak]
process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                           text=True, bufsize=1)
port = None
results = []
failures = []
try:
    ready, _, _ = select.select([process.stdout], [], [], 120)
    if not ready:
        raise RuntimeError("sidecar did not become ready within 120s")
    line = process.stdout.readline().strip()
    match = re.fullmatch(r"READY port=(\d+)", line)
    if not match:
        raise RuntimeError(f"sidecar not ready: {line}")
    port = int(match.group(1))
    base = f"http://127.0.0.1:{port}"
    with urlopen(base + "/healthz", timeout=5) as response:
        health = json.load(response)
    if health.get("status") != "ready":
        raise RuntimeError("healthz was not ready")
    if mode == "start-stop":
        requests = [("start-stop", {"request_id": "start-stop", "context_features": {},
                     "candidates": [{"model_profile_id": strong}, {"model_profile_id": weak}],
                     "optional_prompt": "Route this short test prompt."})]
    else:
        requests = [(path.name, path) for path in files]
    for name, source in requests:
        try:
            payload = json.loads(source.read_text()) if mode == "shadow" else source
            started = time.monotonic()
            body = json.dumps(payload).encode()
            with urlopen(Request(base + "/estimate", body,
                                 {"Content-Type": "application/json"}), timeout=120) as response:
                result = json.load(response)
            elapsed_ms = round((time.monotonic() - started) * 1000, 2)
            estimates = result["estimates"]
            pair_score = next((reason.split("=", 1)[1]
                               for item in estimates if item["model_profile_id"] == strong
                               for reason in item["reasons"]
                               if reason.startswith("raw_pair_win_rate=")), None)
            if pair_score is None or not 0 <= float(pair_score) <= 1:
                raise RuntimeError("no finite pair score")
            results.append({"file": name, "request_id": result["request_id"],
                            "raw_pair_win_rate": float(pair_score), "elapsed_ms": elapsed_ms,
                            "sha256": hashlib.sha256(body).hexdigest()})
        except Exception as error:
            if mode == "start-stop":
                raise
            failures.append({"file": name, "error_type": type(error).__name__})
    print(f"real sidecar: ready; {len(results)} finite pair score(s), {len(failures)} failed")
finally:
    process.terminate()
    try:
        exit_code = process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()
        raise RuntimeError("sidecar did not stop after SIGTERM")
    if exit_code != 0:
        raise RuntimeError(f"sidecar exit code {exit_code}: {process.stderr.read()[-1000:]}")
    process.stdout.close()
    process.stderr.close()
    if port is not None:
        with socket.socket() as sock:
            sock.settimeout(1)
            if sock.connect_ex(("127.0.0.1", port)) == 0:
                raise RuntimeError("sidecar port remained open")

if mode == "shadow":
    output.mkdir(parents=True, exist_ok=True)
    report = {"mode": "shadow", "router": "bert", "strong": strong, "weak": weak,
              "dataset_count": len(files), "max_requests": int(max_requests),
              "completed": len(results), "failed": len(failures), "dropped": 0,
              "results": results, "failures": failures}
    (output / "real-sidecar-results.json").write_text(json.dumps(report, indent=2) + "\n")
print("real sidecar: stopped; port closed")
if failures or not results:
    raise SystemExit("real sidecar: incomplete shadow evaluation")
PY
