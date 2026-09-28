#!/usr/bin/env python3
"""Real pinned agent-browser smoke on a local, public fixture. No LLM/API credentials.

python3 scripts/browser-smoke.py --agent-browser /absolute/bin/agent-browser
  --chromium /absolute/chrome --output /absolute/task/artifacts/browser-smoke
"""
import argparse
import functools
import http.server
import json
from pathlib import Path
import shutil
import subprocess
import threading
import time
import uuid


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--agent-browser", required=True)
    parser.add_argument("--chromium", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=True)
    fixture = root / "fixture"
    fixture.mkdir(exist_ok=True)
    (fixture / "index.html").write_text(
        '<title>Browser capability fixture</title><h1 id="visits">Public fixture</h1>'
        '<a href="/next.html">Next</a><a href="/data.csv" download>Download</a>'
        '<script>const n=Number(localStorage.getItem("fixtureVisits")||0)+1;'
        'localStorage.setItem("fixtureVisits",String(n));'
        'document.getElementById("visits").textContent="Public fixture visit "+n;</script>'
    )
    (fixture / "next.html").write_text('<title>Next</title><p>Extraction evidence</p><a href="/data.csv" download>Download</a>')
    (fixture / "data.csv").write_text("name,value\npublic,42\n")

    class Handler(http.server.SimpleHTTPRequestHandler):
        def log_message(self, *_):
            with (root / "fixture-http.log").open("a") as log:
                log.write(self.path + "\n")

        def end_headers(self):
            if self.path == "/data.csv":
                self.send_header("Content-Disposition", 'attachment; filename="data.csv"')
            super().end_headers()

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Handler, directory=str(fixture)))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    runtime = root / uuid.uuid4().hex
    runtime.mkdir()
    output = runtime / "output"
    output.mkdir()
    cli = runtime / "celeris-browser.py"
    shutil.copyfile(Path(__file__).resolve().parents[1] / "crates/task-worker/src/browser_cli.py", cli)
    # Fixture-only diagnostic capture. Never enable raw response capture in production.
    cli.write_text(cli.read_text().replace('        if len(result.stdout)',
        '        (ROOT / ("fixture-" + operation + ".response")).write_bytes(result.stdout)\n        if len(result.stdout)'))
    config = {"executable": args.agent_browser, "session_id": "celeris-smoke-" + uuid.uuid4().hex,
              "allowed_domains": ["127.0.0.1"], "output": str(output)}
    (runtime / "config.json").write_text(json.dumps(config))
    # A test-only browser executable; production uses agent-browser's installed browser.
    (runtime / "upstream.json").write_text(json.dumps({"executablePath": args.chromium, "args": "--no-sandbox"}))
    (runtime / "policy.json").write_text(json.dumps({"default": "deny", "allow": ["launch", "navigate", "click", "snapshot", "gettext", "screenshot", "download", "scroll", "close"]}))
    results = []

    second_cli = None

    def command(*argv, success=True, cli_path=None):
        started = time.monotonic()
        result = subprocess.run(["python3", str(cli_path or cli), *argv], capture_output=True, text=True, timeout=60)
        results.append({"action": argv[0], "exit": result.returncode, "seconds": round(time.monotonic() - started, 2), "session": "second" if cli_path else "first"})
        assert (result.returncode == 0) == success, (argv[0], result.stdout, result.stderr)
        return result.stdout

    try:
        command("open", f"http://127.0.0.1:{server.server_port}/index.html")
        text = command("snapshot")
        assert "Next" in text
        assert "Public fixture visit 1" in text
        snapshot = json.loads(text.splitlines()[1])
        assert snapshot["_boundary"]["nonce"]
        refs = snapshot["data"]["refs"]
        def named_ref(name, role):
            matches = [key for key, value in refs.items() if value.get("name") == name and value.get("role") == role]
            assert len(matches) == 1, (name, refs)
            return "@" + matches[0]
        download_ref = named_ref("Download", "link")
        next_ref = named_ref("Next", "link")
        command("download", download_ref)
        command("extract", next_ref)
        command("screenshot")
        command("click", next_ref)
        assert "Download" in command("snapshot")
        command("eval", "document.cookie", success=False)
        command("cookies", success=False)
        command("storage", "local", success=False)
        command("--cdp", "9222", success=False)
        command("open", "https://not-allowed.invalid/", success=False)
        assert len(list(output.glob("extract-*.json"))) == 1
        assert len(list(output.glob("screenshot-*.png"))) == 1
        assert len(list(output.glob("download-*.bin"))) == 1
        # Verify upstream's own policy, bypassing the shim grammar for negative tests.
        import os
        env = {k: v for k, v in os.environ.items() if not k.startswith("AGENT_BROWSER_")}
        env["AGENT_BROWSER_NAMESPACE"] = "celeris"
        for raw in [["eval", "1+1"], ["cookies"], ["storage", "local"]]:
            result = subprocess.run([args.agent_browser, "--config", str(runtime / "upstream.json"),
                                     "--session", config["session_id"],
                                     "--action-policy", str(runtime / "policy.json"),
                                     "--allowed-domains", ",".join(config["allowed_domains"]),
                                     "--content-boundaries", "--max-output", "16000", "--json", *raw],
                                    env=env, capture_output=True, text=True, timeout=30)
            data = json.loads(result.stdout)
            assert data.get("success") is False, raw
            results.append({"upstream_denied": raw[0], "success": data["success"]})
        # Upstream 0.38.1 treats an empty/unreadable policy as no policy; the shim must refuse.
        good_policy = (runtime / "policy.json").read_text()
        for broken in ['{"default":"deny","allow":[]}', '{"default":"deny","allow":']:
            (runtime / "policy.json").write_text(broken)
            command("snapshot", success=False)
        (runtime / "policy.json").unlink()
        command("snapshot", success=False)
        (runtime / "policy.json").write_text(good_policy)
        results.append({"policy_fail_closed": ["empty_allow", "unparsable", "missing"], "success": True})
        # Observe harmless page-rendered storage state through normal snapshots.
        # Do not use raw cookie/storage reads or eval to test session isolation.
        command("open", f"http://127.0.0.1:{server.server_port}/index.html")
        assert "Public fixture visit 2" in command("snapshot")
        second_runtime = root / uuid.uuid4().hex
        second_runtime.mkdir()
        second_output = second_runtime / "output"
        second_output.mkdir()
        second_cli = second_runtime / "celeris-browser.py"
        for name in ["celeris-browser.py", "upstream.json", "policy.json"]:
            shutil.copyfile(runtime / name, second_runtime / name)
        second_config = dict(config, session_id="celeris-smoke-" + uuid.uuid4().hex,
                             output=str(second_output))
        (second_runtime / "config.json").write_text(json.dumps(second_config))
        command("open", f"http://127.0.0.1:{server.server_port}/index.html", cli_path=second_cli)
        assert "Public fixture visit 1" in command("snapshot", cli_path=second_cli)
        results.append({"isolation": "first_visit_2_second_visit_1", "success": True})
    finally:
        if second_cli is not None:
            command("close", cli_path=second_cli)
        command("close")
        server.shutdown()
    (root / "result.json").write_text(json.dumps({"ok": True, "version": "0.38.1", "runtime": str(runtime), "second_runtime": str(second_cli.parent) if second_cli else None, "results": results}, indent=2) + "\n")
    print(json.dumps({"ok": True, "checks": len(results)}))


if __name__ == "__main__":
    main()
