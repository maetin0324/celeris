#!/usr/bin/env python3
"""Real agent-browser 0.38.1 `auth login` through celeris-credentiald's plugin bridge.

Local-only evidence run (no network, no LLM). It starts a self-signed HTTPS login fixture on
127.0.0.1, a scratch celeris-credentiald (scratch HOME and XDG_RUNTIME_DIR, never the real
~/.config/celeris), registers a sentinel credential, then drives the pinned binary with
legacy diagnostic variants and, with --worker-settings, the worker-generated wiring:

  1 celeris-shape   pre-fix argv and upstream config from browser_credential.rs
  2 corrected-cli   upstream's documented argv/plugin-array shape; FD 3 delivered the Celeris way
                    (pipe on the `auth login` CLI process only). A diagnostic wrapper records
                    whether the plugin process actually sees FD 3.
  3 test-fd3        same as 2, but the wrapper (test-only) recreates the FD 3 pipe from a 0600
                    token file so the rest of the chain (broker, fill, submit) can be observed.

Every agent-browser stdout/stderr, the broker journal and the fixture state are saved under
--output and scanned for the password sentinel.

python3 scripts/browser-auth-login-check.py --agent-browser /abs/agent-browser-linux-x64 \
  --chromium /abs/chrome --credentiald /abs/celeris-credentiald --output /abs/artifacts/auth-login
"""
import argparse
import http.server
import json
import os
from pathlib import Path
import secrets
import shutil
import signal
import socket
import ssl
import subprocess
import tempfile
import threading
import time
import urllib.parse

PORT = 27901
ORIGIN = f"https://127.0.0.1:{PORT}"
PLUGIN = "celeris-credential"

