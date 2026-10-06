#!/usr/bin/env bash
# Opt-in host check. See docs/ops/browser-web-live-check.md before running it.
set -euo pipefail

if [[ ${CELERIS_BROWSER_REAL_CHECK:-} != 1 || ${CELERIS_USERNS_TESTS:-} != 1 ]]; then
  exit 2
fi
umask 077

ROOT=/local/celeris/data/workspaces/01M46W97H391DSFW1XJ745W0G9/repos/agent-platform  # PATCH(fable): copy runs outside the tree
# Defaults live under /var/tmp, never /tmp: the launcher covers /tmp with a per-session tmpfs,
# so a socket, state_dir or bwrap path under /tmp disappears inside the session (D3).
# Keep the evidence dir name short: the run workspace lives below it.
EVIDENCE=${CELERIS_BROWSER_EVIDENCE_DIR:-/var/tmp/cb-$(id -u)}
LAUNCHER_CONFIG=${CELERIS_BROWSER_TEST_LAUNCHER_CONFIG:-/var/tmp/celeris-browser-config-$(id -u)/launcher.toml}
DAEMON_CONFIG=${CELERIS_BROWSER_TEST_DAEMON_CONFIG:-$EVIDENCE/daemon.toml}
LAUNCHER_BIN=${CELERIS_BROWSER_TEST_LAUNCHER_BIN:?set CELERIS_BROWSER_TEST_LAUNCHER_BIN}
DAEMON_BIN=${CELERIS_BROWSER_TEST_DAEMON_BIN:?set CELERIS_BROWSER_TEST_DAEMON_BIN}
LAUNCHER_USER=${CELERIS_BROWSER_TEST_LAUNCHER_USER:-celeris-browser}
WEB_PASSWORD_FILE=${CELERIS_WEB_PASSWORD_FILE:-$EVIDENCE/web.password}
OWNER_SOCKET=${CELERIS_WEB_OWNER_SOCKET:-$EVIDENCE/web-private/owner.sock}
ATTESTATION_KEY=${CELERIS_WEB_ATTESTATION_KEY_FILE:-$EVIDENCE/web-private/attestation.key}
CONFORMANCE=${CELERIS_BROWSER_CONFORMANCE_FILE:-$EVIDENCE/conformance.json}
DENIAL_FILE=${CELERIS_BROWSER_TEST_DENIAL_FILE:-$EVIDENCE/egress-denied.json}
# The decision wait needs a canonical exact HTTPS origin (task-core valid_exact_origin). It is
# only the subject of the trusted API decision route: nothing navigates to it, and `.invalid`
# never resolves. The loopback http pages exist for the browser egress check only (D7).
DECISION_ORIGIN=${CELERIS_BROWSER_TEST_DECISION_ORIGIN:-https://real-check.celeris.invalid}

[[ $EVIDENCE = /* && $LAUNCHER_CONFIG = /* && $DAEMON_CONFIG = /* ]] || {
  echo "evidence and config paths must be absolute" >&2; exit 1;
}
[[ $DECISION_ORIGIN =~ ^https://[a-z0-9]([a-z0-9.-]*[a-z0-9])?(:[1-9][0-9]{0,4})?$ && $DECISION_ORIGIN != *:443 ]] || {
  echo "decision origin must be a canonical https://host[:port] (no path, no :443)" >&2; exit 1;
}
[[ $(id -u) != 0 ]] || { echo "run as the test daemon user, not root" >&2; exit 1; }
[[ $(id -un) != "$LAUNCHER_USER" ]] || { echo "launcher must use another UID" >&2; exit 1; }

# Both configs are intentionally supplied by the host operator. The launcher rejects any
# config that is not root-owned; the daemon config contains a local ACP harness and an
# isolated test DB. Check the paths before starting either process.
python3 - "$LAUNCHER_CONFIG" "$DAEMON_CONFIG" "$EVIDENCE" "$OWNER_SOCKET" "$DENIAL_FILE" "$WEB_PASSWORD_FILE" "$ATTESTATION_KEY" <<'PY'
import os, pathlib, sys, tomllib
launcher, daemon, evidence, owner, denial, password, key = map(pathlib.Path, sys.argv[1:])
TMP = pathlib.Path("/tmp").resolve()
def under_tmp(path):
    return pathlib.Path(path).resolve().is_relative_to(TMP)
for path in (launcher, launcher.parent, daemon, evidence):
    if under_tmp(path):
        raise SystemExit(f"{path} is under /tmp; the launcher hides /tmp per session, use /var/tmp")
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
launcher_paths = [l["socket"], l["state_dir"]] + [l[k] for k in ("session_root",) if k in l]
for path in [daemon, d["db"], d["workspace_root"], *launcher_paths,
             d["api"]["token_file"], d["api"]["browser_attestation_public_key_file"],
             d["api"]["browser_credentiald_control_socket"], owner, denial, password, key]:
    p = pathlib.Path(path)
    if not p.is_absolute() or not p.resolve().is_relative_to(pathlib.Path(evidence).resolve()):
        raise SystemExit(f"test path outside evidence dir: {p}")
    if under_tmp(p):
        raise SystemExit(f"test path under /tmp: {p}")
for key_name in ("bwrap", "sandboxd", "egress", "chrome", "agent_browser"):
    if key_name in l and under_tmp(l[key_name]):
        raise SystemExit(f"launcher {key_name} is under /tmp and is hidden inside the session")
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
LIVE_PORT=${CELERIS_BROWSER_TEST_LIVE_PORT:-17732}
for port in "$API_PORT" "$WEB_PORT" "$PAGE_PORT" "$DENIED_PORT" "$LIVE_PORT"; do
  [[ $port =~ ^[0-9]+$ ]] || { echo "invalid test port" >&2; exit 1; }
  [[ $port != 7710 && $port != 7700 ]] || { echo "production ports are forbidden" >&2; exit 1; }
done
[[ $(printf '%s\n' "$API_PORT" "$WEB_PORT" "$PAGE_PORT" "$DENIED_PORT" "$LIVE_PORT" | sort -u | wc -l) = 5 ]] || {
  echo "test ports must differ" >&2; exit 1;
}
API="http://127.0.0.1:$API_PORT/api/v1"
WEB="http://127.0.0.1:$WEB_PORT"
PAGE="http://127.0.0.1:$PAGE_PORT"
DENIED_ORIGIN="http://127.0.0.1:$DENIED_PORT"
LIVE_UPSTREAM="127.0.0.1:$LIVE_PORT"
TOKEN_FILE=$(python3 - "$DAEMON_CONFIG" <<'PY'
import sys,tomllib
print(tomllib.load(open(sys.argv[1],"rb"))["api"]["token_file"])
PY
)
[[ -f $TOKEN_FILE ]] || { echo "missing isolated API token" >&2; exit 1; }

cleanup() {
  for pid in "${WEB_PID:-}" "${DAEMON_PID:-}" "${LAUNCHER_PID:-}" "${PAGE_PID:-}" "${DENIED_PID:-}" "${LIVE_PID:-}"; do
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
# dup2(3, 3) is a no-op that keeps O_CLOEXEC when the socket already is fd 3 (D1).
os.set_inheritable(3, True)
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

# Live View upstream fixture: stands in for the dashboard behind the gateway. It serves an HTML
# entry and accepts the stream WebSocket, logging every client frame it receives, so the check
# can show that input without a control lease never reaches the upstream.
setsid python3 -u - "$LIVE_PORT" "$EVIDENCE/live-upstream-frames.jsonl" <<'PY' >"$EVIDENCE/live-upstream.log" 2>&1 &
import base64, hashlib, http.server, json, re, sys
port, frames = int(sys.argv[1]), sys.argv[2]
MAGIC = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
def read_exact(f, n):
    data = f.read(n)
    if len(data) != n: raise EOFError
    return data
class Live(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def do_GET(self):
        path = self.path.split("?")[0]
        if re.fullmatch(r"/api/session/\d{1,5}/stream", path) and self.headers.get("Upgrade", "").lower() == "websocket":
            self.close_connection = True
            accept = base64.b64encode(hashlib.sha1((self.headers["Sec-WebSocket-Key"] + MAGIC).encode()).digest()).decode()
            self.send_response(101)
            self.send_header("Upgrade", "websocket"); self.send_header("Connection", "Upgrade")
            self.send_header("Sec-WebSocket-Accept", accept); self.end_headers(); self.wfile.flush()
            try:
                while True:
                    b0, b1 = read_exact(self.rfile, 2)
                    n = b1 & 0x7F
                    if n == 126: n = int.from_bytes(read_exact(self.rfile, 2), "big")
                    elif n == 127: n = int.from_bytes(read_exact(self.rfile, 8), "big")
                    mask = read_exact(self.rfile, 4) if b1 & 0x80 else b"\0\0\0\0"
                    payload = bytes(c ^ mask[i % 4] for i, c in enumerate(read_exact(self.rfile, n)))
                    if b0 & 0x0F == 8: return
                    with open(frames, "a") as out:
                        out.write(json.dumps({"opcode": b0 & 0x0F, "payload": payload.decode(errors="replace")}) + "\n")
            except (EOFError, OSError):
                return
        body = b"<!doctype html><title>live fixture</title><p>live</p>\n" if path == "/" else b"{}"
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8" if path == "/" else "application/json")
        self.send_header("Content-Length", str(len(body))); self.end_headers(); self.wfile.write(body)
    def log_message(self, fmt, *args):
        sys.stderr.write("live-upstream " + (fmt % args) + "\n")
srv = http.server.ThreadingHTTPServer(("127.0.0.1", port), Live)
print("Serving live fixture", flush=True)
srv.serve_forever()
PY
LIVE_PID=$!
for _ in {1..100}; do
  grep -q 'Serving live fixture' "$EVIDENCE/live-upstream.log" 2>/dev/null && break
  kill -0 "$LIVE_PID" 2>/dev/null || { echo "live view fixture exited" >&2; exit 1; }
  sleep 0.1
done
grep -q 'Serving live fixture' "$EVIDENCE/live-upstream.log" || { echo "live view fixture readiness timeout" >&2; exit 1; }

CELERIS_WEB_BIND="127.0.0.1:$WEB_PORT" \
CELERIS_API_URL="http://127.0.0.1:$API_PORT" \
CELERIS_API_TOKEN_FILE="$TOKEN_FILE" \
CELERIS_WEB_LIVE_VIEW_UPSTREAM="$LIVE_UPSTREAM" \
setsid node "$ROOT/web/server/index.js" >"$EVIDENCE/web.log" 2>&1 &
WEB_PID=$!
for _ in {1..100}; do
  if curl -fsS "$WEB/healthz" >"$EVIDENCE/web-health.json" 2>/dev/null; then break; fi
  kill -0 "$WEB_PID" 2>/dev/null || { echo "web exited" >&2; exit 1; }
  sleep 0.2
done
[[ -s $EVIDENCE/web-health.json && -S $OWNER_SOCKET ]] || { echo "web gateway readiness timeout" >&2; exit 1; }

# Use one deterministic driver for all API assertions. It writes only redacted JSON.
python3 - "$ROOT" "$EVIDENCE" "$API" "$WEB" "$PAGE" "$DENIED_ORIGIN" "$TOKEN_FILE" "$OWNER_SOCKET" "$WEB_PASSWORD_FILE" "$DECISION_ORIGIN" <<'PY'
import base64, http.cookiejar, json, os, pathlib, socket, sys, time, urllib.error, urllib.parse, urllib.request
root, evidence, api, web, page, denied_origin, token_file, owner_socket, password_file, decision_origin = sys.argv[1:]
token = pathlib.Path(token_file).read_text().strip()
out = pathlib.Path(evidence)
log = []
jar = http.cookiejar.CookieJar()
client = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))
def request(url, method="GET", body=None, *, origin=None, auth=False, opener=None):
    headers = {"Accept":"application/json"}
    if auth: headers["Authorization"] = "Bearer " + token
    if origin: headers["Origin"] = origin
    if body is not None:
        body = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(url, data=body, headers=headers, method=method)
    try:
        with (opener or client).open(req, timeout=10) as resp:
            raw, status = resp.read(), resp.status
    except urllib.error.HTTPError as error:
        raw, status = error.read(), error.code
    try: value = json.loads(raw)
    except ValueError: value = raw.decode(errors="replace")
    return status, value
def checked(label, url, method="GET", body=None, *, origin=None, auth=False, expected=200, opener=None):
    status, value = request(url, method, body, origin=origin, auth=auth, opener=opener)
    log.append({"step": label, "status": status, "expected": expected})
    if status != expected: raise AssertionError(f"{label}: HTTP {status}, wanted {expected}: {value}")
    return value
anonymous = urllib.request.build_opener()  # no cookie jar: never logged in
def ws_frame(text):
    payload, mask = text.encode(), os.urandom(4)
    head = bytes([0x81, 0x80 | len(payload)]) if len(payload) < 126 else bytes([0x81, 0x80 | 126]) + len(payload).to_bytes(2, "big")
    return head + mask + bytes(c ^ mask[i % 4] for i, c in enumerate(payload))
def ws_read(sock):
    def exact(n):
        data = b""
        while len(data) < n:
            chunk = sock.recv(n - len(data))
            if not chunk: raise AssertionError("live stream closed before a reply")
            data += chunk
        return data
    b0, b1 = exact(2)
    n = b1 & 0x7F
    if n == 126: n = int.from_bytes(exact(2), "big")
    elif n == 127: n = int.from_bytes(exact(8), "big")
    return b0 & 0x0F, exact(n)
def ws_open(path, *, cookies=True):
    """Upgrade through the gateway like the Live View page does. Returns (status line, socket)."""
    url = urllib.parse.urlsplit(web)
    host = url.netloc
    cookie = "; ".join(f"{c.name}={c.value}" for c in jar) if cookies else ""
    key = base64.b64encode(os.urandom(16)).decode()
    sock = socket.create_connection((url.hostname, url.port), timeout=10)
    lines = [f"GET {path} HTTP/1.1", f"Host: {host}", f"Origin: {web}", "Connection: Upgrade", "Upgrade: websocket",
             "Sec-WebSocket-Version: 13", f"Sec-WebSocket-Key: {key}"] + ([f"Cookie: {cookie}"] if cookie else [])
    sock.sendall(("\r\n".join(lines) + "\r\n\r\n").encode())
    head = b""
    while b"\r\n\r\n" not in head:
        chunk = sock.recv(1)
        if not chunk: break
        head += chunk
    return head.split(b"\r\n", 1)[0].decode(errors="replace"), sock
def lease_less_input(label, stream_path):
    """Input without a human-control lease must be refused by the gateway and never reach upstream."""
    frames = out/"live-upstream-frames.jsonl"
    before = frames.read_text().count("input_mouse") if frames.exists() else 0
    status_line, sock = ws_open(stream_path)
    with sock:
        if " 101 " not in status_line + " ": raise AssertionError(f"{label}: stream upgrade refused: {status_line}")
        sock.sendall(ws_frame(json.dumps({"type":"input_mouse","event":"mousePressed","x":1,"y":1,"button":"left","clickCount":1})))
        deadline = time.monotonic()+10
        while True:
            opcode, payload = ws_read(sock)
            if opcode == 1:
                reply = json.loads(payload)
                if reply.get("type") == "input_denied": break
            if time.monotonic() >= deadline: raise AssertionError(f"{label}: no input_denied reply")
    time.sleep(0.5)
    after = frames.read_text().count("input_mouse") if frames.exists() else 0
    log.append({"step": label, "reply": reply, "upstream_input_frames": after - before})
    if reply.get("code") != "lease_required" or after != before:
        raise AssertionError(f"{label}: lease-less input was not refused: {reply}, forwarded={after - before}")
try:
    org = json.loads((pathlib.Path(root)/"docs/ops/browser-department-org.json").read_text())
    org["profile"]["browser"]["allowed_domains"] = [page]
    nodes = checked("org list", api+"/org", auth=True)["items"]
    if any(node["id"] == "browser-execution" for node in nodes):
        checked("org PATCH", api+"/org/browser-execution", "PATCH", {"profile":org["profile"]}, auth=True)
    else:
        checked("org POST", api+"/org", "POST", org, auth=True, expected=201)
    password = pathlib.Path(password_file).read_text().strip()
    checked("web login", web+"/login", "POST", {"password":password}, origin=web)
    # Settings edit through the gateway, the same route the web settings screen uses
    # (PATCH /api/v1/org/{id}/browser-settings). Widen, reject an invalid origin, then restore.
    settings = web+"/api/org/browser-execution/browser-settings"  # PATCH(fable): gateway maps /api/* to daemon /api/v1/*
    def grant_domains(node): return node["profile"]["browser"]["allowed_domains"]
    widened = checked("settings PATCH widen", settings, "PATCH", {"allowed_domains":[page, denied_origin]}, origin=web)
    if grant_domains(widened) != [page, denied_origin]: raise AssertionError(f"settings not applied: {grant_domains(widened)}")
    checked("settings PATCH invalid origin", settings, "PATCH", {"allowed_domains":["javascript:alert(1)"]}, origin=web, expected=422)
    restored = checked("settings PATCH restore", settings, "PATCH", {"allowed_domains":[page]}, origin=web)
    listed_nodes = checked("org list after settings", api+"/org", auth=True)["items"]
    stored = next(n for n in listed_nodes if n["id"] == "browser-execution")
    if grant_domains(restored) != [page] or grant_domains(stored) != [page]:
        raise AssertionError(f"settings restore not stored: {grant_domains(stored)}")
    log.append({"step":"settings edit", "allowed_domains":grant_domains(stored), "result":"passed"})
    body = {"title":"loopback browser live check", "objective":f"Visit {page}, read #inside, attempt navigation to {denied_origin} and report that egress denied it; then wait for this check to finish.", "skills":["browser-enabled"], "genre":"coding", "requirements":{"browser":{"allowed_domains":[page]}}, "acceptance":[{"type":"reviewer","text":"loopback browser check completed"}]}
    task = checked("task POST", api+"/tasks", "POST", body, auth=True, expected=201)
    task_id = task["id"]
    # Dispatch refuses a browser run without a stored task policy (browser_policy_required, D2).
    policy = {"policy_id":"real-check-loopback","revision":1,"domain_mode":"common_hosts",
              "network_domains":[page],"allowed_actions":["navigate","snapshot","extract","click","scroll"],
              "approval_actions":[],"credential_policy_ids":[]}
    checked("task browser policy PUT", api+f"/tasks/{task_id}/browser/policy", "PUT", policy, auth=True)
    stored_policy = checked("task browser policy GET", api+f"/tasks/{task_id}/browser/policy", auth=True)
    log.append({"task_id":task_id,"allowed_domains":[page],"policy":stored_policy})
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
    # Unauthenticated Live View: no session cookie, both the page and the stream upgrade.
    checked("live view unauthenticated", web+run["live_path"], opener=anonymous, expected=401)
    stream_path = run["live_path"]+"/api/session/9222/stream?last_seen=0"
    status_line, sock = ws_open(stream_path, cookies=False)
    # PATCH(fable diag2): drain the 401 reply before closing; an unread body makes close() send RST,
    # and the gateway then dies on an unhandled ECONNRESET (diag1). This only sidesteps that defect.
    try:
        sock.shutdown(socket.SHUT_WR)
        while sock.recv(4096): pass
    except OSError: pass
    sock.close()
    log.append({"step":"live stream unauthenticated", "status_line":status_line})
    if " 401 " not in status_line + " ": raise AssertionError(f"unauthenticated stream not refused: {status_line}")
    checked("live view", web+run["live_path"])
    lease_less_input("input without lease (agent running)", stream_path)
    control_url = web+f"/browser/control/{task_id}/{run_id}/{session_id}"
    state = checked("control status", control_url)["status"]
    # Takeover is only valid from paused (ControlPhase); pause first (D5).
    paused = checked("pause", control_url, "POST", {"csrf":csrf,"expected_version":state["version"],"idempotency_key":"real-check-pause","command":{"kind":"pause"}}, origin=web)["status"]
    log.append({"pause_phase":paused.get("phase"), "agent_phase_before":state.get("phase")})
    # An in-flight agent action leaves the run in `pausing` until it converges.
    deadline = time.monotonic()+30
    while paused.get("phase") == "pausing" and time.monotonic() < deadline:
        time.sleep(0.5)
        paused = request(control_url)[1].get("status", paused)
    if paused.get("phase") != "paused": raise AssertionError(f"pause did not reach paused: {paused.get('phase')}")
    control = checked("takeover", control_url, "POST", {"csrf":csrf,"expected_version":paused["version"],"idempotency_key":"real-check-takeover","command":{"kind":"takeover","ttl_secs":60}}, origin=web)["status"]
    if control["phase"] != "human_control": raise AssertionError(f"lease not acquired: {control['phase']}")
    checked("release", control_url+"/release", "POST", {"csrf":csrf}, origin=web)
    lease_less_input("input without lease (after release)", stream_path)
    # Open a trusted, non-credential decision wait on the real run. This is independent
    # of when the local ACP harness decides to request human input.
    wait_body = {"run_id":run_id,"session_id":session_id,"reason":"waiting_for_approval",
      "origin":decision_origin,"purpose":"real-check decision route","operation":{"intent_id":"real-check-intent","action":"navigate"},
      "policy_revision":1,"policy_hash":"real-check-policy","owner_id":"owner","resume_key":"real-check-wait"}
    wait = checked("wait open", api+f"/tasks/{task_id}/browser/requests", "POST", wait_body, auth=True, expected=201)["wait"]
    listed = checked("wait listed", api+f"/tasks/{task_id}/browser/waits", auth=True)["items"]
    if not any(w["wait_id"] == wait["wait_id"] and w["state"] == "pending" for w in listed):
        raise AssertionError("wait missing from task")
    checked("decision deny", web+f"/browser/waits/{wait['wait_id']}/decision", "POST", {"csrf":csrf,"task_id":task_id,"expected_version":wait["version"],"decision":"deny"}, origin=web)
    log.append({"run_id":run_id,"session_id":session_id,"wait_id":wait["wait_id"],"decision_origin":decision_origin,"result":"api_passed"})
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
