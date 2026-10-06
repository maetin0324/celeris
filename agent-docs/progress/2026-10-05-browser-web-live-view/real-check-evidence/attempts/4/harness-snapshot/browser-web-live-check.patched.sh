#!/usr/bin/env bash
# Opt-in host check. See docs/ops/browser-web-live-check.md before running it.
set -euo pipefail

if [[ ${CELERIS_BROWSER_REAL_CHECK:-} != 1 || ${CELERIS_USERNS_TESTS:-} != 1 ]]; then
  exit 2
fi
umask 077

ROOT=/local/celeris/data/workspaces/01M46W97H391DSFW1XJ745W0G9/repos/agent-platform  # PATCH(fable): copy runs outside the tree
EVIDENCE=${CELERIS_BROWSER_EVIDENCE_DIR:?set CELERIS_BROWSER_EVIDENCE_DIR}
LAUNCHER_CONFIG=${CELERIS_BROWSER_TEST_LAUNCHER_CONFIG:?set CELERIS_BROWSER_TEST_LAUNCHER_CONFIG}
DAEMON_CONFIG=${CELERIS_BROWSER_TEST_DAEMON_CONFIG:?set CELERIS_BROWSER_TEST_DAEMON_CONFIG}
LAUNCHER_BIN=${CELERIS_BROWSER_TEST_LAUNCHER_BIN:?set CELERIS_BROWSER_TEST_LAUNCHER_BIN}
DAEMON_BIN=${CELERIS_BROWSER_TEST_DAEMON_BIN:?set CELERIS_BROWSER_TEST_DAEMON_BIN}
LAUNCHER_USER=${CELERIS_BROWSER_TEST_LAUNCHER_USER:?set CELERIS_BROWSER_TEST_LAUNCHER_USER}
WEB_PASSWORD_FILE=${CELERIS_WEB_PASSWORD_FILE:?set CELERIS_WEB_PASSWORD_FILE}
OWNER_SOCKET=${CELERIS_WEB_OWNER_SOCKET:?set CELERIS_WEB_OWNER_SOCKET}
ATTESTATION_KEY=${CELERIS_WEB_ATTESTATION_KEY_FILE:?set CELERIS_WEB_ATTESTATION_KEY_FILE}
CONFORMANCE=${CELERIS_BROWSER_CONFORMANCE_FILE:?set CELERIS_BROWSER_CONFORMANCE_FILE}
DENIAL_FILE=${CELERIS_BROWSER_TEST_DENIAL_FILE:?set CELERIS_BROWSER_TEST_DENIAL_FILE}