WRAPPER = r'''#!/usr/bin/env python3
# Diagnostic plugin wrapper. Records shapes only (no credential values), then runs the bridge.
import json, os, subprocess, sys
diag, bridge, mode = sys.argv[1], sys.argv[2], sys.argv[3]
rec = {"argv_tail": sys.argv[4:], "env_names": sorted(os.environ),
       "xdg_runtime_dir_set": "XDG_RUNTIME_DIR" in os.environ}
try:
    rec["fd3_before"] = os.readlink("/proc/self/fd/3")
except OSError:
    rec["fd3_before"] = None
data = sys.stdin.buffer.read()
try:
    req = json.loads(data)
    rec["request"] = {"protocol": req.get("protocol"), "type": req.get("type"),
                      "capability": req.get("capability"),
                      "request_keys": sorted((req.get("request") or {}).keys()),
                      "profileName": (req.get("request") or {}).get("profileName"),
                      "url": (req.get("request") or {}).get("url")}
except ValueError:
    rec["request"] = "unparsable"
pass_fds = ()
if mode == "token-file":
    tok = sys.argv[4]
    r, w = os.pipe()
    with open(tok, "rb") as f:
        os.write(w, f.read())
    os.unlink(tok)
    os.close(w)
    os.dup2(r, 3)
    os.set_inheritable(3, True)
    pass_fds = (3,)
elif rec["fd3_before"] is not None:
    pass_fds = (3,)
p = subprocess.run([bridge, "bridge"], input=data, capture_output=True, pass_fds=pass_fds)
try:
    out = json.loads(p.stdout)
    rec["response"] = {"protocol": out.get("protocol"), "success": out.get("success"),
                       "credential_keys": sorted((out.get("credential") or {}).keys())}
except ValueError:
    rec["response"] = "unparsable"
rec["bridge_exit"] = p.returncode
rec["bridge_stderr"] = p.stderr.decode(errors="replace").strip()[:40]
with open(diag, "a") as f:
    f.write(json.dumps(rec) + "\n")
sys.stdout.buffer.write(p.stdout)
'''


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--agent-browser", required=True)
    ap.add_argument("--chromium", required=True)
    ap.add_argument("--credentiald", required=True)
    ap.add_argument("--output", type=Path, required=True)
    ap.add_argument("--worker-settings", type=Path,
                    help="task-worker test export: upstream.json, policy.json, auth-argv.json")
    args = ap.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    scratch = Path(tempfile.mkdtemp(prefix="authchk-"))
    os.chmod(scratch, 0o700)
    home, run, sock = scratch / "home", scratch / "run", scratch / "abs"
    for d in (home, run, sock):
        d.mkdir(mode=0o700)
    username = "authcheck-user"
    password = "SENTINEL-PW-" + secrets.token_hex(12)
    marker = "LOGIN-OK-" + secrets.token_hex(6)
    (scratch / "sentinel").write_text(password)  # only for the final scan; never copied to --output
    log = []

    def note(**kv):
        log.append(kv)
        print(json.dumps(kv)[:400], flush=True)

    # ---- HTTPS fixture (self-signed, loopback). It never logs request bodies.
    subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
                    "-subj", "/CN=127.0.0.1", "-addext", "subjectAltName=IP:127.0.0.1",
                    "-keyout", str(scratch / "key.pem"), "-out", str(scratch / "cert.pem")],
                   check=True, capture_output=True)
    fixture_state = {"posts": 0, "login_ok": 0, "login_bad": 0, "welcome_ok": 0}
    cookie = secrets.token_hex(8)

    class H(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def page(self, code, body, headers=()):
            b = body.encode()
            self.send_response(code)
            for k, v in headers:
                self.send_header(k, v)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(b)))
            self.end_headers()
            self.wfile.write(b)

        def do_GET(self):
            if self.path.startswith("/welcome"):
                if f"s={cookie}" in (self.headers.get("Cookie") or ""):
                    fixture_state["welcome_ok"] += 1
                    return self.page(200, f"<title>Welcome</title><p id=ok>{marker}</p>")
                return self.page(403, "<p>not logged in</p>")
            self.page(200, "<title>Login</title><form method=post action=/login>"
                      "<input type=text name=username id=username autocomplete=username>"
                      "<input type=password name=password id=password>"
                      "<button type=submit>Sign in</button></form>")

        def do_POST(self):
            n = int(self.headers.get("Content-Length") or 0)
            form = urllib.parse.parse_qs(self.rfile.read(n).decode())
            fixture_state["posts"] += 1
            if form.get("username") == [username] and form.get("password") == [password]:
                fixture_state["login_ok"] += 1
                return self.page(303, "", [("Location", "/welcome"), ("Set-Cookie", f"s={cookie}; Path=/")])
            fixture_state["login_bad"] += 1
            self.page(401, "<p>LOGIN-FAILED</p>")

    srv = http.server.ThreadingHTTPServer(("127.0.0.1", PORT), H)
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.load_cert_chain(scratch / "cert.pem", scratch / "key.pem")
    srv.socket = ctx.wrap_socket(srv.socket, server_side=True)
    threading.Thread(target=srv.serve_forever, daemon=True).start()

    # ---- scratch credentiald
    denv = {"PATH": os.environ.get("PATH", ""), "HOME": str(home), "XDG_RUNTIME_DIR": str(run)}
    for d in (home / ".config", home / ".config/celeris", home / ".local", home / ".local/celeris"):
        d.mkdir(mode=0o700, exist_ok=True)
    r = subprocess.run([args.credentiald, "init"], env=denv, capture_output=True, text=True)
    note(step="credentiald init", exit=r.returncode, stderr=r.stderr.strip())
    broker = subprocess.Popen([args.credentiald, "serve", str(os.getpid())], env=denv,
                              stdout=subprocess.DEVNULL, stderr=open(out / "credentiald.stderr", "w"))
    control = run / "celeris-credentiald/control.sock"
    for _ in range(100):
        if control.exists() and (run / "celeris-credentiald/resolve.sock").exists():
            break
        time.sleep(0.05)

    def ctl(req):
        s = socket.socket(socket.AF_UNIX)
        s.connect(str(control))
        s.sendall(json.dumps(req).encode())
        s.shutdown(socket.SHUT_WR)
        data = b""
        while chunk := s.recv(65536):
            data += chunk
        return json.loads(data)

    task_id = "task-authcheck"
    ref = {"credential_id": "cred-authcheck", "provider": "manual", "policy_id": "pol-authcheck"}
    policy = {"policy_id": "pol-authcheck", "revision": 1, "exact_origin": ORIGIN, "task_id": task_id,
              "max_ttl_seconds": 60, "require_approval": True, "allow_persistence": False}
    rep = ctl({"op": "register", "reference": ref, "policy": policy, "revision": 1,
               "secret": {"username": username, "password": password}})
    note(step="register", origin=ORIGIN, reply=rep)
    # http origin probe (documented only; the broker is expected to refuse it)
    rep = ctl({"op": "register", "reference": dict(ref, credential_id="cred-http"),
               "policy": dict(policy, exact_origin=f"http://127.0.0.1:{PORT}"), "revision": 1,
               "secret": {"username": "x", "password": "y"}})
    note(step="register-http-origin-probe", origin=f"http://127.0.0.1:{PORT}", reply=rep)

    def lease(tag, session):
        now = int(time.time())
        ph = secrets.token_hex(32)
        b = ctl({"op": "bind", "binding": {"token": "", "task_id": task_id, "run_id": "run-" + tag,
                                          "session_id": session, "exact_origin": ORIGIN,
                                          "policy_hash": ph, "expires_at": now + 60}})
        g = ctl({"op": "grant", "request": {
            "reference": ref, "policy": policy, "credential_revision": 1, "task_id": task_id,
            "run_id": "run-" + tag, "session_id": session, "approval_id": "appr-" + tag,
            "approved_by": "human-authcheck", "policy_hash": ph, "idempotency_key": "lease-" + tag,
            "ttl_seconds": 60, "approval_expires_at": now + 300, "session_expires_at": now + 60}})
        note(step="bind+grant", variant=tag, bind_success=b.get("success"), grant=g.get("success"),
             grant_code=g.get("code"))
        return b.get("binding_token"), g.get("lease_id")

    # ---- agent-browser
    wrapper = scratch / "plugin-wrapper.py"
    wrapper.write_text(WRAPPER)
    wrapper.chmod(0o700)
    diag = out / "plugin-diag.jsonl"
    browser_cfg = {"executablePath": args.chromium, "args": "--no-sandbox", "ignoreHttpsErrors": True}
    seg_policy = {"default": "deny", "allow": sorted(["auth_login", "close", "launch", "navigate",
                                                       f"plugin:{PLUGIN}:credential.read"])}
    (scratch / "policy-credential.json").write_text(json.dumps(seg_policy))
    obs_policy = {"default": "deny", "allow": ["close", "launch", "navigate", "snapshot", "gettext", "url"]}
    (scratch / "policy-observe.json").write_text(json.dumps(obs_policy))
    seq = [0]

    def ab(variant, session, cfg, pol, command, token=None, redact=(), env_extra=None):
        seq[0] += 1
        env = {"PATH": os.environ.get("PATH", ""), "HOME": str(home), "XDG_RUNTIME_DIR": str(run),
               "AGENT_BROWSER_NAMESPACE": "celeris-authcheck", "AGENT_BROWSER_SOCKET_DIR": str(sock)}
        env.update(env_extra or {})
        argv = [args.agent_browser, "--config", str(cfg), "--session", session, "--action-policy",
                str(scratch / pol), "--allowed-domains", "127.0.0.1", "--content-boundaries",
                "--max-output", "16000", "--json", *command]
        rfd, pre = None, None
        if token:
            # Supervisor-equivalent: a private pipe whose read end is FD 3 of the CLI process only.
            rfd, wfd = os.pipe()
            os.write(wfd, token.encode())
            os.close(wfd)
            pre = lambda: os.dup2(rfd, 3)  # noqa: E731  (dup2 makes FD 3 inheritable)
        try:
            p = subprocess.run(argv, env=env, capture_output=True, text=True, timeout=90,
                               preexec_fn=pre, close_fds=False, cwd=scratch)
        finally:
            if rfd is not None:
                os.close(rfd)
        shown = " ".join(argv)
        for secret, name in redact:
            shown = shown.replace(secret, name)
        tag = "".join(c if c.isalnum() else "_" for c in "-".join(command[:2]))[:24]
        base = out / f"{seq[0]:02d}-{variant}-{tag}"
        base.with_suffix(".stdout").write_text(p.stdout)
        base.with_suffix(".stderr").write_text(p.stderr)
        note(variant=variant, cmd=shown, exit=p.returncode, fd3_pipe=bool(token),
             stdout=p.stdout.strip()[:300], stderr=p.stderr.strip()[:300])
        return p

    def cfg_file(name, plugins):
        f = scratch / name
        f.write_text(json.dumps({"idleTimeout": "5m", "noWebmcp": True, "plugins": plugins, **browser_cfg}))
        (out / name).write_text(f.read_text())
        return f

    sessions = []
    try:
        if args.worker_settings:
            settings = args.worker_settings
            worker_cfg = json.loads((settings / "upstream.json").read_text())
            worker_cfg["plugins"][0]["command"] = args.credentiald
            worker_cfg.update(browser_cfg)
            wc = scratch / "upstream-worker.json"
            wc.write_text(json.dumps(worker_cfg))
            wp = scratch / "policy-worker.json"
            wp.write_bytes((settings / "policy.json").read_bytes())
            argv = json.loads((settings / "auth-argv.json").read_text())
            assert argv == ["auth", "login", PLUGIN, "--credential-provider", PLUGIN,
                            "--item", "LEASE", "--no-navigate", "--url", "ORIGIN/"]
            ws = "authchk-worker-" + secrets.token_hex(4)
            sessions.append((ws, "policy-worker.json"))
            tok, lid = lease("worker", ws)
            login_argv = [lid if a == "LEASE" else ORIGIN + "/" if a == "ORIGIN/" else a for a in argv]
            red = [(lid or "-", "<lease>")]
            opened = ab("worker", ws, wc, "policy-worker.json", ["open", ORIGIN + "/"], token=tok)
            before = ab("worker", ws, wc, "policy-worker.json", ["get", "url"])
            logged = ab("worker", ws, wc, "policy-worker.json", login_argv, token=tok, redact=red)
            after = ab("worker", ws, wc, "policy-worker.json", ["get", "url"])
            login_ok = fixture_state["login_ok"] == 1
            # The production worker replaces the policy atomically at this same path.
            replacement = scratch / "policy-worker.next"
            replacement.write_text(json.dumps(obs_policy))
            replacement.replace(wp)
            body = ab("worker-after-policy", ws, wc, "policy-worker.json", ["get", "text", "body"])
            marker_ok = marker in body.stdout
            # Restore the segment policy at the same path; the lease must still reject replay.
            replacement.write_bytes((settings / "policy.json").read_bytes())
            replacement.replace(wp)
            ab("worker-replay", ws, wc, "policy-worker.json", ["close"])
            time.sleep(0.5)  # 0.38.1 removes the old daemon socket asynchronously.
            for _ in range(3):
                reopened = ab("worker-replay", ws, wc, "policy-worker.json",
                              ["open", ORIGIN + "/"], token=tok)
                if reopened.returncode == 0:
                    break
                time.sleep(0.5)
            replay_origin = ab("worker-replay", ws, wc, "policy-worker.json", ["get", "url"])
            replay = ab("worker-replay", ws, wc, "policy-worker.json",
                        login_argv, token=tok, redact=red)
            result = {"agent_browser": "0.38.1", "worker_settings": str(settings),
                      "open_exit": opened.returncode, "origin_before_exit": before.returncode,
                      "auth_login_exit": logged.returncode, "origin_after_exit": after.returncode,
                      "fixture_login_ok": login_ok, "marker_verified": marker_ok,
                      "reopen_exit": reopened.returncode,
                      "replay_origin_exit": replay_origin.returncode,
                      "replay_exit": replay.returncode}
            (out / "worker-verification.json").write_text(json.dumps(result, indent=2))
            note(step="worker-verification", **result)

        # Variant 1: the pre-fix worker wiring (kept as a negative control).
        s1 = "authchk-v1-" + secrets.token_hex(4); sessions.append((s1, "policy-credential.json"))
        c1 = cfg_file("upstream-credential.v1.json",
                      {PLUGIN: {"command": args.credentiald, "args": ["bridge"]}})
        tok, lid = lease("v1", s1)
        red = [(lid or "-", "<lease>")]
        ab("v1", s1, c1, "policy-credential.json", ["open", ORIGIN + "/"])
        ab("v1", s1, c1, "policy-credential.json", ["get", "url"])
        ab("v1", s1, c1, "policy-credential.json",
           ["auth", "login", "--credential-provider", PLUGIN, "--credential-ref", lid, "--no-navigate",
            "--url", ORIGIN + "/"], token=tok, redact=red)
        note(step="revoke", variant="v1", reply=ctl({"op": "revoke", "lease_id": lid, "actor_id": "supervisor"}))

        # Variant 2: upstream-documented shape, FD 3 only on the auth-login CLI process.
        s2 = "authchk-v2-" + secrets.token_hex(4); sessions.append((s2, "policy-credential.json"))
        c2 = cfg_file("upstream-credential.v2.json", [{"name": PLUGIN, "command": str(wrapper),
                      "args": [str(diag), args.credentiald, "inherit"], "capabilities": ["credential.read"]}])
        tok, lid = lease("v2", s2)
        red = [(lid or "-", "<lease>")]
        ab("v2", s2, c2, "policy-credential.json", ["open", ORIGIN + "/"])
        ab("v2", s2, c2, "policy-credential.json", ["get", "url"])
        ab("v2", s2, c2, "policy-credential.json",
           ["auth", "login", PLUGIN, "--credential-provider", PLUGIN, "--item", lid, "--no-navigate",
            "--url", ORIGIN + "/"], token=tok, redact=red)
        after2 = fixture_state["login_ok"]
        note(step="revoke", variant="v2", reply=ctl({"op": "revoke", "lease_id": lid, "actor_id": "supervisor"}))

        # Variant 2b (diagnostic): FD 3 pipe also on the `open` that spawns the session daemon.
        s2b = "authchk-v2b-" + secrets.token_hex(4); sessions.append((s2b, "policy-credential.json"))
        tok, lid = lease("v2b", s2b)
        red = [(lid or "-", "<lease>")]
        ab("v2b", s2b, c2, "policy-credential.json", ["open", ORIGIN + "/"], token=tok)
        ab("v2b", s2b, c2, "policy-credential.json",
           ["auth", "login", PLUGIN, "--credential-provider", PLUGIN, "--item", lid, "--no-navigate",
            "--url", ORIGIN + "/"], token=tok, redact=red)
        note(step="revoke", variant="v2b", reply=ctl({"op": "revoke", "lease_id": lid, "actor_id": "supervisor"}))

        # Variant 3: test-only token delivery by file so the rest of the chain is observable.
        s3 = "authchk-v3-" + secrets.token_hex(4); sessions.append((s3, "policy-observe.json"))
        tok, lid = lease("v3", s3)
        tokfile = scratch / "binding-token"
        fd = os.open(tokfile, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        os.write(fd, tok.encode()); os.close(fd)
        c3 = cfg_file("upstream-credential.v3.json", [{"name": PLUGIN, "command": str(wrapper),
                      "args": [str(diag), args.credentiald, "token-file", str(tokfile)],
                      "capabilities": ["credential.read"]}])
        red = [(lid or "-", "<lease>"), (str(tokfile), "<tokenfile>")]
        # Segment policy plus `url`, which `get url` needs on the real binary.
        (scratch / "policy-v3.json").write_text(json.dumps(dict(seg_policy, allow=sorted(seg_policy["allow"] + ["url"]))))
        ab("v3", s3, c3, "policy-v3.json", ["open", ORIGIN + "/"], redact=red)
        ab("v3", s3, c3, "policy-v3.json", ["get", "url"], redact=red)
        ab("v3", s3, c3, "policy-v3.json",
           ["auth", "login", PLUGIN, "--credential-provider", PLUGIN, "--item", lid, "--no-navigate",
            "--url", ORIGIN + "/"], redact=red)
        ab("v3", s3, c3, "policy-v3.json", ["get", "url"], redact=red)
        # A: same policy path, contents rewritten in place to an observation policy.
        (scratch / "policy-v3.json").write_text(json.dumps(obs_policy))
        text = ab("v3-A-inplace", s3, c3, "policy-v3.json", ["get", "text", "body"], redact=red)
        snap = ab("v3-A-inplace", s3, c3, "policy-v3.json", ["snapshot"], redact=red)
        note(step="marker", in_gettext=marker in text.stdout, in_snapshot=marker in snap.stdout)
        # B: a different policy path (what a separate harness policy file would be).
        ab("v3-B-newpath", s3, c3, "policy-observe.json", ["get", "url"], redact=red)
        # C: a different upstream config file without plugins (harness config).
        c3b = cfg_file("upstream-observe.v3.json", [])
        ab("v3-C-newconfig", s3, c3b, "policy-observe.json", ["get", "url"], redact=red)
        # Replay of the consumed lease must fail (one use).
        fd = os.open(tokfile, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        os.write(fd, tok.encode()); os.close(fd)
        ab("v3-replay", s3, c3, "policy-credential.json", ["open", ORIGIN + "/"], redact=red)
        ab("v3-replay", s3, c3, "policy-credential.json",
           ["auth", "login", PLUGIN, "--credential-provider", PLUGIN, "--item", lid, "--no-navigate",
            "--url", ORIGIN + "/"], redact=red)
        note(step="fixture", v2_login_ok=after2, **fixture_state)
    finally:
        for s, pol in sessions:
            ab("cleanup", s, scratch / "upstream-credential.v3.json" if (scratch / "upstream-credential.v3.json").exists()
               else scratch / "upstream-credential.v1.json", pol, ["close"])
        broker.send_signal(signal.SIGTERM)
        broker.wait(timeout=10)
        srv.shutdown()
        journal = home / ".local/celeris/credentiald/audit/journal.jsonl"
        if journal.exists():
            (out / "broker-journal.jsonl").write_text(journal.read_text())
        # Kill leftover daemons/browsers that carry our socket dir in their environment.
        for pid in os.listdir("/proc"):
            try:
                if pid.isdigit() and str(sock).encode() in Path(f"/proc/{pid}/environ").read_bytes():
                    os.kill(int(pid), signal.SIGKILL)
                    note(step="killed-leftover", pid=int(pid))
            except OSError:
                pass
        (out / "steps.jsonl").write_text("".join(json.dumps(e) + "\n" for e in log))
        hits = []
        for root_dir in (out, scratch):
            for f in root_dir.rglob("*"):
                if f.is_file() and f.name != "sentinel":
                    try:
                        if password.encode() in f.read_bytes():
                            hits.append(str(f.relative_to(root_dir.parent)))
                    except OSError:
                        pass
        files = sorted(str(f.relative_to(scratch)) for f in scratch.rglob("*") if f.is_file()
                       and not str(f.relative_to(scratch)).startswith("home/.local/celeris"))
        (out / "sentinel-scan.json").write_text(json.dumps({"scanned": [str(out), str(scratch)],
                                                            "hits": hits, "scratch_files": files}, indent=1))
        print(json.dumps({"sentinel_hits": hits, "scratch": str(scratch)}))
        shutil.rmtree(scratch, ignore_errors=True)  # scratch key, vault and browser profile
    if args.worker_settings:
        journal = out / "broker-journal.jsonl"
        result["replay_broker_used"] = any(
            (entry := json.loads(line)).get("action") == "deny"
            and entry.get("decision_code") == "used"
            and entry.get("run_id") == "run-worker"
            for line in journal.read_text().splitlines()
        ) if journal.exists() else False
        (out / "worker-verification.json").write_text(json.dumps(result, indent=2))
        if not (result["open_exit"] == result["origin_before_exit"] == result["auth_login_exit"]
                == result["origin_after_exit"] == 0 and result["fixture_login_ok"]
                and result["marker_verified"] and result["reopen_exit"] == 0
                and result["replay_origin_exit"] == 0
                and result["replay_exit"] != 0 and result["replay_broker_used"] and not hits):
            raise SystemExit("worker credential wiring verification failed")


if __name__ == "__main__":
    main()
