#!/usr/bin/env python3
"""P4-F: 実 daemon 全体（API・dispatcher・CoS chat・cos_digest・cron・worker）の長時間の資源の計測（非本番）。

P1-I（`crates/celeris/tests/long_run_soak.rs`）は `build_dispatcher` の dispatcher だけを測った。この driver は本物の
`celeris` binary を一時の home で起こし、4 条件（off・v8_load・shadow・enabled）を同じ負荷で揃えて走らせる。

  run      1 条件を走らせ、`samples.jsonl`（schema celeris.daemon-soak/1）と `ops.jsonl` を書く。
  summary  1 つ以上の run の dir を集計し、`summary.json`・`summary.md` を書く（本番の観測と同じ式・同じ経過秒）。
  plan     24/72 時間の実施の範囲（時間・条件・host・停止条件・承認の雛形）を出す。何も起こさない。

境界（ADR 2026-11-05-phase4-p4f-daemon-soak）:
- LLM を呼ばない（fake harness は `soak_harness.py`）。外部ネットワークに出ない（API は 127.0.0.1）。
- 本番の DB・config・systemd・releases に触れない（`--state` が本番の path の下なら exit 2）。
- `--secs` が [SHORT_LIMIT_SECS]（1 時間）を超える run は、人の承認の file（`--approval`）が要る。Celeris の worker の
  run の中（`CELERIS_RUN_ID` がある）では、承認があっても 1 時間を超える run を起こさない。
- 自動停止: 壁時計・RSS の上限・disk の空き・DB の大きさ・daemon の終了・API の連続の失敗。止めた理由は `ops.jsonl` の
  最後の行（`step = "stop"`）と summary に残る。driver が SIGTERM を受けたとき（`pkill` など）も止まる理由
  `terminated` を書き、daemon を止め、daemon の log を `--out` に写してから終わる。
- CPU を焼かない: 負荷は task の投入（既定 10 秒に 1 件）と chat（既定 5 分に 1 通）で決め、tick は 100 ms。
"""
import argparse
import datetime
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

SCHEMA = "celeris.daemon-soak/1"
SUMMARY_SCHEMA = "celeris.daemon-soak-summary/1"
CONDITIONS = ["off", "v8_load", "shadow", "enabled"]
SHORT_LIMIT_SECS = 3600
PRODUCTION_PREFIXES = ("/local/celeris/state", "/local/celeris/data/db",
                       str(pathlib.Path.home() / ".local/celeris"), str(pathlib.Path.home() / ".config/celeris"))
# 本番の観測（2026-11-04、P1-E の shadow。long_run_soak_summary.py と同じ値）: (最初の標本からの秒, VmRSS kB)。
PROD_SAMPLES = [[0, 181820], [600, 391292], [1200, 505532], [1529, 588668]]
ARENA_HEAP = 64 << 20  # glibc の 64-bit の非 main arena の heap（HEAP_MAX_SIZE、64 MiB 境界に揃う）
PM_KEYS = ("rss_kb", "rss_anon_kb", "rss_file_kb", "threads", "sqlite_memory_used", "sqlite_memory_highwater",
           "sqlite_pagecache_bytes", "sqlite_malloc_count", "malloc_arena", "malloc_mmap", "malloc_in_use",
           "malloc_free", "v8_generations", "v8_used_heap", "v8_total_heap", "v8_external", "v8_malloced")


# ---------------------------------------------------------------- 純粋な関数（test_daemon_soak.py が試す）

def gate(secs, approval, cond, env, now=None):
    """実施の門。(ok, reason)。1 時間以内は承認なしで通す。超えるなら承認の file の中身を確かめる。"""
    if secs <= 0:
        return False, "secs must be positive"
    if secs <= SHORT_LIMIT_SECS:
        return True, "short"
    if env.get("CELERIS_RUN_ID"):
        return False, "refusing a run over 1 hour inside a Celeris worker run (CELERIS_RUN_ID is set)"
    if approval is None:
        return False, f"--secs {secs} exceeds {SHORT_LIMIT_SECS}: a human approval file (--approval) is required"
    by = str(approval.get("approved_by", ""))
    if not re.fullmatch(r"human:\S+", by):
        return False, "approval.approved_by must be human:<name>"
    if approval.get("scope") != "non-production":
        return False, "approval.scope must be non-production"
    if not isinstance(approval.get("max_secs"), int) or approval["max_secs"] < secs:
        return False, f"approval.max_secs ({approval.get('max_secs')}) is below --secs {secs}"
    if cond not in (approval.get("conditions") or []):
        return False, f"condition {cond} is not in approval.conditions"
    exp = approval.get("expires_at")
    try:
        expires = datetime.datetime.fromisoformat(str(exp).replace("Z", "+00:00"))
    except ValueError:
        return False, "approval.expires_at must be an ISO 8601 time"
    now = now or datetime.datetime.now(datetime.timezone.utc)
    if expires <= now:
        return False, "the approval has expired"
    return True, f"approved by {by}"


class Terminated(Exception):
    """driver が SIGTERM を受けた。"""


def raise_terminated(signum, frame):
    raise Terminated()