[[ $EVIDENCE = /* && $LAUNCHER_CONFIG = /* && $DAEMON_CONFIG = /* ]] || {
  echo "evidence and config paths must be absolute" >&2; exit 1;
}
[[ $(id -u) != 0 ]] || { echo "run as the test daemon user, not root" >&2; exit 1; }
[[ $(id -un) != "$LAUNCHER_USER" ]] || { echo "launcher must use another UID" >&2; exit 1; }

# Both configs are intentionally supplied by the host operator. The launcher rejects any
# config that is not root-owned; the daemon config contains a local ACP harness and an
# isolated test DB. Check the paths before starting either process.
python3 - "$LAUNCHER_CONFIG" "$DAEMON_CONFIG" "$EVIDENCE" "$OWNER_SOCKET" "$DENIAL_FILE" "$WEB_PASSWORD_FILE" "$ATTESTATION_KEY" <<'PY'
import os, pathlib, sys, tomllib
launcher, daemon, evidence, owner, denial, password, key = map(pathlib.Path, sys.argv[1:])
for path in (launcher, daemon):
    if not path.is_file(): raise SystemExit(f"missing config: {path}")
l = tomllib.loads(launcher.read_text())
d = tomllib.loads(daemon.read_text())
if launcher.stat().st_uid != 0 or launcher.stat().st_mode & 0o022:
    raise SystemExit("launcher config must be root-owned and not writable by group/other")
if launcher.parent.stat().st_uid != 0 or launcher.parent.stat().st_mode & 0o022:
    raise SystemExit("launcher config directory must be root-owned and not writable by group/other")
if d.get("browser", {}).get("runtime") != "launcher":
    raise SystemExit("test daemon must use browser.runtime=launcher")
if pathlib.Path(d["browser"]["launcher_socket"]).resolve() != pathlib.Path(l["socket"]).resolve():
    raise SystemExit("daemon and launcher sockets differ")
for path in (daemon, d["db"], d["workspace_root"], l["socket"], l["state_dir"],
             d["api"]["token_file"], d["api"]["browser_attestation_public_key_file"],
             d["api"]["browser_credentiald_control_socket"], owner, denial, password, key):
    p = pathlib.Path(path)
    if not p.is_absolute() or not p.resolve().is_relative_to(pathlib.Path(evidence).resolve()):
        raise SystemExit(f"test path outside evidence dir: {p}")
if pathlib.Path(evidence).resolve().is_relative_to(pathlib.Path.home().resolve()):
    raise SystemExit("evidence dir must not be inside the production home")
if pathlib.Path(evidence).resolve() == pathlib.Path("/tmp"):
    raise SystemExit("use a dedicated evidence directory")
if d.get("api", {}).get("listen", "").split(":")[0] != "127.0.0.1":
    raise SystemExit("test API must bind 127.0.0.1")
if d["api"]["listen"].endswith(":7710") or d["api"]["listen"].endswith(":7700"):
    raise SystemExit("production API ports are forbidden")
if os.getuid() not in l.get("allowed_uids", []):
    raise SystemExit("launcher allowed_uids must include the test daemon UID")
if pathlib.Path(l["socket"]).exists():
    raise SystemExit("test launcher socket already exists; refusing to reuse another process")
PY

mkdir -p "$EVIDENCE"
# The launcher needs traverse permission; logs remain 0600 under umask 077.
chmod 711 "$EVIDENCE"
mkdir -p "$(dirname "$OWNER_SOCKET")"
chmod 700 "$(dirname "$OWNER_SOCKET")"
for f in "$LAUNCHER_BIN" "$DAEMON_BIN" "$WEB_PASSWORD_FILE" "$ATTESTATION_KEY" "$CONFORMANCE"; do
  [[ -f $f ]] || { echo "missing prerequisite: $f" >&2; exit 1; }
done
if [[ -e $DENIAL_FILE ]]; then echo "denial evidence already exists" >&2; exit 1; fi

API_PORT=$(python3 - "$DAEMON_CONFIG" <<'PY'
import sys,tomllib
print(tomllib.load(open(sys.argv[1],"rb"))["api"]["listen"].rsplit(":",1)[1])
PY
)
WEB_PORT=${CELERIS_BROWSER_TEST_WEB_PORT:-17729}
PAGE_PORT=${CELERIS_BROWSER_TEST_PAGE_PORT:-17730}
DENIED_PORT=${CELERIS_BROWSER_TEST_DENIED_PORT:-17731}
[[ $WEB_PORT =~ ^[0-9]+$ && $PAGE_PORT =~ ^[0-9]+$ && $DENIED_PORT =~ ^[0-9]+$ ]] || { echo "invalid test port" >&2; exit 1; }
[[ $WEB_PORT != 7710 && $WEB_PORT != 7700 && $PAGE_PORT != 7710 && $PAGE_PORT != 7700 && $DENIED_PORT != 7710 && $DENIED_PORT != 7700 ]] || {
  echo "production ports are forbidden" >&2; exit 1;
}
[[ $WEB_PORT != "$PAGE_PORT" && $WEB_PORT != "$DENIED_PORT" && $PAGE_PORT != "$DENIED_PORT" && $API_PORT != "$WEB_PORT" && $API_PORT != "$PAGE_PORT" && $API_PORT != "$DENIED_PORT" ]] || {
  echo "test ports must differ" >&2; exit 1;
}
API="http://127.0.0.1:$API_PORT/api/v1"
WEB="http://127.0.0.1:$WEB_PORT"
PAGE="http://127.0.0.1:$PAGE_PORT"
DENIED_ORIGIN="http://127.0.0.1:$DENIED_PORT"
TOKEN_FILE=$(python3 - "$DAEMON_CONFIG" <<'PY'
import sys,tomllib
print(tomllib.load(open(sys.argv[1],"rb"))["api"]["token_file"])
PY
)
[[ -f $TOKEN_FILE ]] || { echo "missing isolated API token" >&2; exit 1; }

cleanup() {
  for pid in "${WEB_PID:-}" "${DAEMON_PID:-}" "${LAUNCHER_PID:-}" "${PAGE_PID:-}" "${DENIED_PID:-}"; do
    if [[ -n $pid ]]; then kill -TERM -- "-$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; fi
  done
}
trap cleanup EXIT

# The fixture launcher and daemon are separate processes. Nothing here starts/stops a
# systemd unit or connects to a production socket.
setsid sudo -n python3 - "$LAUNCHER_USER" "$LAUNCHER_CONFIG" "$LAUNCHER_BIN" "$(id -u)" <<'PY' >"$EVIDENCE/launcher.log" 2>&1 &
import os, pathlib, pwd, socket, sys, tomllib
user, config, binary, daemon_uid = sys.argv[1:]
cfg = tomllib.load(open(config, "rb"))
path = pathlib.Path(cfg["socket"])
sock = socket.socket(socket.AF_UNIX)
sock.bind(str(path))
os.chown(path, int(daemon_uid), -1)
os.chmod(path, 0o600)
sock.listen(128)
os.dup2(sock.fileno(), 3, inheritable=True)
os.set_inheritable(3, True)  # PATCH(fable): dup2(3,3) keeps CLOEXEC when the socket is already fd 3
account = pwd.getpwnam(user)
os.setgroups([])
os.setgid(account.pw_gid)
os.setuid(account.pw_uid)
os.environ["LISTEN_FDS"] = "1"
os.environ["LISTEN_PID"] = str(os.getpid())
os.execv(binary, [binary, "--config", config])
PY
LAUNCHER_PID=$!
LAUNCHER_SOCKET=$(python3 - "$LAUNCHER_CONFIG" <<'PY'
import sys,tomllib
print(tomllib.load(open(sys.argv[1],"rb"))["socket"])
PY
)
for _ in {1..100}; do
  [[ -S $LAUNCHER_SOCKET ]] && break
  kill -0 "$LAUNCHER_PID" 2>/dev/null || { echo "launcher exited" >&2; exit 1; }
  sleep 0.1
done

setsid "$DAEMON_BIN" --config "$DAEMON_CONFIG" --log-format text >"$EVIDENCE/daemon.log" 2>&1 &
DAEMON_PID=$!
for _ in {1..100}; do
  if curl -fsS "$API/health" >"$EVIDENCE/health.json" 2>/dev/null; then break; fi
  kill -0 "$DAEMON_PID" 2>/dev/null || { echo "daemon exited" >&2; exit 1; }
  sleep 0.2
done
[[ -s $EVIDENCE/health.json ]] || { echo "test daemon health timeout" >&2; exit 1; }

mkdir -p "$EVIDENCE/page"
printf '<!doctype html><title>Celeris browser check</title><p id="inside">inside</p>\n' > "$EVIDENCE/page/index.html"
setsid python3 -m http.server "$PAGE_PORT" --bind 127.0.0.1 --directory "$EVIDENCE/page" >"$EVIDENCE/page.log" 2>&1 &
PAGE_PID=$!
for _ in {1..100}; do
  if curl -fsS "$PAGE/" >"$EVIDENCE/page-response.html" 2>/dev/null; then break; fi
  kill -0 "$PAGE_PID" 2>/dev/null || { echo "test page exited" >&2; exit 1; }
  sleep 0.1
done
[[ -s $EVIDENCE/page-response.html ]] || { echo "test page readiness timeout" >&2; exit 1; }
mkdir -p "$EVIDENCE/denied-page"
printf '<!doctype html><title>Forbidden origin</title>\n' > "$EVIDENCE/denied-page/index.html"
setsid python3 -u -m http.server "$DENIED_PORT" --bind 127.0.0.1 --directory "$EVIDENCE/denied-page" >"$EVIDENCE/denied-page.log" 2>&1 &
DENIED_PID=$!
for _ in {1..100}; do
  grep -q 'Serving HTTP' "$EVIDENCE/denied-page.log" 2>/dev/null && break
  kill -0 "$DENIED_PID" 2>/dev/null || { echo "denied origin server exited" >&2; exit 1; }
  sleep 0.1
done
grep -q 'Serving HTTP' "$EVIDENCE/denied-page.log" || { echo "denied origin server readiness timeout" >&2; exit 1; }

CELERIS_WEB_BIND="127.0.0.1:$WEB_PORT" \
CELERIS_API_URL="http://127.0.0.1:$API_PORT" \
CELERIS_API_TOKEN_FILE="$TOKEN_FILE" \
CELERIS_WEB_LIVE_VIEW_UPSTREAM="127.0.0.1:$PAGE_PORT" \
setsid node "$ROOT/web/server/index.js" >"$EVIDENCE/web.log" 2>&1 &
WEB_PID=$!
for _ in {1..100}; do
  if curl -fsS "$WEB/healthz" >"$EVIDENCE/web-health.json" 2>/dev/null; then break; fi
  kill -0 "$WEB_PID" 2>/dev/null || { echo "web exited" >&2; exit 1; }
  sleep 0.2
done
[[ -s $EVIDENCE/web-health.json && -S $OWNER_SOCKET ]] || { echo "web gateway readiness timeout" >&2; exit 1; }

# Use one deterministic driver for all API assertions. It writes only redacted JSON.
python3 - "$ROOT" "$EVIDENCE" "$API" "$WEB" "$PAGE" "$DENIED_ORIGIN" "$TOKEN_FILE" "$OWNER_SOCKET" "$WEB_PASSWORD_FILE" <<'PY'
import http.cookiejar, json, pathlib, socket, sys, time, urllib.error, urllib.request
root, evidence, api, web, page, denied_origin, token_file, owner_socket, password_file = sys.argv[1:]
token = pathlib.Path(token_file).read_text().strip()
out = pathlib.Path(evidence)
log = []
jar = http.cookiejar.CookieJar()
client = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))
def request(url, method="GET", body=None, *, origin=None, auth=False):
    headers = {"Accept":"application/json"}
    if auth: headers["Authorization"] = "Bearer " + token
    if origin: headers["Origin"] = origin
    if body is not None:
        body = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(url, data=body, headers=headers, method=method)
    try:
        with client.open(req, timeout=10) as resp:
            raw, status = resp.read(), resp.status
    except urllib.error.HTTPError as error:
        raw, status = error.read(), error.code
    try: value = json.loads(raw)
    except ValueError: value = raw.decode(errors="replace")
    return status, value
def checked(label, url, method="GET", body=None, *, origin=None, auth=False, expected=200):
    status, value = request(url, method, body, origin=origin, auth=auth)
    log.append({"step": label, "status": status, "expected": expected})
    if status != expected: raise AssertionError(f"{label}: HTTP {status}, wanted {expected}: {value}")
    return value
try:
    org = json.loads((pathlib.Path(root)/"docs/ops/browser-department-org.json").read_text())
    org["profile"]["browser"]["allowed_domains"] = [page]
    nodes = checked("org list", api+"/org", auth=True)["items"]
    if any(node["id"] == "browser-execution" for node in nodes):
        checked("org PATCH", api+"/org/browser-execution", "PATCH", {"profile":org["profile"]}, auth=True)
    else:
        checked("org POST", api+"/org", "POST", org, auth=True, expected=201)
    body = {"title":"loopback browser live check", "objective":f"Visit {page}, read #inside, attempt navigation to {denied_origin} and report that egress denied it; then wait for this check to finish.", "skills":["browser-enabled"], "genre":"coding", "requirements":{"browser":{"allowed_domains":[page]}}, "acceptance":[{"type":"reviewer","text":"loopback browser check completed"}]}
    task = checked("task POST", api+"/tasks", "POST", body, auth=True, expected=201)
    task_id = task["id"]
    # PATCH(fable, diagnostic): dispatch refuses browser runs without a stored task policy (browser_policy_required).
    checked("task browser policy PUT", api+f"/tasks/{task_id}/browser/policy", "PUT", {"policy_id":"real-check-loopback","revision":1,"domain_mode":"common_hosts","network_domains":[page],"allowed_actions":["navigate","snapshot","extract","click","scroll"],"approval_actions":[],"credential_policy_ids":[]}, auth=True)
    log.append({"task_id":task_id,"allowed_domains":[page]})
    password = pathlib.Path(password_file).read_text().strip()
    checked("web login", web+"/login", "POST", {"password":password}, origin=web)
    challenge = checked("owner challenge", web+"/browser/owner-session", "POST", {}, origin=web)["challenge"]
    with socket.socket(socket.AF_UNIX) as sock:
        sock.settimeout(5); sock.connect(owner_socket)
        sock.sendall((json.dumps({"op":"approve","challenge":challenge})+"\n").encode())
        if json.loads(sock.recv(1024))["ok"] is not True: raise AssertionError("owner approval denied")
    owner = checked("owner session", web+"/browser/owner-session")
    if not owner["isOwner"] or not owner["csrfToken"]: raise AssertionError("owner session absent")
    csrf = owner["csrfToken"]
    deadline = time.monotonic()+120
    while True:
        status, runs = request(web+f"/browser/runs?task_id={task_id}")
        if status == 200 and (items := runs.get("items", [])) and any(r["state"] == "RUNNING" for r in items): break
        if time.monotonic() >= deadline: raise AssertionError(f"browser run did not start: {status} {runs}")
        time.sleep(0.5)
    run = next(r for r in items if r["state"] == "RUNNING")
    run_id, session_id = run["run_id"], run["session_id"]
    if run.get("live_path") != f"/browser/live/{task_id}/{run_id}" or any(r.get("live_view_url") for r in items):
        raise AssertionError("unsafe live path or raw URL")
    checked("live view", web+run["live_path"])
    control_url = web+f"/browser/control/{task_id}/{run_id}/{session_id}"
    state = checked("control status", control_url)["status"]
    control = checked("takeover", control_url, "POST", {"csrf":csrf,"expected_version":state["version"],"idempotency_key":"real-check-takeover","command":{"kind":"takeover","ttl_secs":60}}, origin=web)["status"]
    if control["phase"] not in ("paused","human_control"): raise AssertionError("lease not acquired")
    checked("release", control_url+"/release", "POST", {"csrf":csrf}, origin=web)
    # Open a trusted, non-credential decision wait on the real run. This is independent
    # of when the local ACP harness decides to request human input.
    wait_body = {"run_id":run_id,"session_id":session_id,"reason":"waiting_for_approval",
      "origin":page,"purpose":"real-check decision route","operation":{"intent_id":"real-check-intent","action":"navigate"},
      "policy_revision":1,"policy_hash":"real-check-policy","owner_id":"owner","resume_key":"real-check-wait"}
    wait = checked("wait open", api+f"/tasks/{task_id}/browser/requests", "POST", wait_body, auth=True, expected=201)["wait"]
    listed = checked("wait listed", api+f"/tasks/{task_id}/browser/waits", auth=True)["items"]
    if not any(w["wait_id"] == wait["wait_id"] and w["state"] == "pending" for w in listed):
        raise AssertionError("wait missing from task")
    checked("decision deny", web+f"/browser/waits/{wait['wait_id']}/decision", "POST", {"csrf":csrf,"task_id":task_id,"expected_version":wait["version"],"decision":"deny"}, origin=web)
    log.append({"run_id":run_id,"session_id":session_id,"wait_id":wait["wait_id"],"result":"api_passed"})
finally:
    (out/"checks.json").write_text(json.dumps(log, indent=2, ensure_ascii=False)+"\n")
PY

python3 - "$DENIAL_FILE" "$PAGE" "$DENIED_ORIGIN" <<'PY'
import json, pathlib, sys, time
path, allowed, denied = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3]
deadline = time.monotonic() + 30
while not path.is_file() and time.monotonic() < deadline:
    time.sleep(0.2)
if not path.is_file(): raise SystemExit("browser harness did not produce egress denial evidence")
record = json.loads(path.read_text())
if record != {"allowed_origin": allowed, "attempted_origin": denied, "denied": True, "source": "agent-browser"}:
    raise SystemExit("egress denial evidence did not match the local fixture")
PY
if grep -q '"GET ' "$EVIDENCE/denied-page.log"; then
  echo "forbidden origin received a page request" >&2
  exit 1
fi
python3 - "$EVIDENCE/checks.json" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1])
checks = json.loads(path.read_text())
checks.append({"step":"browser egress", "denied":True, "forbidden_page_requests":0, "result":"passed"})
path.write_text(json.dumps(checks, indent=2, ensure_ascii=False)+"\n")
PY
