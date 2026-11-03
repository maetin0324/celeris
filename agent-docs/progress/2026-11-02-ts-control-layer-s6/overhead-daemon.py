#!/usr/bin/env python3
"""S6 overhead: 一時 DB の非本番 daemon で RSS・isolate 数・tick の待ちを測る（負荷をかけない。LLM を呼ばない）。
使い方: overhead-daemon.py rss <celeris> <script-deploy> <dist1> <dist2> <reps> <out.json>
        overhead-daemon.py tick <celeris-probe> <script-deploy> <dist1> <tasks> <secs> <out.json>
daemon は $TMPDIR の一時 dir と loopback の port だけを使い、終わったら止めて消す。"""
import json, os, shutil, signal, socket, sqlite3, subprocess, sys, tempfile, time, urllib.error, urllib.request

HUMAN = "ov-human-token"

def config(home, port, script, tick_ms=None):
    tick = f"tick_ms = {tick_ms}\n" if tick_ms else ""
    s = f'''db = "celeris.sqlite3"
workspace_root = "ws"
{tick}
[llm_sources.local]
kind = "fake"
accounts = ["a1"]
models = ["fake"]

[harnesses.fake]
kind = "fake"
api_formats = ["fake"]
source_kinds = ["fake"]
command = "sh"
args = ["-c", "cat >/dev/null; echo '{{\\"type\\":\\"question\\",\\"text\\":\\"not needed\\"}}'"]

[route_defaults.fake]
candidates = [{{ source = "local", model = "fake" }}]

[harness_selection]
execution_class_defaults = {{}}
rules = [{{ harness = "fake", required_capabilities = [] }}]

[cos]
enabled = false

[workspace]
build_cache_dir = "{home}/build-cache"

[scratch]
dir = "{home}/scratch"

[api]
listen = "127.0.0.1:{port}"

[[api.tokens]]
principal = "human:overhead"
token_file = "human.token"
'''
    if script:
        s += '\n[script]\nstate_dir = "state"\n'
    return s

