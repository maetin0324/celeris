#!/usr/bin/env python3
"""対照 pilot の driver（1 事例・1 時点・1 方式）。scripts/dev/parent-eval-blind.sh から呼ぶ（ADR 2026-11-24-strategy-pilot-ablation
D1、ADR 2026-11-22-parent-checkpoint-evaluator D8.3）。

一時 home に worktree の build の daemon（一時 DB・loopback）を起こし、時点の slice から使い捨ての案件・承認済み Roadmap・
strategy root・子を store の公開の関数（task-api の試験 `parent_checkpoint_blind_seed`）で seed する。方式の違いは controller の
mode だけで作る:
- 方式 a（旧の固定 Task tree）: mode shadow。evaluate の判断は effect_state suppressed で記録され、reviewer run は起きない（LLM 0）。
  子の局所の review は slice の史実の記録（reviewer_passed）を使い、流し直さない（ADR D1.1）。
- 方式 b（途中の親 Evaluator）: mode enabled。evaluate が start_side_review（SideReviewKind::ParentEvaluation）で reviewer run を
  起こし、root に最終の記録 `parent-evaluation/<判断 id>.json` の ArtifactProduced が載るまで待つ。
- 方式 c（Evidence + 複数戦略）: b の評価に、版 v1 を書く run と Critic を足す（ADR D1.3）。版を書く段は未実装で、この driver は
  方式 c を断る（arm-driver の進捗の「欠け」）。
reviewer の作業 tree は slice の file だけを置いた seed の git repo。出力は <out>/<case>/<point>/<arm>/ の output.json（verdict・
next・claims〈根拠の参照つき〉・usage）・reads.json（読んだ path）・run.json（run id・harness・model・usage・USD・案件の id）と、
方式に渡した入力の写し input/（request.json・prompt.md・files.json と tree/ の本文）。本番の DB・daemon・config に触れない。
LLM を呼ぶかは harness の設定だけで決まる。
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
ARMS = ("a", "b", "c")
# 方式 c の版を書く run と Critic の段（ADR D1.3）はまだ driver に無い。手で書いた版で代えない（ADR D1.5）。
ARMS_RUNNABLE = ("a", "b")
CASES = {"replay": ("C-mid", "D-done", "F-done")}
# 1 run の上限（ADR D3.1。3 方式で同じ値）と 1 時点の費用の上限。
MAX_USD_PER_POINT = 3.0
OUTPUT_SCHEMA = "celeris.pilot-arm-output/1"
CLAIM_KINDS = ("measured", "learned", "refuted", "uncertainty")
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


def claims_of(evaluation):
    """評価の本文（`parent_evaluation`）から主張の一覧を出す。各主張は種類・文・根拠の参照（evaluation の refs そのまま）。"""
    out = []
    for kind in CLAIM_KINDS:
        for item in (evaluation or {}).get(kind) or []:
            if not isinstance(item, dict):
                continue
            text = item.get("statement") or item.get("claim") or ""
            refs = item.get("references") or []
            out.append({"kind": kind, "statement": text, "refs": refs if isinstance(refs, list) else [refs]})
    return out


def output_from_record(record, usage, case, point, arm):
    """方式 b・c の output.json: 本番の起動口の最終の記録から verdict・next（Rust の検査の後の effective）・claims・usage を出す。
    record（最終の記録の全文）も残す（試験 parent_checkpoint_blind_live_checks が check_evaluation に通す）。"""
    eff = (record or {}).get("effective") or {}
    return {"schema": OUTPUT_SCHEMA, "case": case, "point": point, "arm": arm,
            "verdict": eff.get("verdict"), "next": eff.get("next"),
            "status": (record or {}).get("status"),
            "claims": claims_of((record or {}).get("evaluation")),
            "usage": usage, "record": record}


def output_fixed_plan(decision, rows, children, case, point):
    """方式 a の output.json: 親の途中評価は無い（verdict は無し）。固定の計画は次の子へ進む（next は continue）。claims は
    cutoff 以前の子の局所の review の合格（史実の reviewer_passed）で、根拠は子の task。usage は無し（LLM 0）。"""
    claims = []
    for key, tid in sorted(children.items()):
        passed = [r for r in rows if r.get("task") == key and r.get("type") == "reviewer_passed"]
        if passed:
            claims.append({"kind": "local_review", "statement": f"子 {key} の局所の review が合格（史実の記録）",
                           "refs": [{"kind": "task", "task_id": tid}]})
    return {"schema": OUTPUT_SCHEMA, "case": case, "point": point, "arm": "a",
            "verdict": None, "next": "continue", "status": "fixed_plan",
            "claims": claims, "usage": None,
            "controller_decision": decision}


def tree_listing(root):
    """作業 tree の file（.git を除く）の一覧: path・bytes・sha256。"""
    files = []
    for p in sorted(pathlib.Path(root).rglob("*")):
        rel = p.relative_to(root)
        if p.is_file() and rel.parts[0] != ".git":
            data = p.read_bytes()
            files.append({"path": rel.as_posix(), "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})
    return files


def suppressed_evaluate(rows):
    """controller の判断の行（id, proposal_json, effect_state）から、evaluate の判断で effect_state が suppressed のものを返す。"""
    for did, proposal, state in rows:
        if state == "suppressed" and '"evaluate"' in proposal:
            return {"id": did, "effect_state": state, "proposal": json.loads(proposal)}
    return None


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
            if src.exists():
                files[name] = src.read_bytes()
            elif name == "goal.md":
                # goal.md は全時点で同じ（子 0 の manifest の sha256 が同じ）。fixture は E-mid にだけ置く。
                files[name] = (shadow / "E-mid" / name).read_bytes()
            # reports.json は時点の file だけを使う。他の時点の写しは cutoff の後の報告を含みうる（C-mid に E-mid の報告）。
        return files

    def slice_rows(self):
        files = self.slice_files()
        return [json.loads(line) for line in files["events.jsonl"].decode().splitlines() if line.strip()]

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
max_usd_per_day = {min(self.a.max_usd, MAX_USD_PER_POINT)}

[project_controller.checkpoint]
max_evaluation_attempts = 1
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

    def wait_suppressed(self):
        """方式 a: evaluate の判断が suppressed で記録されるまで待つ（reviewer run は起きない）。"""
        deadline = time.time() + self.a.max_wall_secs
        while time.time() < deadline:
            if self.proc.poll() is not None:
                die("the daemon exited during the shadow evaluation")
            conn = sqlite3.connect(f"file:{self.db}?mode=ro", uri=True)
            rows = conn.execute("SELECT id, proposal_json, effect_state FROM project_controller_decisions "
                                "WHERE project_id = ? ORDER BY created_at, id", (self.seeded["project_id"],)).fetchall()
            conn.close()
            found = suppressed_evaluate(rows)
            if found:
                return found
            time.sleep(1)
        print((self.home / "daemon.log").read_text()[-4000:], file=sys.stderr)
        die(f"no suppressed evaluate decision in {self.a.max_wall_secs} s")

    def reviewer_runs(self):
        conn = sqlite3.connect(f"file:{self.db}?mode=ro", uri=True)
        n = conn.execute("SELECT COUNT(*) FROM runs WHERE role = 'reviewer' AND task_id = ?",
                         (self.seeded["root_id"],)).fetchone()[0]
        conn.close()
        return n

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
        # input/ の作業 tree は seed の直後（daemon と run が artifacts/・runs/ を書く前）に写す。
        self.inp = self.copy_tree(self.out_dir())
        self.write_config()
        self.start()
        self.deploy_ts()
        self.start()
        pid = self.seeded["project_id"]
        # 承認済み Roadmap（版 1）は seed が置く（WU eval-seed）。方式の違いは controller の mode だけ（ADR D1）。
        mode = "shadow" if self.a.arm == "a" else "enabled"
        rev = self.api("GET", f"/api/v1/projects/{pid}/controller")["state"]["revision"]
        self.api("PUT", f"/api/v1/projects/{pid}/controller",
                 {"mode": mode, "expected_revision": rev, "reason": f"対照 pilot の方式 {self.a.arm}（一時 DB）"})
        self.api("POST", f"/api/v1/projects/{pid}/controller/wake",
                 {"external_ref": "blind-start", "note": "開始", "idempotency_key": "blind-start"})
        if self.a.arm == "a":
            decision = self.wait_suppressed()
            # suppressed の後に reviewer run が起きないことを数秒見てから止める。
            time.sleep(3)
            runs = self.reviewer_runs()
            self.stop()
            if runs:
                die(f"arm a started {runs} reviewer run(s)")
            self.collect_fixed_plan(decision)
            return
        final = self.wait_record()
        name = final["artifact"]["name"]
        record = self.artifact_body(final)
        attempts = [e for _, e in self.artifacts() if e["artifact"]["name"].startswith(name[:-len(".json")] + ".attempt-")]
        run_id = attempts[-1]["run_id"] if attempts else final.get("run_id")
        self.stop()
        self.collect(record, run_id)

    def out_dir(self):
        out = pathlib.Path(self.a.out) / self.a.case / self.a.point / self.a.arm
        out.mkdir(parents=True, exist_ok=True)
        return out

    def copy_tree(self, out):
        """input/: 方式に渡した作業 tree（seed の commit）の file の一覧（files.json）と本文（tree/）。"""
        inp = out / "input"
        (inp / "tree").mkdir(parents=True, exist_ok=True)
        files = tree_listing(self.repo)
        for f in files:
            dest = inp / "tree" / f["path"]
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy(self.repo / f["path"], dest)
        dump(inp / "files.json", {"work_tree": str(self.repo), "files": files})
        return inp

    def copy_request(self, inp, run_id):
        """input/request.json・prompt.md: run の dir（runs/<run_id>/）の request.json と、harness が書いた prompt.txt。
        fake の harness（stub）は prompt を書かないので、本物の prompt の関数（parent_checkpoint_section・result_section）で
        request から描く（task-api の試験 `parent_checkpoint_blind_render`）。"""
        run_dirs = [p for p in self.home.rglob(run_id) if p.is_dir() and (p / "request.json").is_file()] if run_id else []
        if not run_dirs:
            die(f"no runs/{run_id}/request.json under {self.home}")
        run_dir = run_dirs[0]
        shutil.copy(run_dir / "request.json", inp / "request.json")
        if (run_dir / "prompt.txt").is_file():
            shutil.copy(run_dir / "prompt.txt", inp / "prompt.md")
            return "harness"
        spec = self.home / "render-spec.json"
        spec.write_text(json.dumps({"request": str(run_dir / "request.json"), "out": str(inp / "prompt.md")}))
        r = subprocess.run([self.a.seed_bin, "--ignored", "--exact", "parent_checkpoint_blind_render", "--nocapture"],
                           env={**self.clean_env(), "PARENT_EVAL_BLIND_RENDER": str(spec)}, capture_output=True, text=True)
        if r.returncode != 0:
            die(f"render failed: {r.stdout[-2000:]} {r.stderr[-2000:]}")
        return "rendered"

    def run_json(self, run_id, row, usage, prompt_source):
        return {"case": self.a.case, "point": self.a.point, "arm": self.a.arm, "run_id": run_id,
                "harness": self.a.harness, "adapter": row[0] if row else None, "model": row[1] if row else None,
                "status": row[3] if row else None, "usage": usage, "usd": (usage or {}).get("cost_usd"),
                "llm_runs": 1 if run_id else 0, "prompt_source": prompt_source,
                "project_id": self.seeded["project_id"], "root_id": self.seeded["root_id"],
                "children": self.seeded["children"]}

    def collect_fixed_plan(self, decision):
        out = self.out_dir()
        dump(self.inp / "request.json", {"arm": "a", "llm_runs": 0, "controller_decision": decision})
        dump(out / "output.json", output_fixed_plan(decision, self.slice_rows(), self.seeded["children"],
                                                    self.a.case, self.a.point))
        dump(out / "reads.json", {"work_tree": str(self.repo), "home": str(self.home), "logs": [], "reads": []})
        dump(out / "run.json", self.run_json(None, None, None, None))

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
        out = self.out_dir()
        prompt_source = self.copy_request(self.inp, run_id)
        dump(out / "output.json", output_from_record(record, usage, self.a.case, self.a.point, self.a.arm))
        dump(out / "reads.json", {"work_tree": str(self.repo), "home": str(self.home), "logs": [str(p) for p in logs],
                                  "reads": reads})
        dump(out / "run.json", self.run_json(run_id, row, usage, prompt_source))
        if not logs:
            die(f"no stdout log for run {run_id} (reads.json is empty)")


def dump(path, value):
    pathlib.Path(path).write_text(json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n")


def main():
    p = argparse.ArgumentParser()
    for k in ("case", "point", "arm", "state", "out", "celeris", "script-deploy", "bundle", "seed-bin", "shadow",
              "harness"):
        p.add_argument(f"--{k}", required=True)
    p.add_argument("--stub")
    p.add_argument("--llm-config")
    p.add_argument("--max-wall-secs", type=int, default=600)
    p.add_argument("--max-usd", type=float, default=1.0)
    a = p.parse_args()
    if a.harness not in ("stub", "config"):
        die("--harness must be stub or config")
    if a.point not in CASES.get(a.case, ()):
        die(f"--case {a.case} --point {a.point} is not a pilot point")
    if a.arm not in ARMS_RUNNABLE:
        die(f"--arm {a.arm} is not runnable (arms {','.join(ARMS_RUNNABLE)})")
    pt = Point(a)
    try:
        pt.run()
    finally:
        pt.stop()
        log = pt.home / "daemon.log"
        if log.exists():
            dest = pathlib.Path(a.out) / a.case / a.point / a.arm
            dest.mkdir(parents=True, exist_ok=True)
            shutil.copy(log, dest / "daemon.log")
        shutil.rmtree(pt.secret_dir, ignore_errors=True)


if __name__ == "__main__":
    main()
