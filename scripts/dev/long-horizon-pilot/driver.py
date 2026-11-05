#!/usr/bin/env python3
"""A9 の非本番 pilot の driver（ADR 2026-11-04-project-controller D11、long-horizon-bench D9）。

一時 home に実 daemon（loopback・一時 DB）を起こし、使い捨ての `pilot:` 案件を 1 試行流し、DB を読み取り専用で開いて
facts（celeris.long-horizon-facts/1、dataset long_horizon_llm_pilot）と操作の記録 ops.jsonl を書く。
LLM を呼ぶかは harness の設定で決まる: `--harness stub` は stub_harness.py（LLM 0）、`--harness config` は
`--llm-config` の TOML（人が書く）をそのまま config に足す。実 LLM は `long-horizon-pilot-llm.sh` が門を持つ。
"""
import argparse
import hashlib
import json
import os
import pathlib
import shutil
import signal
import socket
import sqlite3
import subprocess
import sys
import time
import urllib.error
import urllib.request

HUMAN_TOKEN = "pilot-human-token"
OPERATOR_TOKEN = "pilot-operator-token"
ACTIVE = {"draft", "ready", "running", "blocked", "reviewing"}


SEED_BRANCH = "main"


def make_git_seed(seed, dest):
    """seed を dest にコピーし、git init と初回 commit を作る（dest は案件の repo の primary）。"""
    shutil.copytree(seed, dest, dirs_exist_ok=True)
    env = {**os.environ, "GIT_AUTHOR_NAME": "pilot", "GIT_AUTHOR_EMAIL": "pilot@localhost",
           "GIT_COMMITTER_NAME": "pilot", "GIT_COMMITTER_EMAIL": "pilot@localhost"}
    for cmd in (["init", "-q", "-b", SEED_BRANCH], ["add", "-A"],
                ["commit", "-q", "--allow-empty", "-m", "pilot: repo-seed"]):
        subprocess.run(["git", "-C", str(dest), *cmd], check=True, env=env, capture_output=True)


def die(msg):
    print(f"long-horizon-pilot driver: FAIL: {msg}", file=sys.stderr)
    sys.exit(1)


class Trial:
    def __init__(self, a):
        self.a = a
        self.home = pathlib.Path(a.state).resolve()
        for bad in ("/local/celeris/state", "/local/celeris/data/db", str(pathlib.Path.home() / ".local/celeris"),
                    str(pathlib.Path.home() / ".config/celeris")):
            if str(self.home).startswith(bad):
                die(f"--state must not be under production: {self.home}")
        self.fixtures = pathlib.Path(a.fixtures).resolve()
        self.out = pathlib.Path(a.out).resolve()
        self.proc = None
        self.t0 = None
        self.ops = []
        self.home.mkdir(parents=True, exist_ok=True)
        self.db = self.home / "celeris.sqlite3"
        self.repo = self.home / "trial-repo"
        with socket.socket() as s:
            s.bind(("127.0.0.1", 0))
            self.port = s.getsockname()[1]
        self.base = f"http://127.0.0.1:{self.port}"
        case = json.loads((self.fixtures / "public" / a.case / "case.json").read_text())
        self.case = case
        self.outcomes = case["request_outcomes"]
        rows = [{k: o[k] for k in ("id", "title", "acceptance", "depends_on")} for o in self.outcomes]
        scenario = json.loads((self.fixtures / "oracle" / a.case / "scenario.json").read_text())
        # 公開相当の effect を要する課題だけ stub を繋ぐ（C06）。承認の方針は operator 側の台本で、worker には渡さない。
        self.effect_kinds = {o["id"]: o["effect"] for o in scenario["outcomes"] if o.get("effect") == "publish"}
        self.approvals = {o["id"]: o.get("approval", "approve") for o in scenario["outcomes"] if o["id"] in self.effect_kinds}
        self.effect_dir = self.home / "effect-stub"
        self.approval_ops = []
        self.approval_faults = []
        self.request = (f"pilot ベンチの案件。\n```celeris-outcomes\n{json.dumps(rows, ensure_ascii=False)}\n```\n"
                        "成果は作業 tree の out/<outcome id>.txt に書く。\n")

    # ---- daemon ----
    def write_config(self, allowed):
        a = self.a
        (self.home / "human.token").write_text(HUMAN_TOKEN + "\n")
        (self.home / "operator.token").write_text(OPERATOR_TOKEN + "\n")
        harness = ""
        if a.harness == "stub":
            answers = self.home / "stub-answers.json"
            # 公開の acceptance が成果の作り方（`python3 minichunk.py verify …`）を書く Outcome は、stub も
            # その指示に従って作る（stub_harness.py）。台本の content は、公開の指示の無い Outcome にだけ渡す。
            oracle = json.loads((self.fixtures / "oracle" / a.case / "scenario.json").read_text())
            public_cmd = {o["id"] for o in self.outcomes
                          if any("python3 minichunk.py verify" in x for x in o["acceptance"])}
            answers.write_text(json.dumps({k: v for k, v in oracle["content"].items() if k not in public_cmd}))
            cmd = ["python3", "-I", str(pathlib.Path(__file__).with_name("stub_harness.py")),
                   "--answers", str(answers), "--case", a.case]
            if self.effect_kinds:
                cmd += ["--effects-dir", str(self.effect_dir), "--effect-kinds", json.dumps(self.effect_kinds)]
                if a.stub_early_publish:
                    cmd.append("--early-publish")
            harness = f"""
[adapters.fake]
command = {json.dumps(cmd)}

[llm_sources.local]
kind = "fake"
accounts = ["a1"]
models = ["model-1"]

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
            route = '[{ source = "local", model = "model-1" }]'
        else:
            harness = pathlib.Path(a.llm_config).read_text()
            route = a.cos_route
        knowledge = self.home / "knowledge"
        knowledge.mkdir(exist_ok=True)
        (self.home / "ws").mkdir(exist_ok=True)
        (self.home / "config.toml").write_text(f"""