class Daemon:
    def __init__(self, binary, script, tick_ms=None):
        self.home = tempfile.mkdtemp(prefix="ov-daemon.", dir=os.environ["TMPDIR"])
        os.makedirs(self.home + "/ws")
        open(self.home + "/human.token", "w").write(HUMAN + "\n")
        sock = socket.socket(); sock.bind(("127.0.0.1", 0)); self.port = sock.getsockname()[1]; sock.close()
        open(self.home + "/config.toml", "w").write(config(self.home, self.port, script, tick_ms))
        self.log = open(self.home + "/daemon.log", "ab")
        self.p = subprocess.Popen([binary, "--config", self.home + "/config.toml", "--log-format", "text", "--release", "overhead"],
                                  stdout=self.log, stderr=subprocess.STDOUT)
        for _ in range(600):
            if self.p.poll() is not None:
                raise SystemExit("daemon exited: " + open(self.home + "/daemon.log").read()[-2000:])
            try:
                st, _b = self.api("GET", "/tasks?limit=1")
                if st == 200:
                    return
            except OSError:
                pass
            time.sleep(0.2)
        raise SystemExit("daemon API did not answer")

    def api(self, method, path, body=None):
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(f"http://127.0.0.1:{self.port}/api/v1{path}", data=data, method=method,
                                     headers={"Authorization": f"Bearer {HUMAN}", "Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=60) as r:
                return r.status, json.loads(r.read() or b"null")
        except urllib.error.HTTPError as e:
            raw = e.read()
            try:
                return e.code, json.loads(raw)
            except Exception:
                return e.code, raw.decode(errors="replace")

    def rss_mib(self):
        for line in open(f"/proc/{self.p.pid}/status"):
            if line.startswith("VmRSS:"):
                return int(line.split()[1]) / 1024
        raise SystemExit("no VmRSS")

    def isolates(self):
        n = 0
        for t in os.listdir(f"/proc/{self.p.pid}/task"):
            try:
                if open(f"/proc/{self.p.pid}/task/{t}/comm").read().strip() == "celeris-script":
                    n += 1
            except OSError:
                pass
        return n

    def settled(self, secs=6.0):
        """secs 待ってから 5 回読んだ RSS の中央値。"""
        time.sleep(secs)
        v = []
        for _ in range(5):
            v.append(self.rss_mib()); time.sleep(0.2)
        return sorted(v)[2]

    def deploy(self, script_deploy, dist):
        return subprocess.run([script_deploy, "--dist", dist, "--state-dir", self.home + "/state"],
                              check=True, capture_output=True, text=True).stdout.strip()

    def register(self, sha):
        for _ in range(300):
            st, body = self.api("POST", "/script/generations", {"bundle_sha256": sha})
            if st != 503:
                break
            time.sleep(0.2)
        assert st == 200 and body.get("state") == "loaded", (st, body)
        return body

    def domain(self, name):
        st, body = self.api("GET", "/script/domains")
        assert st == 200, (st, body)
        return next(d for d in body["domains"] if d["domain"] == name)

    def put_mode(self, name, mode, sha=None, bps=None):
        body = {"mode": mode, "expected_version": self.domain(name)["version"], "reason": "overhead"}
        if sha:
            body["generation"] = sha
        if bps is not None:
            body["canary_bps"] = bps
        st, r = self.api("PUT", f"/script/domains/{name}/mode", body)
        assert st == 200, (name, mode, st, r)

    def generations(self):
        st, body = self.api("GET", "/script/generations")
        assert st == 200, (st, body)
        return {g["bundle_sha256"]: g["state"] for g in body["generations"]}

    def stop(self):
        self.p.send_signal(signal.SIGTERM)
        self.p.wait(timeout=60)
        self.log.close()
        shutil.rmtree(self.home, ignore_errors=True)

def rss(args):
    binary, deploy, dist1, dist2, reps, out = args
    runs = []
    for rep in range(int(reps)):
        row = {"rep": rep + 1}
        d = Daemon(binary, False)
        row["no_script_section"] = {"rss_mib": d.settled(), "isolates": d.isolates()}
        d.stop()
        d = Daemon(binary, True)
        row["script_no_generation"] = {"rss_mib": d.settled(), "isolates": d.isolates()}
        sha1, sha2 = d.deploy(deploy, dist1), d.deploy(deploy, dist2)
        assert sha1 != sha2
        d.register(sha1)
        d.put_mode("cos_digest", "compare", sha1)
        row["gen1_active"] = {"rss_mib": d.settled(), "isolates": d.isolates(), "states": d.generations()}
        d.register(sha2)
        row["gen1_active_gen2_loaded"] = {"rss_mib": d.settled(), "isolates": d.isolates(), "states": d.generations()}
        # 切替の直後: 旧 generation が draining の間（見回りは 1 秒ごと）に読む。in-flight は 0。
        t0 = time.monotonic()
        d.put_mode("cos_digest", "compare", sha2)
        states = d.generations()
        row["gen2_active_gen1_draining"] = {"rss_mib": d.rss_mib(), "isolates": d.isolates(), "states": states,
                                            "secs_after_switch": round(time.monotonic() - t0, 3),
                                            "gen1_state": states.get(sha1)}
        row["gen2_active_gen1_retired"] = {"rss_mib": d.settled(), "isolates": d.isolates(), "states": d.generations()}
        d.stop()
        runs.append(row)
        print(json.dumps(row, ensure_ascii=False), flush=True)
    json.dump({"runs": runs}, open(out, "w"), ensure_ascii=False, indent=1)

def tick(args):
    binary, deploy, dist1, tasks, secs, out = args
    d = Daemon(binary, True)
    sha = d.deploy(deploy, dist1)
    gen = d.register(sha)
    for name, need in (("lane_rules", 1000), ("route_priority", 1000)):
        d.put_mode(name, "compare", sha)
        c = sqlite3.connect(d.home + "/celeris.sqlite3", timeout=30)
        with c:
            c.execute('INSERT INTO script_guard_state (domain, generation_id, mode, "window", consecutive_failures, timeouts_10min, '
                      "clean_streak, ts_side_clean_streak, calls, safety_violations, updated_at) "
                      "VALUES (?, ?, 'compare', ?, 0, '[]', ?, 0, ?, 0, strftime('%Y-%m-%dT%H:%M:%SZ','now')) "
                      'ON CONFLICT (domain, generation_id) DO UPDATE SET mode = excluded.mode, "window" = excluded."window", '
                      "clean_streak = excluded.clean_streak, calls = excluded.calls, updated_at = excluded.updated_at",
                      (name, gen["id"], "o" * need, need, need))
        c.close()
        d.put_mode(name, "canary", sha, 9999)
    created = []
    for i in range(int(tasks)):
        st, r = d.api("POST", "/tasks", {"title": f"overhead {i}", "objective": "overhead の tick 待ちの測定（fake harness）",
                                         "acceptance": [{"type": "command", "cmd": "true", "expect_exit": 0}]})
        created.append(st)
        if st >= 300:
            print("task POST", st, r, flush=True)
    time.sleep(float(secs))
    lines = [l.strip() for l in open(d.home + "/daemon.log", errors="replace") if "OVERHEAD_TICK_WAIT" in l]
    c = sqlite3.connect(d.home + "/celeris.sqlite3", timeout=30)
    cols = [r[1] for r in c.execute("PRAGMA table_info(events)")]
    runs_n = c.execute("SELECT count(*) FROM runs").fetchone()[0] if c.execute("SELECT 1 FROM sqlite_master WHERE name='runs'").fetchone() else None
    tables = {}
    for t in ("script_domain_events", "script_generation_events", "script_compare_log"):
        if c.execute("SELECT 1 FROM sqlite_master WHERE name=?", (t,)).fetchone():
            tables[t] = c.execute(f"SELECT count(*) FROM {t}").fetchone()[0]
    ev = {}
    for (j,) in c.execute("SELECT json FROM events"):
        k = json.loads(j).get("type")
        if k and ("script" in k):
            ev[k] = ev.get(k, 0) + 1
    records = []
    for (j,) in c.execute("SELECT json FROM events"):
        o = json.loads(j)
        if o.get("type") == "script_compare" and len(records) < 2:
            records.append(o)
    modes = {n: d.domain(n)["mode"] for n in ("lane_rules", "route_priority")}
    c.close()
    res = {"tasks": int(tasks), "task_post_status": sorted(set(created)), "secs": float(secs), "tick_wait_lines": lines,
           "runs": runs_n, "tables": tables, "script_events": ev, "sample_records": records, "modes_at_end": modes}
    json.dump(res, open(out, "w"), ensure_ascii=False, indent=1)
    print(json.dumps({k: v for k, v in res.items() if k not in ("sample_records",)}, ensure_ascii=False)[:3000], flush=True)
    d.stop()

{"rss": rss, "tick": tick}[sys.argv[1]](sys.argv[2:])
