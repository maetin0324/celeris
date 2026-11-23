#!/usr/bin/env python3
"""盲検の評価の driver（1 時点）。scripts/dev/parent-eval-blind.sh から呼ぶ（ADR 2026-11-22-parent-checkpoint-evaluator D8.3）。

一時 home に worktree の build の daemon（一時 DB・loopback）を起こし、時点（E-mid・D-done）の slice から使い捨ての案件・
strategy root・子を store の公開の関数（task-api の試験 `parent_checkpoint_blind_seed`）で seed し、controller を enabled に
する。controller の evaluate が start_side_review（SideReviewKind::ParentEvaluation）で reviewer run を起こし、root に最終の
記録 `parent-evaluation/<判断 id>.json` の ArtifactProduced が載るまで待つ。reviewer の作業 tree は slice の file だけを置いた
seed の git repo。出力は <out>/<point>/evaluation.json（最終の記録）・reads.json（読んだ path）・run.json（run id・harness・
model・usage・USD・案件の id）。本番の DB・daemon・config に触れない。LLM を呼ぶかは harness の設定だけで決まる。
"""
import argparse
import hashlib
import json
import os
import pathlib
import re
import secrets
import shutil
import signal
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from datetime import datetime

SLICE_FILES = ("goal.md", "reports.json", "slice.json")
STUB_HARNESS = """
[adapters.fake]
command = {cmd}

[llm_sources.local]
kind = "fake"
accounts = ["a1"]
models = ["model-1"]

[budget_rates."model-1"]
input_per_million = 1.0
output_per_million = 5.0
cache_read_per_million = 0.1
cache_write_per_million = 2.0

[harnesses.claude_code]
kind = "fake"
api_formats = ["fake"]
source_kinds = ["fake"]

[route_defaults.claude_code]
candidates = [{{ source = "local", model = "model-1" }}]

[harness_selection]
execution_class_defaults = {{}}
rules = [{{ harness = "claude_code", required_capabilities = [] }}]
"""


def die(msg):
    print(f"parent-eval-blind driver: FAIL: {msg}", file=sys.stderr)
    sys.exit(1)


def ts(raw):
    """RFC 3339（Z・ナノ秒）を datetime にする（小数は 6 桁で切る）。"""
    m = re.fullmatch(r"(.*?T\d\d:\d\d:\d\d)(\.\d+)?(Z|[+-]\d\d:\d\d)", raw)
    frac = (m.group(2) or "")[:7]
    zone = "+00:00" if m.group(3) == "Z" else m.group(3)
    return datetime.fromisoformat(m.group(1) + frac + zone)


def git_seed(files, dest):
    """slice の file だけを dest に置き、git init と初回 commit を作る（reviewer の作業 tree）。"""
    dest.mkdir(parents=True)
    for name, data in files.items():
        (dest / name).write_bytes(data)
    env = {**os.environ, "GIT_AUTHOR_NAME": "blind", "GIT_AUTHOR_EMAIL": "blind@localhost",
           "GIT_COMMITTER_NAME": "blind", "GIT_COMMITTER_EMAIL": "blind@localhost"}
    for cmd in (["init", "-q", "-b", "main"], ["add", "-A"], ["commit", "-q", "-m", "slice"]):
        subprocess.run(["git", "-C", str(dest), *cmd], check=True, env=env, capture_output=True)


def reads_from_log(text):
    """run の stdout の log（stdout.jsonl）から読んだ path と command を取り出す（claude-code の stream-json の tool_use と stub の `read:`）。"""
    out = []

    def walk(v):
        if isinstance(v, dict):
            if v.get("type") == "tool_use" and isinstance(v.get("input"), dict):
                inp = v["input"]
                for key in ("file_path", "path", "notebook_path"):
                    if isinstance(inp.get(key), str):
                        out.append({"tool": v.get("name"), "path": inp[key]})
                if isinstance(inp.get("command"), str):
                    out.append({"tool": v.get("name"), "command": inp["command"]})
                if isinstance(inp.get("pattern"), str) and "path" not in inp:
                    out.append({"tool": v.get("name"), "pattern": inp["pattern"]})
            for x in v.values():
                walk(x)
        elif isinstance(v, list):
            for x in v:
                walk(x)

    for line in text.splitlines():
        try:
            row = json.loads(line)
        except ValueError:
            continue
        if isinstance(row, dict) and row.get("type") == "progress" and str(row.get("msg", "")).startswith("read: "):
            out.append({"tool": "stub", "path": row["msg"][len("read: "):]})
        walk(row)
    return out