def production_path(path):
    p = str(pathlib.Path(path).resolve())
    return any(p == x or p.startswith(x + "/") for x in PRODUCTION_PREFIXES)


def stop_reason(sample, limits, elapsed, secs, api_failures):
    """自動停止の理由（止めないなら None）。順に: daemon の終了・RSS・disk・DB・API・壁時計。"""
    if sample.get("daemon_exit") is not None:
        return f"daemon_exited:{sample['daemon_exit']}"
    rss = sample.get("proc", {}).get("rss_kb") or 0
    if rss > limits["max_rss_mb"] * 1024:
        return f"rss_limit:{rss}kB>{limits['max_rss_mb']}MB"
    free = sample.get("disk_free_mb")
    if free is not None and free < limits["min_free_mb"]:
        return f"disk_free:{free}MB<{limits['min_free_mb']}MB"
    db = sample.get("db_bytes") or 0
    if db > limits["max_db_mb"] << 20:
        return f"db_size:{db}B>{limits['max_db_mb']}MB"
    if api_failures >= limits["max_api_failures"]:
        return f"api_down:{api_failures}"
    if elapsed >= secs:
        return "wall"
    return None


def parse_proc_status(text):
    out = {}
    names = {"VmRSS:": "rss_kb", "RssAnon:": "rss_anon_kb", "RssFile:": "rss_file_kb", "RssShmem:": "rss_shmem_kb",
             "Threads:": "threads", "VmSwap:": "swap_kb"}
    for line in text.splitlines():
        parts = line.split()
        if len(parts) >= 2 and parts[0] in names and parts[1].isdigit():
            out[names[parts[0]]] = int(parts[1])
    return out


def arena_estimate(maps_text):
    """非 main の malloc の arena の数の推定: 64 MiB 境界に始まる名前なしの rw-p の領域で、続く ---p と合わせて 64 MiB。"""
    rows = []
    for line in maps_text.splitlines():
        parts = line.split()
        if len(parts) < 5:
            continue
        lo, hi = (int(x, 16) for x in parts[0].split("-"))
        rows.append((lo, hi, parts[1], len(parts) >= 6))
    n = 0
    for i, (lo, hi, perms, named) in enumerate(rows):
        if named or perms != "rw-p" or lo % ARENA_HEAP:
            continue
        end = hi
        if i + 1 < len(rows) and rows[i + 1][0] == hi and rows[i + 1][2] == "---p" and not rows[i + 1][3]:
            end = rows[i + 1][1]
        if end - lo == ARENA_HEAP:
            n += 1
    return n


def parse_process_memory(line):
    """daemon の log の `process_memory` の行（text 形式の key=value）を dict にする。違う行なら None。"""
    line = ANSI.sub("", line)
    if "process_memory" not in line:
        return None
    vals = dict(re.findall(r"\b([a-z0-9_]+)=(-?\d+)\b", line))
    if "rss_kb" not in vals:
        return None
    return {k: int(vals[k]) for k in PM_KEYS if k in vals}


def last_process_memory(log_path, start_at=0):
    """log の start_at（byte）以降の最後の `process_memory` の行と、読み終えた位置。"""
    last = None
    try:
        with open(log_path, "rb") as f:
            f.seek(start_at)
            data = f.read()
    except OSError:
        return None, start_at
    for raw in data.decode(errors="replace").splitlines():
        pm = parse_process_memory(raw)
        if pm:
            last = pm
    return last, start_at + len(data)


def slope(p, q):
    """2 点 (秒, kB) の傾きを MB/時（P1-I と同じ式）。"""
    dt = q[0] - p[0]
    return None if dt <= 0 else round((q[1] - p[1]) / 1024 / (dt / 3600), 1)


def nearest(points, offset):
    return min(points, key=lambda p: abs(p[0] - offset))


