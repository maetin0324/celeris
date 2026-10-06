#!/bin/sh
# Phase 5 shadow-check: start routellm_sidecar.py with the fake classifier on a loopback ephemeral
# port, send the Rust fixture request, validate the responses with task-core
# (crates/celeris/tests/routing_estimator_sidecar_validate.rs), then SIGTERM and confirm the PID
# exited and the port is closed. No arguments. No network beyond 127.0.0.1. Exit 0 only if all pass.
# The fake classifier never stands in for the real RouteLLM run (real-sidecar-check.sh, human step).
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
fixtures="$repo/crates/task-core/tests/fixtures/estimator_sidecar_v1"
[ "$#" -eq 0 ] || { echo 'usage: fake-shadow-check.sh (no arguments)' >&2; exit 2; }

work=$(mktemp -d)
pid=''
cleanup() {
  if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
    kill -TERM "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT INT TERM

python3 "$here/routellm_sidecar.py" --host 127.0.0.1 --port 0 --router bert \
  --weights-dir /missing-weights-fake-only --strong model-a --weak model-b --fake-classifier \
  >"$work/stdout" 2>"$work/stderr" &
pid=$!

# Wait for the READY line (the process writes it once the socket is bound).
port=''
i=0
while [ "$i" -lt 300 ]; do
  line=$(head -n 1 "$work/stdout" 2>/dev/null || true)
  case "$line" in
    'READY port='*) port=${line#READY port=}; break ;;
  esac
  kill -0 "$pid" 2>/dev/null || { echo 'sidecar exited before READY' >&2; cat "$work/stderr" >&2; exit 1; }
  sleep 0.1
  i=$((i + 1))
done
[ -n "$port" ] || { echo 'sidecar did not print READY' >&2; exit 1; }
echo "fake sidecar pid=$pid port=$port"

mkdir -p "$work/check"
python3 - "$port" "$fixtures/request_valid.json" "$work/check" <<'PY'
import json, sys
from pathlib import Path
from urllib.request import Request, urlopen

port, fixture, out = sys.argv[1], Path(sys.argv[2]), Path(sys.argv[3])
base = f"http://127.0.0.1:{port}"
with urlopen(base + "/healthz", timeout=5) as r:
    health = json.load(r)
assert health["status"] == "ready", health
descriptor = {k: health[k] for k in
              ("estimator_id", "version", "protocol_version", "needs_prompt", "dependencies")}
(out / "descriptor.json").write_text(json.dumps(descriptor))

request = json.loads(fixture.read_text())
with_prompt = dict(request, request_id="req-2", optional_prompt="fixture prompt for the fake classifier")
for n, req in enumerate([request, with_prompt]):
    body = json.dumps(req).encode()
    http = Request(base + "/estimate", body, {"Content-Type": "application/json"})
    with urlopen(http, timeout=5) as r:
        assert r.status == 200, r.status
        resp = json.load(r)
    (out / f"request-{n}.json").write_text(json.dumps(req))
    (out / f"response-{n}.json").write_text(json.dumps(resp))
    reasons = sorted({x for e in resp["estimates"] for x in e["reasons"]})
    print(f"estimate {n}: request_id={resp['request_id']} reasons={reasons}")
PY

# task-core validation of the real-process responses (Rust side).
CELERIS_SIDECAR_CHECK_DIR="$work/check" cargo test -q -p celeris \
  --test routing_estimator_sidecar_validate -- --ignored --exact \
  fake_sidecar_response_passes_task_core_validation --nocapture

# SIGTERM: the process exits 0, the PID is gone and the port no longer accepts connections.
kill -TERM "$pid"
status=0
wait "$pid" || status=$?
[ "$status" -eq 0 ] || { echo "sidecar exit status $status after SIGTERM" >&2; cat "$work/stderr" >&2; exit 1; }
if kill -0 "$pid" 2>/dev/null; then echo "pid $pid still alive" >&2; exit 1; fi
pid=''
python3 - "$port" <<'PY'
import socket, sys
with socket.socket() as s:
    s.settimeout(1)
    rc = s.connect_ex(("127.0.0.1", int(sys.argv[1])))
assert rc != 0, "port still open after SIGTERM"
PY
echo "fake-shadow-check: ok (validated, SIGTERM exit 0, pid gone, port $port closed)"
