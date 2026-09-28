#!/usr/bin/env python3
"""Fake agent-browser 0.38.1 for the browser Phase 2 end-to-end tests (not the real substrate).

Pages come from the localhost fixture `site.json` next to this script. The credential plugin
is spawned exactly as configured by the supervisor, with the private binding pipe on FD 3.
The received secret is kept only in memory; the state file records a digest, never the value.
"""
import hashlib
import json
import pathlib
import subprocess
import sys
from urllib.parse import urlsplit

HERE = pathlib.Path(__file__).resolve().parent
args = sys.argv[1:]
if args == ["--version"]:
    print("agent-browser 0.38.1")
    sys.exit(0)


def opt(name):
    return args[args.index(name) + 1] if name in args else None


command = args[args.index("--json") + 1:]
root = pathlib.Path.cwd()
session = opt("--session")
with (root / "commands.jsonl").open("a") as out:
    out.write(json.dumps(command) + "\n")
policy = json.loads(pathlib.Path(opt("--action-policy")).read_text())
domains = (opt("--allowed-domains") or "").split(",")
site = json.loads((HERE / "site.json").read_text())
state_path = root / f"state-{session}.json"
state = json.loads(state_path.read_text()) if state_path.exists() else {"url": "about:blank"}
ACTION = {"open": "navigate", "snapshot": "snapshot", "get": "gettext", "close": "close",
          "auth": "auth_login"}


def reply(ok, **data):
    print(json.dumps({"success": ok, **({"data": data} if data else {})}))
    state_path.write_text(json.dumps(state))
    sys.exit(0 if ok else 1)


verb = command[0]
action = "geturl" if command[:2] == ["get", "url"] else ACTION.get(verb)
if action != "geturl" and action not in policy["allow"]:
    print(json.dumps({"success": False, "error": "Action denied by policy"}))
    sys.exit(1)


def allowed(url):
    host = urlsplit(url).hostname or ""
    return any(host.endswith(d[1:]) if d.startswith("*.") else host == d for d in domains)


if verb == "open":
    url = command[1]
    url = site.get("redirects", {}).get(url, url)
    if not allowed(url):
        print(json.dumps({"success": False, "error": "not allowed by domain filter"}))
        sys.exit(1)
    state["url"] = url
    reply(True)
if action == "geturl":
    reply(True, url=state["url"])
if verb == "snapshot":
    reply(True, text=site.get("pages", {}).get(state["url"], ""))
if verb == "auth":
    config = json.loads(pathlib.Path(opt("--config")).read_text())
    plugin = config["plugins"][command[command.index("--credential-provider") + 1]]
    request = {"protocol": "agent-browser.plugin.v1", "type": "credential.resolve",
               "capability": "credential.read",
               "request": {"profileName": "celeris-credential",
                           "itemRef": command[command.index("--credential-ref") + 1],
                           "url": state["url"]}}
    result = subprocess.run([plugin["command"], *plugin["args"]], input=json.dumps(request).encode(),
                            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, pass_fds=(3,),
                            check=False)
    try:
        response = json.loads(result.stdout)
    except ValueError:
        response = {}
    credential = response.get("credential") if response.get("success") is True else None
    if not credential:
        state["plugin"] = "failure"
        reply(False)
    # "Fill and submit": the page now knows who logged in. Keep only a digest on disk.
    state["plugin"] = "success"
    state["login_digest"] = hashlib.sha256(
        (credential["username"] + "\0" + credential["password"]).encode()).hexdigest()
    reply(True)
if verb == "close":
    reply(True)
reply(False)