def pct(values, p):
    v = sorted(values)
    return v[(len(v) - 1) * p // 100] if v else None


ANSI = re.compile(r"\x1b\[[0-9;]*m")


def ts_epoch(text):
    """RFC 3339（小数は 9 桁まで）を epoch 秒にする。"""
    m = re.fullmatch(r"(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)(?:\.(\d+))?Z", text)
    if not m:
        raise ValueError(text)
    base = datetime.datetime.fromisoformat(m.group(1) + "+00:00").timestamp()
    return base + (float("0." + m.group(2)) if m.group(2) else 0.0)


def count_rows(conn):
    """daemon の状態の行数: task・run・リース・event・wake・判断・比較・chat・受信箱・cron と、event の取り込みの遅れ。"""
    out = {}
    tables = {r[0] for r in conn.execute("SELECT name FROM sqlite_master WHERE type='table'")}
    one = lambda sql: conn.execute(sql).fetchone()[0]
    for t in ("tasks", "runs", "events", "project_wakes", "project_controller_decisions", "script_compare_log",
              "chat_runs", "chat_messages", "cos_inbox_items", "cron_job_runs", "work_units"):
        if t in tables:
            out[t] = one(f"SELECT COUNT(*) FROM {t}")
    if "tasks" in tables:
        out["tasks_by_status"] = dict(conn.execute("SELECT status, COUNT(*) FROM tasks GROUP BY status"))
        out["task_leases"] = one("SELECT COUNT(*) FROM tasks WHERE lease_worker_run_id IS NOT NULL")
    if "runs" in tables:
        out["runs_running"] = one("SELECT COUNT(*) FROM runs WHERE status = 'running'")
    if "work_units" in tables:
        out["wu_leases"] = one("SELECT COUNT(*) FROM work_units WHERE lease_run_id IS NOT NULL")
    if "project_wakes" in tables:
        out["wakes_unconsumed"] = one("SELECT COUNT(*) FROM project_wakes WHERE consumed_by IS NULL")
    if "events" in tables:
        top = one("SELECT COALESCE(MAX(rowid), 0) FROM events")
        out["events_max_rowid"] = top
        if "feed_cursor" in tables:
            lag = {}
            for name, value in conn.execute("SELECT name, value FROM feed_cursor"):
                if str(value).isdigit() and name in ("events", "cos_triage:events", "project_wake", "root_done_summary"):
                    lag[name] = top - int(value)
            out["event_lag"] = lag
    return out


# ---------------------------------------------------------------- daemon

class Soak:
    def __init__(self, a):
        self.a = a
        self.home = pathlib.Path(a.state).resolve()
        self.out = pathlib.Path(a.out).resolve()
        self.out.mkdir(parents=True, exist_ok=True)
        self.home.mkdir(parents=True, exist_ok=True)
        self.db = self.home / "celeris.sqlite3"
        self.log = self.home / "daemon.log"
        self.token = secrets.token_urlsafe(32)
        self.secret_dir = pathlib.Path(tempfile.mkdtemp(prefix="soak-token-"))
        self.token_file = self.secret_dir / "human.token"
        with socket.socket() as s:
            s.bind(("127.0.0.1", 0))
            self.port = s.getsockname()[1]
        self.base = f"http://127.0.0.1:{self.port}"
        self.proc = None
        self.t0 = None
        self.log_pos = 0
        self.pm = None
        self.latency_rowid = 0
        (self.out / "samples.jsonl").write_text("")
        (self.out / "ops.jsonl").write_text("")

    def op(self, step, **detail):
        row = {"schema": SCHEMA, "step": step, "at": round(time.time() - (self.t0 or time.time()), 1),
               "condition": self.a.cond, "label": self.a.label, **detail}
        with open(self.out / "ops.jsonl", "a") as f:
            f.write(json.dumps(row, ensure_ascii=False) + "\n")
        print(f"daemon-soak[{self.a.label}/{self.a.cond}]: {step} {json.dumps(detail, ensure_ascii=False)[:200]}",
              flush=True)

    def write_config(self):
        harness = [sys.executable, "-I", str(pathlib.Path(__file__).with_name("soak_harness.py")),
                   "--sleep", str(self.a.harness_sleep)]
        script = '[script]\nstate_dir = "state"\n' if self.a.cond != "off" else ""
        for d in ("ws", "knowledge", "build-cache", "scratch", "releases"):
            (self.home / d).mkdir(exist_ok=True)
        (self.home / "config.toml").write_text(f"""
db = "celeris.sqlite3"
workspace_root = "ws"
tick_ms = {self.a.tick_ms}
max_concurrency = 2

[api]
listen = "127.0.0.1:{self.port}"

[[api.tokens]]
principal = "human:soak"
token_file = {json.dumps(str(self.token_file))}

[cos]
enabled = true
max_turns = 4
max_wall_secs = 120
route_candidates = [{{ source = "local", model = "model-1" }}]

[knowledge]
root = {json.dumps(str(self.home / "knowledge"))}

[workspace]
build_cache_dir = {json.dumps(str(self.home / "build-cache"))}

[scratch]
dir = {json.dumps(str(self.home / "scratch"))}

[selfdeploy]
releases_dir = {json.dumps(str(self.home / "releases"))}

[maintenance]
disk_watch = []

{script}
[project_controller]
enabled = true
allowed_project_ids = {json.dumps([self.pid] if getattr(self, "pid", None) else [])}

[adapters.fake]
command = {json.dumps(harness)}

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
""")

    def start(self):
        fd = os.open(self.token_file, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w") as f:
            f.write(self.token + "\n")
        env = {k: v for k, v in os.environ.items() if not k.startswith("CELERIS_")
               and k not in ("CLAUDECODE", "CLAUDE_CODE_SESSION_ID")}
        env.setdefault("RUST_LOG", "info")
        env["NO_COLOR"] = "1"
        logf = open(self.log, "ab")
        self.proc = subprocess.Popen([self.a.celeris, "--config", str(self.home / "config.toml"), "--log-format",
                                      "text", "--release", "soak"], stdout=logf, stderr=logf,
                                     stdin=subprocess.DEVNULL, env=env, cwd=self.home)
        try:
            for _ in range(600):
                if self.proc.poll() is not None:
                    sys.stderr.write(self.log.read_text(errors="replace")[-3000:])
                    raise SystemExit("daemon-soak: the daemon exited at startup")
                try:
                    self.api("GET", "/api/v1/daemon")
                    return
                except (urllib.error.URLError, ConnectionError, OSError):
                    time.sleep(0.2)
            raise SystemExit("daemon-soak: the daemon API did not answer in 120 s")
        finally:
            self.token_file.unlink(missing_ok=True)

    def stop(self, sig=signal.SIGTERM):
        if self.proc is not None and self.proc.poll() is None:
            self.proc.send_signal(sig)
            try:
                self.proc.wait(timeout=60)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        self.proc = None

    def api(self, method, path, body=None):
        req = urllib.request.Request(self.base + path, method=method,
                                     data=None if body is None else json.dumps(body).encode(),
                                     headers={"Authorization": f"Bearer {self.token}",
                                              "Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=30) as r:
            data = r.read()
        return json.loads(data) if data else None

    # ---- 準備 ----
    def setup(self):
        self.write_config()
        self.start()
        self.t0 = time.time()
        project = self.api("POST", "/api/v1/projects", {
            "title": f"soak:{self.a.cond}", "request": "長時間の資源の計測の案件（P4-F、fake harness）。",
            "workspace": {"kind": "local", "path": str(self.home / "ws")}})
        self.pid = project["id"] if "id" in project else project["project"]["id"]
        self.thread = self.api("POST", "/api/v1/chat/threads", {"project_id": self.pid})["thread"]["id"]
        modes = {}
        if self.a.cond != "off":
            sha = subprocess.run([self.a.script_deploy, "--dist", self.a.bundle, "--state-dir",
                                  str(self.home / "state")], check=True, capture_output=True, text=True).stdout.strip()
            self.api("POST", "/api/v1/script/generations", {"bundle_sha256": sha})
            modes = {"v8_load": {}, "shadow": {"lane_rules": "compare", "cos_digest": "compare"},
                     "enabled": {"lane_rules": "ts", "cos_digest": "compare"}}[self.a.cond]
        self.stop()
        if modes or self.a.cond != "off":
            # mode の昇格（compare→canary→ts）の計数は本番の compare が持つ。一時 DB では行を直接置く（synthetic）。
            conn = sqlite3.connect(self.db)
            gen = conn.execute("SELECT id FROM script_generations ORDER BY rowid DESC LIMIT 1").fetchone()[0]
            now = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
            conn.execute("UPDATE script_generations SET state='active' WHERE id=?", (gen,))
            for domain, mode in modes.items():
                cur = conn.execute("UPDATE script_domain_modes SET mode=?, generation_id=?, version=version+1, "
                                   "updated_at=?, updated_by='human:soak' WHERE domain=?", (mode, gen, now, domain))
                if cur.rowcount != 1:
                    raise SystemExit(f"daemon-soak: script_domain_modes has no {domain} row")
            conn.commit()
            conn.close()
        self.write_config()  # allowed_project_ids に案件を入れる
        self.start()
        ctl_mode = {"off": "off", "v8_load": "off", "shadow": "shadow", "enabled": "shadow"}[self.a.cond]
        if ctl_mode != "off":
            rev = self.api("GET", f"/api/v1/projects/{self.pid}/controller")["state"]["revision"]
            self.api("PUT", f"/api/v1/projects/{self.pid}/controller",
                     {"mode": ctl_mode, "expected_revision": rev, "reason": "soak"})
        self.op("setup", project=self.pid, script_modes=modes, controller=ctl_mode, port=self.port,
                pid=self.proc.pid, celeris=self.a.celeris)

    # ---- 負荷 ----
    def add_task(self, n):
        t = self.api("POST", "/api/v1/tasks", {
            "title": f"soak task {n}", "objective": "fake harness の計測用の task（P4-F）。",
            "acceptance": [{"type": "command", "cmd": "true", "expect_exit": 0}], "project_id": self.pid})
        return t

    def say(self, n):
        self.api("POST", f"/api/v1/chat/threads/{self.thread}/messages", {
            "client_message_id": f"soak-{n}", "text": f"soak の定期の確認 {n}", "attachment_ids": [],
            "reply_to_id": None, "mode": "queue", "resume_queue": False})

    def wake(self, n):
        self.api("POST", f"/api/v1/projects/{self.pid}/controller/wake",
                 {"external_ref": f"soak-{n}", "note": "soak", "idempotency_key": f"soak-{n}"})

    # ---- 標本 ----
    def db_counts(self):
        """DB の行数（読み取り専用の接続）。表が無ければ数えない（版の違う binary の before/after でも読める）。"""
        out = {}
        try:
            conn = sqlite3.connect(f"file:{self.db}?mode=ro", uri=True, timeout=10)
        except sqlite3.Error as e:
            return {"error": str(e)}
        try:
            out.update(count_rows(conn))
            out["dispatch_latency_ms"] = self.latencies(conn)
        except sqlite3.Error as e:
            out["error"] = str(e)
        finally:
            conn.close()
        return out

    def latencies(self, conn):
        """task の `created` の event から最初の `routing_decided` までの時間（ms。前の標本の後に決まった task の分）。"""
        rows = conn.execute(
            "SELECT c.ts, r.ts, r.rowid FROM events r JOIN events c ON c.task_id = r.task_id AND c.seq = 0 "
            "WHERE r.rowid > ? AND json_extract(r.json,'$.type')='routing_decided' "
            "AND NOT EXISTS (SELECT 1 FROM events p WHERE p.task_id = r.task_id AND p.seq < r.seq "
            "AND json_extract(p.json,'$.type')='routing_decided')", (self.latency_rowid,)).fetchall()
        lat = []
        for created, decided, rowid in rows:
            lat.append(round((ts_epoch(decided) - ts_epoch(created)) * 1000))
            self.latency_rowid = max(self.latency_rowid, rowid)
        return lat

    def sample(self, phase):
        pid = self.proc.pid if self.proc else None
        s = {"schema": SCHEMA, "label": self.a.label, "condition": self.a.cond, "phase": phase,
             "t": round(time.time() - self.t0, 1), "at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "pid": pid}
        if self.proc is not None and self.proc.poll() is not None:
            s["daemon_exit"] = self.proc.returncode
        try:
            s["proc"] = parse_proc_status(pathlib.Path(f"/proc/{pid}/status").read_text())
            s["arena_estimate"] = arena_estimate(pathlib.Path(f"/proc/{pid}/maps").read_text())
            s["fds"] = len(os.listdir(f"/proc/{pid}/fd"))
        except (OSError, TypeError):
            s["proc"] = {}
        pm, self.log_pos = last_process_memory(self.log, self.log_pos)
        if pm:
            self.pm = pm
        s["process_memory"] = self.pm
        try:
            d = self.api("GET", "/api/v1/daemon")
            snap = d.get("snapshot") or {}
            s["daemon"] = {"ticks": snap.get("ticks"), "in_flight": len(snap.get("in_flight") or []),
                           "script_tick_wait": snap.get("script_tick_wait"), "last_tick_at": snap.get("last_tick_at")}
            s["api_ok"] = True
        except (urllib.error.URLError, OSError, ValueError) as e:
            s["api_ok"] = False
            s["api_error"] = str(e)[:200]
        s["db"] = self.db_counts()
        s["db_bytes"] = sum(p.stat().st_size for p in self.home.glob("celeris.sqlite3*") if p.exists())
        s["disk_free_mb"] = shutil.disk_usage(self.home).free >> 20
        s["slow_ticks"] = self.slow_ticks()
        with open(self.out / "samples.jsonl", "a") as f:
            f.write(json.dumps(s, ensure_ascii=False) + "\n")
        return s

    def slow_ticks(self):
        try:
            return self.log.read_bytes().count(b"slow tick")
        except OSError:
            return None

    # ---- restart ----
    def kill_and_restart(self):
        # run が走っている間に止める（回収の対象が無い restart は回収を測らない。最大 120 秒待つ）。
        deadline = time.time() + 120
        before = self.db_counts()
        while not before.get("runs_running") and time.time() < deadline:
            time.sleep(0.2)
            before = self.db_counts()
        self.op("sigkill", runs_running=before.get("runs_running"), task_leases=before.get("task_leases"))
        self.stop(signal.SIGKILL)
        t = time.time()
        self.start()
        started = time.time() - t
        deadline = time.time() + 600
        while time.time() < deadline:
            c = self.db_counts()
            if not c.get("runs_running") and not c.get("task_leases"):
                break
            time.sleep(1)
        self.op("restart", start_secs=round(started, 2), reclaimed_secs=round(time.time() - t, 2),
                runs_running_after=c.get("runs_running"),
                task_leases_after=c.get("task_leases"), pid=self.proc.pid)

    # ---- 本体 ----
    def run(self):
        a = self.a
        limits = {"max_rss_mb": a.max_rss_mb, "min_free_mb": a.min_free_mb, "max_db_mb": a.max_db_mb,
                  "max_api_failures": 3}
        self.setup()
        self.t0 = time.time()
        self.op("start", secs=a.secs, limits=limits, task_interval=a.task_interval, chat_interval=a.chat_interval)
        self.sample("start")
        next_sample = next_task = time.time()
        next_chat = time.time() + a.chat_interval
        next_wake = time.time() + a.wake_interval
        n_task = n_chat = n_wake = 0
        failures = 0
        killed = a.kill_at <= 0
        reason = None
        # SIGTERM の既定の動作は process を即座に終わらせ、stop の記録と daemon の log の回収が残らない
        # （N2: 2026-11-06 の 24 時間 soak は別 task の pkill で 9.6 時間で切れ、log を失った）。
        signal.signal(signal.SIGTERM, raise_terminated)
        try:
            while True:
                now = time.time()
                for kind in ("task", "chat", "wake"):
                    due = {"task": next_task, "chat": next_chat, "wake": next_wake}[kind]
                    if now < due:
                        continue
                    # 失敗しても次の予定へ進める（失敗の連打で負荷を変えない）。
                    if kind == "task":
                        n_task += 1
                        next_task += a.task_interval
                        call = lambda: self.add_task(n_task)
                    elif kind == "chat":
                        n_chat += 1
                        next_chat += a.chat_interval
                        call = lambda: self.say(n_chat)
                    else:
                        n_wake += 1
                        next_wake += a.wake_interval
                        call = lambda: self.wake(n_wake)
                    try:
                        call()
                    except urllib.error.HTTPError as e:
                        self.op("load_error", kind=kind, status=e.code,
                                body=e.read().decode(errors="replace")[:300])
                    except (urllib.error.URLError, OSError, ValueError) as e:
                        self.op("load_error", kind=kind, error=str(e)[:200])
                if not killed and now - self.t0 >= a.kill_at:
                    killed = True
                    self.kill_and_restart()
                if now >= next_sample:
                    s = self.sample("cycle")
                    failures = 0 if s.get("api_ok") else failures + 1
                    reason = stop_reason(s, limits, now - self.t0, a.secs, failures)
                    if reason:
                        break
                    next_sample += a.sample_secs
                time.sleep(min(1.0, max(0.05, min(next_task, next_sample, next_chat) - time.time())))
        except KeyboardInterrupt:
            reason = "interrupted"
        except Terminated:
            reason = "terminated"
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        self.sample("end")
        self.op("stop", reason=reason, tasks=n_task, chats=n_chat, wakes=n_wake)
        self.stop()
        shutil.copy(self.log, self.out / "daemon.log")
        return reason


# ---------------------------------------------------------------- 集計

def summarize_run(samples, ops):
    rs = [s for s in samples if s.get("proc", {}).get("rss_kb")]
    if not rs:
        return None
    # 計測の窓は最初の process の標本だけ（restart の後の標本は別の process の値なので傾きに混ぜない）。
    after = [s for s in rs if s.get("pid") != rs[0].get("pid")]
    rs = [s for s in rs if s.get("pid") == rs[0].get("pid")]
    pts = [(s["t"] - rs[0]["t"], s["proc"]["rss_kb"]) for s in rs]
    end_t = pts[-1][0]
    at = {o: nearest(pts, o) for o in (0, 600, 1529) if end_t >= o - 60}

    def series(get):
        # 開始の値は値が 0 でない最初の標本（V8 は generation の復元の後、log の行は起動の直後に出る）。
        vals = [(s["t"], get(s)) for s in rs]
        vals = [(t, v) for t, v in vals if v is not None]
        nz = [x for x in vals if x[1]]
        vals = vals[vals.index(nz[0]):] if nz else vals
        return (vals[0][1], vals[-1][1], max(v for _, v in vals)) if vals else (None, None, None)

    def pmv(k):
        return lambda s: (s.get("process_memory") or {}).get(k)

    first, last = rs[0], rs[-1]
    anon_pts = [(s["t"] - rs[0]["t"], s["proc"].get("rss_anon_kb") or 0) for s in rs]

    def anon_at(a, b):
        # RssAnon の傾き（VmRSS と同じ経過秒・同じ式）。debug の binary の file の page は host の memory の
        # 具合で増減するので、本番（release）と比べるときは RssAnon を見る。
        return nearest(anon_pts, a), nearest(anon_pts, b)

    out = {
        "label": first["label"], "condition": first["condition"], "samples": len(rs), "wall_secs": end_t,
        "stop": next((o.get("reason") for o in ops if o.get("step") == "stop"), None),
        "rss_kb": series(lambda s: s["proc"].get("rss_kb")),
        "rss_anon_kb": series(lambda s: s["proc"].get("rss_anon_kb")),
        "rss_file_kb": series(lambda s: s["proc"].get("rss_file_kb")),
        "threads": series(lambda s: s["proc"].get("threads")),
        "arena_estimate": series(lambda s: s.get("arena_estimate")),
        "fds": series(lambda s: s.get("fds")),
        "sqlite_memory_used": series(pmv("sqlite_memory_used")),
        "sqlite_pagecache_bytes": series(pmv("sqlite_pagecache_bytes")),
        "malloc_arena": series(pmv("malloc_arena")),
        "malloc_mmap": series(pmv("malloc_mmap")),
        "malloc_in_use": series(pmv("malloc_in_use")),
        "malloc_free": series(pmv("malloc_free")),
        "v8_used_heap": series(pmv("v8_used_heap")),
        "v8_total_heap": series(pmv("v8_total_heap")),
        "v8_generations": series(pmv("v8_generations")),
        "rss_slope_whole_mb_per_hour": slope(pts[0], pts[-1]),
        "rss_slope_prod_whole_mb_per_hour": slope(at[0], at[1529]) if 1529 in at else None,
        "rss_slope_prod_steady_mb_per_hour": slope(at[600], at[1529]) if 1529 in at else None,
        "rss_anon_slope_prod_whole_mb_per_hour": slope(*anon_at(0, 1529)) if 1529 in at else None,
        "rss_anon_slope_prod_steady_mb_per_hour": slope(*anon_at(600, 1529)) if 1529 in at else None,
        "rss_anon_slope_after_600_mb_per_hour": slope(nearest(anon_pts, 600), anon_pts[-1]) if end_t > 900 else None,
        "rss_slope_after_600_mb_per_hour": slope(nearest(pts, 600), pts[-1]) if end_t > 900 else None,
        "db_start": first.get("db"), "db_end": last.get("db"),
        "slow_ticks": last.get("slow_ticks"),
        "script_tick_wait_end": (last.get("daemon") or {}).get("script_tick_wait"),
    }
    lat = [x for s in rs for x in (s.get("db") or {}).get("dispatch_latency_ms") or []]
    out["dispatch_latency_ms"] = {"n": len(lat), "p50": pct(lat, 50), "p95": pct(lat, 95), "max": max(lat or [0])}
    restart = next((o for o in ops if o.get("step") == "restart"), None)
    if restart:
        out["restart"] = {k: restart.get(k) for k in ("start_secs", "reclaimed_secs", "runs_running_after",
                                                      "task_leases_after")}
        out["restart"]["before"] = next(({"runs_running": o.get("runs_running"), "task_leases": o.get("task_leases")}
                                         for o in ops if o.get("step") == "sigkill"), None)
    out["samples_after_restart"] = len(after)
    # 残差: RssAnon の増分のうち、malloc の使用中（in_use）と空き（free。arena に残る断片）と V8 の total heap の増分で
    # 説明できない分（kB）。arena の bytes は madvise で返した page も含み RSS を超え得るので、内訳には使わない。
    d = lambda k: (out[k][1] - out[k][0]) if out[k][0] is not None and out[k][1] is not None else None
    anon = d("rss_anon_kb")
    parts = [d("malloc_in_use"), d("malloc_free"), d("malloc_mmap"), d("v8_total_heap")]
    if anon is not None and all(p is not None for p in parts[:3]):
        explained = sum(p for p in parts if p is not None) // 1024
        out["anon_delta_kb"] = anon
        out["anon_explained_kb"] = explained
        out["anon_residual_kb"] = anon - explained
    return out


def thresholds(runs):
    """合否の目安（P1-I の段 3 の目安を daemon 全体に広げたもの）。off の run と比べる。"""
    off = next((r for r in runs if r["condition"] == "off"), None)
    rows = []
    for r in runs:
        checks = {}
        # VmRSS でなく RssAnon で比べる（debug の binary の file の page は host の memory の具合で増減する。P4-F の実測）。
        s = r.get("rss_anon_slope_after_600_mb_per_hour")
        if off and s is not None and off.get("rss_anon_slope_after_600_mb_per_hour") is not None:
            checks["anon_steady_slope_vs_off_le_10mb_h"] = s - off["rss_anon_slope_after_600_mb_per_hour"] <= 10
        v = r["v8_used_heap"]
        if v[0]:
            checks["v8_used_heap_end_lt_2x_start"] = v[1] < 2 * v[0]
        g = r["v8_generations"]
        if g[2] is not None:
            checks["v8_generations_le_2"] = g[2] <= 2
        if r.get("restart"):
            checks["runs_and_leases_after_restart_0"] = (r["restart"].get("runs_running_after") == 0
                                                         and r["restart"].get("task_leases_after") == 0)
        checks["not_auto_stopped"] = r["stop"] in ("wall", "interrupted")
        rows.append({"label": r["label"], "condition": r["condition"], "checks": checks,
                     "pass": all(checks.values())})
    return rows


def load_dir(d):
    d = pathlib.Path(d)
    samples = [json.loads(x) for x in (d / "samples.jsonl").read_text().splitlines() if x.strip()]
    ops = [json.loads(x) for x in (d / "ops.jsonl").read_text().splitlines() if x.strip()]
    bad = [s for s in samples if s.get("schema") != SCHEMA]
    if bad:
        raise ValueError(f"{d}: schema mismatch")
    return samples, ops


def markdown(doc):
    lines = ["| label/cond | 窓の秒 | 停止 | RSS 開始→終了 kB | 傾き 0→1529 秒 MB/時 | 傾き 600→1529 秒 MB/時 | RssAnon 傾き 0→1529・600→1529 MB/時 | RssAnon Δ kB | "
             "malloc in_use Δ kB | malloc free Δ kB | SQLite 終了 kB | V8 used 最大 kB | threads | arena 推定 | "
             "残差 kB | dispatch p50/p95 ms |",
             "|---|---:|---|---|---:|---:|---|---:|---:|---:|---:|---:|---|---|---:|---|"]
    k = lambda v: None if v is None else v // 1024
    for r in doc["runs"]:
        def dd(key):
            a, b, _ = r[key]
            return None if a is None or b is None else b - a
        lines.append(
            f"| {r['label']}/{r['condition']} | {r['wall_secs']:.0f} | {r['stop']} | {r['rss_kb'][0]}→{r['rss_kb'][1]} | "
            f"{r['rss_slope_prod_whole_mb_per_hour']} | {r['rss_slope_prod_steady_mb_per_hour']} | "
            f"{r['rss_anon_slope_prod_whole_mb_per_hour']}・{r['rss_anon_slope_prod_steady_mb_per_hour']} | {dd('rss_anon_kb')} | "
            f"{k(dd('malloc_in_use'))} | {k(dd('malloc_free'))} | {k(r['sqlite_memory_used'][1])} | "
            f"{k(r['v8_used_heap'][2])} | {r['threads'][0]}→{r['threads'][1]} | {r['arena_estimate'][0]}→"
            f"{r['arena_estimate'][1]} | {r.get('anon_residual_kb')} | {r['dispatch_latency_ms']['p50']}/"
            f"{r['dispatch_latency_ms']['p95']} |")
    lines.append("")
    lines.append(f"本番の観測（2026-11-04、shadow）: {PROD_SAMPLES[0][1]}→{PROD_SAMPLES[-1][1]} kB／1,529 秒、"
                 f"傾き 全体 {slope(PROD_SAMPLES[0], PROD_SAMPLES[-1])}・定常 {slope(PROD_SAMPLES[1], PROD_SAMPLES[-1])} MB/時。")
    for t in doc["thresholds"]:
        lines.append(f"- {t['label']}/{t['condition']}: {'pass' if t['pass'] else 'FAIL'} {json.dumps(t['checks'])}")
    return "\n".join(lines) + "\n"


def cmd_summary(a):
    runs = []
    for d in a.dirs:
        samples, ops = load_dir(d)
        r = summarize_run(samples, ops)
        if r is None:
            print(f"daemon-soak: {d}: no samples with RSS", file=sys.stderr)
            return 2
        runs.append(r)
    doc = {"schema": SUMMARY_SCHEMA, "prod_samples": PROD_SAMPLES, "runs": runs, "thresholds": thresholds(runs)}
    pathlib.Path(a.json).write_text(json.dumps(doc, ensure_ascii=False, indent=1) + "\n")
    pathlib.Path(a.markdown).write_text(markdown(doc))
    print(markdown(doc))
    return 0


def cmd_plan(a):
    hours = a.hours
    plan = {
        "scope": "non-production",
        "conditions": CONDITIONS,
        "hours": hours,
        "processes": "4 個の celeris daemon（条件ごと、別の一時 home・127.0.0.1 の別 port）",
        "harness": "fake（soak_harness.py、LLM 0、課金 0）",
        "load": "task 10 秒に 1 件・chat 5 分に 1 通・controller の wake 1 時間に 1 回・tick 100 ms",
        "restart": "--kill-at で 1 回（SIGKILL→起動→running の run とリースが 0 に戻るまで）",
        "auto_stop": "壁時計・RSS 4096 MB・disk の空き 5 GB・DB 4096 MB・daemon の終了・API の 3 回連続の失敗",
        # P4-F の 26 分の実測: 1 daemon の DB（WAL を含む）が 74〜108 MB。比例の推定（WAL の checkpoint で小さくなり得る）。
        "disk_estimate_mb": round(hours * 3600 / 1560 * 108 * 4),
        "recommended_args": ["--max-db-mb", str(max(4096, round(hours * 3600 / 1560 * 108 * 2)))],
        "approval_template": {"approved_by": "human:<name>", "scope": "non-production",
                              "max_secs": int(hours * 3600), "conditions": CONDITIONS,
                              "expires_at": "<ISO 8601>", "host": "<host>", "note": "P4-F 段 3"},
        "not_covered": ["本番 daemon の読み取り（段 4）", "本番の off の再起動", "実 LLM"],
    }
    print(json.dumps(plan, ensure_ascii=False, indent=1))
    return 0


def cmd_run(a):
    approval = json.loads(pathlib.Path(a.approval).read_text()) if a.approval else None
    ok, why = gate(a.secs, approval, a.cond, os.environ)
    if not ok:
        print(f"daemon-soak: refused: {why}", file=sys.stderr)
        return 2
    if production_path(a.state) or production_path(a.out):
        print("daemon-soak: refused: --state/--out must not be under production paths", file=sys.stderr)
        return 2
    soak = Soak(a)
    soak.op("gate", ok=ok, reason=why, approval=approval)
    try:
        reason = soak.run()
    finally:
        soak.stop()
        shutil.rmtree(soak.secret_dir, ignore_errors=True)
    return 0 if reason in ("wall", "interrupted") else 1


def main(argv=None):
    p = argparse.ArgumentParser(prog="daemon_soak.py")
    sub = p.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run")
    r.add_argument("--cond", choices=CONDITIONS, required=True)
    r.add_argument("--label", default="main")
    r.add_argument("--out", required=True)
    r.add_argument("--state", required=True)
    r.add_argument("--celeris", required=True)
    r.add_argument("--script-deploy", required=True)
    r.add_argument("--bundle", required=True)
    r.add_argument("--secs", type=int, required=True)
    r.add_argument("--approval")
    r.add_argument("--sample-secs", type=int, default=60)
    r.add_argument("--task-interval", type=float, default=10.0)
    r.add_argument("--chat-interval", type=float, default=300.0)
    r.add_argument("--wake-interval", type=float, default=3600.0)
    r.add_argument("--tick-ms", type=int, default=100)
    r.add_argument("--kill-at", type=int, default=0)
    r.add_argument("--harness-sleep", type=float, default=3.0)
    r.add_argument("--max-rss-mb", type=int, default=4096)
    r.add_argument("--min-free-mb", type=int, default=5120)
    r.add_argument("--max-db-mb", type=int, default=4096)
    s = sub.add_parser("summary")
    s.add_argument("dirs", nargs="+")
    s.add_argument("--json", required=True)
    s.add_argument("--markdown", required=True)
    pl = sub.add_parser("plan")
    pl.add_argument("--hours", type=float, default=24)
    a = p.parse_args(argv)
    return {"run": cmd_run, "summary": cmd_summary, "plan": cmd_plan}[a.cmd](a)


if __name__ == "__main__":
    sys.exit(main())