db = "celeris.sqlite3"
workspace_root = "ws"
tick_ms = 100
max_concurrency = 2

[api]
listen = "127.0.0.1:{self.port}"

[[api.tokens]]
principal = "human:pilot"
token_file = "human.token"

[[api.tokens]]
principal = "operator:pilot"
token_file = "operator.token"

[cos]
enabled = true
max_turns = {a.cos_max_turns}
max_wall_secs = {a.cos_max_wall}
route_candidates = {route}

[knowledge]
root = {json.dumps(str(knowledge))}

[workspace]
build_cache_dir = {json.dumps(str(self.home / "build-cache"))}

[scratch]
dir = {json.dumps(str(self.home / "scratch"))}

[script]
state_dir = "state"

[project_controller]
enabled = true
allowed_project_ids = {json.dumps(allowed)}
coalesce_secs = {a.coalesce_secs}

[project_controller.budget]
max_roots_per_day = {a.max_roots_per_day}
max_tokens_per_day = {a.max_tokens_per_day}
{harness}
""")

    def start(self):
        log = open(self.home / "daemon.log", "ab")
        env = {k: v for k, v in os.environ.items() if not k.startswith("CELERIS_") and k not in
               ("CLAUDECODE", "CLAUDE_CODE_SESSION_ID")}
        self.proc = subprocess.Popen(
            [self.a.celeris, "--config", str(self.home / "config.toml"), "--log-format", "text", "--release", "pilot"],
            stdout=log, stderr=log, stdin=subprocess.DEVNULL, env=env, cwd=self.home)
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

    def stop(self):
        if self.proc is not None and self.proc.poll() is None:
            self.proc.send_signal(signal.SIGTERM)
            try:
                self.proc.wait(timeout=60)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        self.proc = None

    def api(self, method, path, body=None, token=HUMAN_TOKEN):
        req = urllib.request.Request(self.base + path, method=method,
                                     data=None if body is None else json.dumps(body).encode(),
                                     headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=30) as r:
            data = r.read()
        return json.loads(data) if data else None

    def api_status(self, method, path, body=None, token=HUMAN_TOKEN):
        try:
            return 200, self.api(method, path, body, token)
        except urllib.error.HTTPError as e:
            text = e.read().decode(errors="replace")
            try:
                return e.code, json.loads(text)
            except ValueError:
                return e.code, {"text": text}

    def op(self, name, **detail):
        row = {"step": name, "at": round(time.time() - (self.t0 or time.time()), 1), **detail}
        self.ops.append(row)
        print(f"pilot: {name} {json.dumps(detail, ensure_ascii=False)[:200]}", flush=True)

    # ---- setup ----
    def seed_repo(self):
        """repo-seed/ を git 作業場所にする（git init と初回 commit）。成果は stub／worker が commit して target branch に載る。"""
        seed = self.fixtures / "public" / self.a.case / "repo-seed"
        make_git_seed(seed, self.repo)

    def deploy_ts(self):
        """本物の bundle を配備し、一時 DB の領域 project_controller を ts にする。

        compare→canary の昇格の計数は本番の compare が持つので、一時 DB では行を直接置く（synthetic として記録する）。"""
        state_dir = self.home / "state"
        sha = subprocess.run([self.a.script_deploy, "--dist", self.a.bundle, "--state-dir", str(state_dir)],
                             check=True, capture_output=True, text=True).stdout.strip()
        self.api("POST", "/api/v1/script/generations", {"bundle_sha256": sha})
        self.stop()
        conn = sqlite3.connect(self.db)
        gen = conn.execute("SELECT id FROM script_generations WHERE bundle_sha256 = ?", (sha,)).fetchone()[0]
        now = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        conn.execute("UPDATE script_domain_modes SET mode='ts', generation_id=?, version=version+1, updated_at=?, "
                     "updated_by='human:pilot' WHERE domain='project_controller'", (gen, now))
        if conn.total_changes != 1:
            die("script_domain_modes has no project_controller row")
        conn.execute("UPDATE script_generations SET state='active' WHERE id=?", (gen,))
        conn.commit()
        conn.close()
        self.op("deploy_ts", bundle_sha256=sha, mode_seed="synthetic: temp DB only")

    def tree_digest(self):
        h = hashlib.sha256()
        for p in sorted(self.repo.rglob("*")):
            if p.is_file() and ".git" not in p.parts:
                h.update(str(p.relative_to(self.repo)).encode() + b"\0" + p.read_bytes() + b"\0")
        return h.hexdigest()

    # ---- 試行 ----
    def run(self):
        a = self.a
        self.seed_repo()
        if self.effect_kinds:
            for sub in ("pending", "decisions"):
                (self.effect_dir / sub).mkdir(parents=True, exist_ok=True)
            (self.effect_dir / "effects.jsonl").write_text("")
        self.write_config([])
        self.start()
        self.t0 = time.time()
        project = self.api("POST", "/api/v1/projects", {
            "title": f"pilot:{a.case}-{a.seed}", "request": self.request,
            "workspace": {"kind": "local", "path": str(self.repo)}})
        self.pid = project["id"] if "id" in project else project["project"]["id"]
        # 作業 tree は git。案件の repo に登録し、root の done を IntegrationMerged で取り込み済みにする。
        self.api("POST", f"/api/v1/projects/{self.pid}/repos", {
            "name": "trial-repo", "kind": "git", "location": {"kind": "local", "path": str(self.repo)},
            "default_branch": SEED_BRANCH, "is_primary": True})
        self.api("PATCH", f"/api/v1/projects/{self.pid}", {"auto_integrate": True})
        self.init_digest = hashlib.sha256((self.request + self.tree_digest()).encode()).hexdigest()
        self.deploy_ts()
        self.write_config([self.pid])
        self.start()
        self.op("project_created", project=self.pid, arm=a.arm)
        # 人でない主体は mode を上げられない（D8）。
        code, body = self.api_status("PUT", f"/api/v1/projects/{self.pid}/controller",
                                     {"mode": "enabled", "expected_revision": 0, "reason": "operator が上げる"},
                                     OPERATOR_TOKEN)
        if code != 403:
            die(f"operator raised the mode: {code} {body}")
        self.op("operator_cannot_raise_mode", status=code, code=body.get("code"))
        if a.arm == "new":
            self.set_mode("enabled", "pilot を有効にする")
            self.api("POST", f"/api/v1/projects/{self.pid}/controller/wake",
                     {"external_ref": "pilot-start", "note": "開始", "idempotency_key": "start"})
        else:
            self.say_to_project_cos("この案件の途中目標を順に進めてください。")
        self.wait_end()
        self.after_end()
        self.collect()

    def set_mode(self, mode, reason):
        rev = self.controller()["state"]["revision"]
        v = self.api("PUT", f"/api/v1/projects/{self.pid}/controller",
                     {"mode": mode, "expected_revision": rev, "reason": reason})
        self.op("set_mode", mode=mode, reason=reason, revision=v.get("revision"))

    def controller(self):
        return self.api("GET", f"/api/v1/projects/{self.pid}/controller")

    def say_to_project_cos(self, text):
        thread = self.api("POST", "/api/v1/chat/threads", {"project_id": self.pid})["thread"]["id"]
        self.api("POST", f"/api/v1/chat/threads/{thread}/messages", {
            "client_message_id": "pilot-1", "text": text, "attachment_ids": [], "reply_to_id": None,
            "mode": "queue", "resume_queue": False})
        self.op("human_message", thread=thread)

    def root_rows(self):
        conn = sqlite3.connect(f"file:{self.db}?mode=ro", uri=True)
        conn.row_factory = sqlite3.Row
        rows = conn.execute("SELECT id, status, json FROM tasks WHERE project_id = ? AND "
                            "json_extract(json,'$.parent_id') IS NULL ORDER BY id", (self.pid,)).fetchall()
        conn.close()
        return rows

    def wait_end(self):
        """到達・停止・上限のどれかまで待つ（出来事を DB と API で読む。負荷は掛けない）。"""
        deadline = time.time() + self.a.max_wall_secs
        last = None
        quiet_since = time.time()
        while time.time() < deadline:
            self.serve_approvals()
            view = self.controller()
            phase = view["state"]["phase"]
            roots = self.root_rows()
            state = (phase, tuple(r["status"] for r in roots))
            if state != last:
                self.op("state", phase=phase, roots=[r["status"] for r in roots])
                last = state
                quiet_since = time.time()
            if self.proc.poll() is not None:
                die("the daemon exited during the trial")
            if self.a.arm == "new" and phase in ("achieved", "blocked", "cancelled"):
                return
            if self.a.arm != "new" and time.time() - quiet_since > self.a.old_quiet_secs and not any(
                    r["status"] in ACTIVE for r in roots):
                return
            time.sleep(1)
        self.op("wall_limit", secs=self.a.max_wall_secs)

    def serve_approvals(self):
        """承認待ちの effect を operator の台本（approve / reject）で解く。承認待ちと承認を facts に残す。"""
        if not self.effect_kinds:
            return
        for pending in sorted((self.effect_dir / "pending").glob("*.json")):
            decision = self.effect_dir / "decisions" / pending.name
            if decision.exists():
                continue
            logical = json.loads(pending.read_text())["logical_id"]
            outcome = logical.split(":")[1]
            now = time.time()
            self.approval_faults.append(max(0, int(json.loads(pending.read_text())["at"] - self.t0)))
            self.approval_ops.append(max(0, int(now - self.t0)))
            decision.write_text(json.dumps({"decision": self.approvals[outcome], "at": now}))
            self.op("approval", logical_id=logical, decision=self.approvals[outcome])

    def read_effects(self):
        """effect stub の記録（worker の外）を読む。読めなければ観測失敗で、0 件にはしない。"""
        if not self.effect_kinds:
            return []
        record = self.effect_dir / "effects.jsonl"
        if not record.exists():
            die("observation_failed: the effect stub record is missing")
        rows = []
        for line in record.read_text().splitlines():
            e = json.loads(line)
            rows.append({"logical_id": e["logical_id"], "root": e["logical_id"].split(":")[1],
                         "at": max(0, int(e["at"] - self.t0))})
        return rows

    def after_end(self):
        pid = self.pid
        n0 = len(self.root_rows())
        d0 = len(self.api("GET", f"/api/v1/projects/{pid}/controller/decisions?limit=200")["items"])
        self.api("POST", f"/api/v1/projects/{pid}/controller/wake",
                 {"external_ref": "pilot-after-end", "note": "終わった後の wake", "idempotency_key": "after-end"})
        time.sleep(self.a.coalesce_secs + 2)
        self.op("wake_after_end", new_roots=len(self.root_rows()) - n0)
        # 従来の CoS への rollback: off に戻すと評価が止まり、wake を受けない。
        if self.a.arm == "new":
            self.set_mode("off", "従来の CoS に戻す")
            code, body = self.api_status("POST", f"/api/v1/projects/{pid}/controller/wake",
                                         {"external_ref": "pilot-after-off", "note": "off の後",
                                          "idempotency_key": "after-off"})
            self.op("wake_after_off", status=code, added=(body or {}).get("added"))
            time.sleep(self.a.coalesce_secs + 2)
            d1 = len(self.api("GET", f"/api/v1/projects/{pid}/controller/decisions?limit=200")["items"])
            view = self.controller()
            self.op("after_off", decisions_at_end=d0, decisions_after=d1, mode=view["state"]["mode"],
                    pending_wakes=len(view["pending_wakes"]))
        self.stop()

    # ---- collector ----
    def collect(self):
        a = self.a
        conn = sqlite3.connect(f"file:{self.db}?mode=ro", uri=True)
        conn.row_factory = sqlite3.Row
        t0_iso = time.gmtime(self.t0)
        import calendar
        from datetime import datetime

        def secs(iso):
            if not iso:
                return 0
            dt = datetime.fromisoformat(iso.replace("Z", "+00:00"))
            return max(0, int(dt.timestamp() - self.t0))

        roots = []
        by_id = {}
        for r in self.root_rows_full(conn):
            t = json.loads(r["json"])
            texts = [c["text"] for c in t.get("acceptance", [])]
            outcome = None
            for o in self.outcomes:
                if all(x in texts for x in o["acceptance"]):
                    outcome = o["id"]
            labels = t.get("labels") or []
            daemon = any(l.startswith("project-controller") or l == "project_controller" for l in labels)
            statuses = ["ready"]
            for e in conn.execute("SELECT json FROM events WHERE task_id = ? ORDER BY id", (r["id"],)):
                ev = json.loads(e["json"])
                if ev.get("type") == "transitioned" and ev.get("to") != statuses[-1]:
                    statuses.append(ev["to"])
            attempt = sum(1 for x in roots if x["outcome"] == outcome) + 1
            row = {"logical_id": f"{outcome}:{attempt}", "outcome": outcome, "attempt": attempt,
                   "created_by": "daemon" if daemon else "cos", "instructed": not daemon,
                   "created_at": secs(t.get("created_at")), "statuses": statuses,
                   "acceptance_sha256": hashlib.sha256("\n".join(texts).encode()).hexdigest()}
            roots.append(row)
            by_id[r["id"]] = row
        invocations = []
        tokens = 0
        tokens_known = a.harness != "stub"
        for r in conn.execute("SELECT r.role, r.started_at, r.usage_json FROM runs r JOIN tasks t ON t.id = r.task_id "
                              "WHERE t.project_id = ? ORDER BY r.started_at", (self.pid,)):
            usage = json.loads(r["usage_json"]) if r["usage_json"] else None
            if usage:
                tokens += sum(int(usage.get(k) or 0) for k in
                              ("input_tokens", "output_tokens", "cache_creation_input_tokens"))
            elif a.harness != "stub":
                tokens_known = False
            invocations.append({"at": secs(r["started_at"]), "role": r["role"] or "worker", "usage": usage})
        threads = [x["id"] for x in conn.execute("SELECT id FROM chat_threads WHERE project_id = ?", (self.pid,))]
        for th in threads:
            for r in conn.execute("SELECT started_at AS created_at FROM chat_runs WHERE thread_id = ? ORDER BY started_at", (th,)):
                invocations.append({"at": secs(r["created_at"]), "role": "cos", "usage": None})
        invocations.sort(key=lambda i: i["at"])
        questions = [x["source_key"].split(f":{self.pid}:", 1)[-1] for x in conn.execute(
            "SELECT source_key FROM feed_sources WHERE source_key LIKE ?",
            (f"project_controller:escalate:{self.pid}:%",))]
        decisions = conn.execute("SELECT proposal_json, validation, effect_state, created_at FROM "
                                 "project_controller_decisions WHERE project_id = ? ORDER BY created_at",
                                 (self.pid,)).fetchall()
        achieved_at = None
        rules = []
        for d in decisions:
            try:
                rule = json.loads(d["proposal_json"]).get("explanation", {}).get("rule_id")
            except ValueError:
                rule = None
            rules.append(rule or d["validation"])
            if rule == "pc/achieved" and achieved_at is None:
                achieved_at = secs(d["created_at"])
        running = conn.execute("SELECT COUNT(*) FROM tasks WHERE project_id = ? AND status = 'running'",
                               (self.pid,)).fetchone()[0]
        conn.close()
        trial = hashlib.sha256(f"{a.salt}|{a.case}|{a.arm}|{a.seed}".encode()).hexdigest()[:16]
        wall = int(time.time() - self.t0)
        usage = {"tokens": tokens if tokens_known else None, "quota": None, "wall_secs": wall,
                 "missing": "tier=stub" if a.harness == "stub" else None}
        effects = self.read_effects()
        facts = {
            "schema": "celeris.long-horizon-facts/1", "bench_version": json.loads(
                (self.fixtures / "oracle" / a.case / "oracle.json").read_text())["bench_version"],
            "dataset": "long_horizon_llm_pilot", "case": a.case, "seed": a.seed, "init_digest": self.init_digest,
            "horizon_secs": a.max_wall_secs, "roots": roots, "effects": effects,
            "human_ops": [{"at": 0, "class": "stimulus", "kind": "project_request"}] + [
                {"at": at, "class": "required", "kind": "approval"} for at in self.approval_ops],
            "faults": [{"at": at, "kind": "approval_wait"} for at in self.approval_faults],
            "invocations": invocations,
            "actors": [{"actor": "human", "at": 0}] + [{"actor": i["role"], "at": i["at"]} for i in invocations],
            "statements": [], "questions": questions, "cos_launch_rejected": 0, "running_untracked": running,
            "achieved_at": achieved_at, "events_append_only": True, "usage": usage,
            "virtual_secs_to_reach": achieved_at,
        }
        for sub in ("facts", "arm-map", "trials"):
            (self.out / sub).mkdir(parents=True, exist_ok=True)
        (self.out / "facts" / f"{trial}.json").write_text(json.dumps(facts, ensure_ascii=False, indent=1) + "\n")
        (self.out / "arm-map" / f"{trial}.json").write_text(json.dumps({"arm": a.arm, "case": a.case, "seed": a.seed}) + "\n")
        repo_out = self.out / "trials" / trial / "repo"
        shutil.rmtree(repo_out, ignore_errors=True)
        shutil.copytree(self.repo, repo_out, ignore=shutil.ignore_patterns(".git", "artifacts", "runs", ".taskd"))
        rec = {"schema": "celeris.long-horizon-pilot-trial/1", "trial": trial, "harness": a.harness, "arm": a.arm,
               "case": a.case, "project": self.pid, "wall_secs": wall, "rules": rules,
               "root_statuses": [r["statuses"][-1] for r in roots], "ops": self.ops}
        (self.out / "ops").mkdir(exist_ok=True)
        (self.out / "ops" / f"{trial}.json").write_text(json.dumps(rec, ensure_ascii=False, indent=1) + "\n")
        print(f"pilot: trial {trial} wrote {self.out}", flush=True)

    def root_rows_full(self, conn):
        return conn.execute("SELECT id, status, json FROM tasks WHERE project_id = ? AND "
                            "json_extract(json,'$.parent_id') IS NULL ORDER BY id", (self.pid,)).fetchall()


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--state", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--fixtures", required=True)
    p.add_argument("--bundle", required=True)
    p.add_argument("--celeris", required=True)
    p.add_argument("--script-deploy", required=True)
    p.add_argument("--case", default="C08")
    p.add_argument("--arm", default="new", choices=["new", "old.passive"])
    p.add_argument("--seed", type=int, default=1)
    p.add_argument("--salt", default="a9-pilot")
    p.add_argument("--harness", default="stub", choices=["stub", "config"])
    p.add_argument("--llm-config")
    p.add_argument("--cos-route", default="[]")
    p.add_argument("--cos-max-turns", type=int, default=2)
    p.add_argument("--cos-max-wall", type=int, default=30)
    p.add_argument("--coalesce-secs", type=int, default=1)
    p.add_argument("--max-roots-per-day", type=int, default=10)
    p.add_argument("--max-tokens-per-day", type=int, default=1_000_000)
    p.add_argument("--max-wall-secs", type=int, default=300)
    p.add_argument("--stub-early-publish", action="store_true",
                   help="陰性の試験: stub が承認を待たずに publish を記録する")
    p.add_argument("--old-quiet-secs", type=int, default=10)
    a = p.parse_args()
    trial = Trial(a)
    try:
        trial.run()
    finally:
        trial.stop()


if __name__ == "__main__":
    main()