class Point:
    def __init__(self, a):
        self.a = a
        self.home = pathlib.Path(a.state).resolve()
        for bad in ("/local/celeris/state", "/local/celeris/data/db", str(pathlib.Path.home() / ".local/celeris"),
                    str(pathlib.Path.home() / ".config/celeris")):
            if str(self.home).startswith(bad):
                die(f"--state must not be under production: {self.home}")
        self.home.mkdir(parents=True, exist_ok=True)
        self.db = self.home / "celeris.sqlite3"
        self.repo = self.home / "slice-repo"
        self.token = secrets.token_urlsafe(32)
        self.secret_dir = pathlib.Path(tempfile.mkdtemp(prefix="blind-tokens-"))
        self.token_file = self.secret_dir / "human.token"
        with socket.socket() as s:
            s.bind(("127.0.0.1", 0))
            self.port = s.getsockname()[1]
        self.base = f"http://127.0.0.1:{self.port}"
        self.proc = None

    # ---- slice と seed ----
    def slice_files(self):
        shadow = pathlib.Path(self.a.shadow)
        meta = json.loads((shadow / self.a.point / "slice.json").read_text())
        cut = ts(meta["cutoff"])
        events = b"".join(line + b"\n" for line in (shadow / "G-done" / "events.jsonl").read_bytes().splitlines()
                          if line.strip() and ts(json.loads(line)["ts"]) <= cut)
        files = {"events.jsonl": events}
        for name in SLICE_FILES:
            src = shadow / self.a.point / name
            files[name] = (src if src.exists() else shadow / "E-mid" / name).read_bytes()
        return files

    def seed(self):
        git_seed(self.slice_files(), self.repo)
        spec = self.home / "seed-spec.json"
        out = self.home / "seed.json"
        spec.write_text(json.dumps({"db": str(self.db), "repo": str(self.repo), "point": self.a.point, "out": str(out)}))
        r = subprocess.run([self.a.seed_bin, "--ignored", "--exact", "parent_checkpoint_blind_seed", "--nocapture"],
                           env={**self.clean_env(), "PARENT_EVAL_BLIND_SEED": str(spec)}, capture_output=True, text=True)
        if r.returncode != 0:
            die(f"seed failed: {r.stdout[-2000:]} {r.stderr[-2000:]}")
        self.seeded = json.loads(out.read_text())

    # ---- daemon ----
    def clean_env(self):
        return {k: v for k, v in os.environ.items() if not k.startswith("CELERIS_") and k not in
                ("CLAUDECODE", "CLAUDE_CODE_SESSION_ID")}

    def write_config(self):
        if self.a.harness == "stub":
            harness = STUB_HARNESS.format(cmd=json.dumps(["python3", "-I", str(pathlib.Path(self.a.stub).resolve())]))
        else:
            harness = pathlib.Path(self.a.llm_config).read_text()
        (self.home / "ws").mkdir(exist_ok=True)
        (self.home / "config.toml").write_text(f"""
db = "celeris.sqlite3"
workspace_root = "ws"
tick_ms = 100
max_concurrency = 2

[api]
listen = "127.0.0.1:{self.port}"

[[api.tokens]]
principal = "human:blind"
token_file = {json.dumps(str(self.token_file))}

[cos]
enabled = false

[workspace]
build_cache_dir = {json.dumps(str(self.home / "build-cache"))}

[scratch]
dir = {json.dumps(str(self.home / "scratch"))}

[script]
state_dir = "state"

[project_controller]
enabled = true
allowed_project_ids = {json.dumps([self.seeded["project_id"]])}
coalesce_secs = 1

[project_controller.budget]
max_roots_per_day = 1
max_usd_per_day = {self.a.max_usd}
{harness}
""")

    def start(self):
        fd = os.open(self.token_file, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w") as f:
            f.write(self.token + "\n")
        try:
            log = open(self.home / "daemon.log", "ab")
            self.proc = subprocess.Popen(
                [self.a.celeris, "--config", str(self.home / "config.toml"), "--log-format", "text", "--release", "blind"],
                stdout=log, stderr=log, stdin=subprocess.DEVNULL, env=self.clean_env(), cwd=self.home)
            for _ in range(600):
                if self.proc.poll() is not None:
                    print((self.home / "daemon.log").read_text()[-3000:], file=sys.stderr)
                    die("the daemon exited")
                try:
                    self.api("GET", "/api/v1/script/domains")
                    return
                except (urllib.error.URLError, ConnectionError):
                    time.sleep(0.2)
            die("the daemon API did not answer in 120 s")
        finally:
            self.token_file.unlink(missing_ok=True)

    def stop(self):
        if self.proc is not None and self.proc.poll() is None:
            self.proc.send_signal(signal.SIGTERM)
            try:
                self.proc.wait(timeout=60)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        self.proc = None

    def api(self, method, path, body=None):
        req = urllib.request.Request(self.base + path, method=method,
                                     data=None if body is None else json.dumps(body).encode(),
                                     headers={"Authorization": f"Bearer {self.token}", "Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                data = r.read()
        except urllib.error.HTTPError as e:
            print(f"blind: HTTP {e.code} {method} {path}: {e.read().decode(errors='replace')[:600]}", file=sys.stderr)
            raise
        return json.loads(data) if data else None

    def deploy_ts(self):
        """本物の bundle を配備し、一時 DB の領域 project_controller を ts にする（long-horizon-pilot の driver と同じ手順。一時 DB だけ）。"""
        sha = subprocess.run([self.a.script_deploy, "--dist", self.a.bundle, "--state-dir", str(self.home / "state")],
                             check=True, capture_output=True, text=True).stdout.strip()
        self.api("POST", "/api/v1/script/generations", {"bundle_sha256": sha})
        self.stop()
        conn = sqlite3.connect(self.db)
        gen = conn.execute("SELECT id FROM script_generations WHERE bundle_sha256 = ?", (sha,)).fetchone()[0]
        now = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        conn.execute("UPDATE script_domain_modes SET mode='ts', generation_id=?, version=version+1, updated_at=?, "
                     "updated_by='human:blind' WHERE domain='project_controller'", (gen, now))
        if conn.total_changes != 1:
            die("script_domain_modes has no project_controller row")
        conn.execute("UPDATE script_generations SET state='active' WHERE id=?", (gen,))
        conn.commit()
        conn.close()

    def root_events(self):
        conn = sqlite3.connect(f"file:{self.db}?mode=ro", uri=True)
        rows = conn.execute("SELECT seq, json FROM events WHERE task_id = ? ORDER BY seq",
                            (self.seeded["root_id"],)).fetchall()
        conn.close()
        return [(s, json.loads(j)) for s, j in rows]

    def artifacts(self):
        return [(seq, e) for seq, e in self.root_events() if e.get("type") == "artifact_produced"]

    def wait_record(self):
        deadline = time.time() + self.a.max_wall_secs
        while time.time() < deadline:
            if self.proc.poll() is not None:
                die("the daemon exited during the evaluation")
            for _, e in self.artifacts():
                name = e["artifact"]["name"]
                if name.startswith("parent-evaluation/") and ".attempt-" not in name:
                    return e
            time.sleep(1)
        print((self.home / "daemon.log").read_text()[-4000:], file=sys.stderr)
        die(f"no parent-evaluation record on the root in {self.a.max_wall_secs} s")

    def artifact_body(self, event):
        """最終の記録の本文（一時 home の下の file。ArtifactProduced の sha256 と一致を確かめる）。"""
        name, want = event["artifact"]["name"], event["artifact"]["sha256"]
        for p in sorted(self.home.rglob(pathlib.PurePosixPath(name).name)):
            data = p.read_bytes()
            if p.as_posix().endswith(name) and hashlib.sha256(data).hexdigest() in (want, want.removeprefix("sha256:")):
                return json.loads(data)
        die(f"artifact {name} (sha256 {want}) is not under {self.home}")

    def run(self):
        self.seed()
        self.write_config()
        self.start()
        self.deploy_ts()
        self.start()
        pid = self.seeded["project_id"]
        # 人が承認した Roadmap（版 1。Outcome が無い案件では controller が pc/no-outcomes で人に返す）。既存の API で置く。
        base = f"/api/v1/projects/{pid}/roadmap"
        self.api("POST", f"{base}/proposals", {
            "base_version": None, "change_reason": "盲検の評価の案件（一時 DB）",
            "outcomes": [{"id": "o1", "title": "循環の削減", "acceptance": ["既存の module 循環の数が base より減る"],
                          "priority": 1, "depends_on": []}]})
        self.api("POST", f"{base}/proposals/1/decide", {"decision": "approve", "note": "一時 DB の seed"})
        rev = self.api("GET", f"/api/v1/projects/{pid}/controller")["state"]["revision"]
        self.api("PUT", f"/api/v1/projects/{pid}/controller",
                 {"mode": "enabled", "expected_revision": rev, "reason": "盲検の評価（一時 DB）"})
        self.api("POST", f"/api/v1/projects/{pid}/controller/wake",
                 {"external_ref": "blind-start", "note": "開始", "idempotency_key": "blind-start"})
        final = self.wait_record()
        name = final["artifact"]["name"]
        record = self.artifact_body(final)
        attempts = [e for _, e in self.artifacts() if e["artifact"]["name"].startswith(name[:-len(".json")] + ".attempt-")]
        run_id = attempts[-1]["run_id"] if attempts else final.get("run_id")
        self.stop()
        self.collect(record, run_id)

    def collect(self, record, run_id):
        conn = sqlite3.connect(f"file:{self.db}?mode=ro", uri=True)
        row = conn.execute("SELECT adapter, model, account, status, usage_json FROM runs WHERE run_id = ?",
                           (run_id,)).fetchone()
        conn.close()
        usage = json.loads(row[4]) if row and row[4] else None
        logs = [p for p in self.home.rglob("stdout.*") if run_id and run_id in str(p) and p.suffix in (".jsonl", ".log")]
        reads = []
        for p in sorted(logs):
            reads += reads_from_log(p.read_text(errors="replace"))
        out = pathlib.Path(self.a.out) / self.a.point
        out.mkdir(parents=True, exist_ok=True)
        dump = lambda name, v: (out / name).write_text(json.dumps(v, ensure_ascii=False, indent=2, sort_keys=True) + "\n")
        dump("evaluation.json", record)
        dump("reads.json", {"work_tree": str(self.repo), "home": str(self.home), "logs": [str(p) for p in logs],
                            "reads": reads})
        dump("run.json", {"point": self.a.point, "run_id": run_id, "harness": self.a.harness,
                          "adapter": row[0] if row else None, "model": row[1] if row else None,
                          "status": row[3] if row else None, "usage": usage,
                          "usd": (usage or {}).get("cost_usd"), "project_id": self.seeded["project_id"],
                          "root_id": self.seeded["root_id"], "children": self.seeded["children"]})
        if not logs:
            die(f"no stdout log for run {run_id} (reads.json is empty)")


def main():
    p = argparse.ArgumentParser()
    for k in ("point", "state", "out", "celeris", "script-deploy", "bundle", "seed-bin", "shadow", "harness"):
        p.add_argument(f"--{k}", required=True)
    p.add_argument("--stub")
    p.add_argument("--llm-config")
    p.add_argument("--max-wall-secs", type=int, default=600)
    p.add_argument("--max-usd", type=float, default=1.0)
    a = p.parse_args()
    if a.harness not in ("stub", "config"):
        die("--harness must be stub or config")
    pt = Point(a)
    try:
        pt.run()
    finally:
        pt.stop()
        log = pt.home / "daemon.log"
        if log.exists():
            dest = pathlib.Path(a.out) / a.point
            dest.mkdir(parents=True, exist_ok=True)
            shutil.copy(log, dest / "daemon.log")
        shutil.rmtree(pt.secret_dir, ignore_errors=True)


if __name__ == "__main__":
    main()
