"""試験用の test double: 合成の「測定 daemon DB」と、それを export した形の evidence dir を組む。

- 読取元 DB は crates/task-core/migrations/*.sql をそのまま流した一時 SQLite（実 schema の形）。中身は合成で、
  実 daemon の原記録ではない。これで通る正例は「変換・照合アルゴリズムの正例」であり、実 daemon の証明ではない。
- evidence の records.jsonl / summary.json は jev_source_reconcile の投影（Rust の build_campaign_export の
  移植）で作る。celerisctl が書いた実物ではない（実物との一致は手で確かめる。progress の記録を参照）。
- Jev 消費行の run_id: 本番の dispatcher（lane_assist.rs::record_jev_usage）は run_id を書かない。正例では
  run_id を入れた「run に結べる原記録」を合成し、本番の形（run_id なし）は負例で live 不可になることを見る。
"""
import copy
import glob
import importlib.util
import json
import os
import sqlite3

HERE = os.path.dirname(os.path.abspath(__file__))
DEV = os.path.normpath(os.path.join(HERE, "..", ".."))
MIGRATIONS = os.path.normpath(os.path.join(DEV, "..", "..", "crates", "task-core", "migrations"))


def _mod(name, file):
    spec = importlib.util.spec_from_file_location(name, os.path.join(DEV, file))
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


reader = _mod("jev_source_reader", "jev_source_reader.py")
rec = _mod("jev_source_reconcile", "jev_source_reconcile.py")

CAMPAIGN = "jev-b1c1-side-1"
MHASH = "a" * 64
TS = "2026-10-19T12:00:00Z"
ROOT, CHILD, GRAND = "01M5A0000000000000000RT001", "01M5A0000000000000000CH001", "01M5A0000000000000000GR001"
BROOT = "01M5A0000000000000000BR001"
RUNS = {"r1": "01M5A00000000000000RUN0001", "r2": "01M5A00000000000000RUN0002", "r3": "01M5A00000000000000RUN0003",
        "r4": "01M5A00000000000000RUN0004", "rb": "01M5A00000000000000RUN00B1"}
CANDS = [
    {"source_id": "anthropic", "model_id": "claude-sonnet-5-5", "harness_id": "claude_code"},
    {"source_id": "anthropic", "model_id": "claude-opus-5-5", "harness_id": "claude_code"},
]
RESOLVED = {"mode": "decision_assisted", "source": "task", "lower_policy": "route_defaults",
            "evidence_mode": "none", "online_update": False}
RESOLVED_B = dict(RESOLVED, mode="fixed")
VERSION = "jev-classifier-2026-10-01"
# (key, task, arm, parent run key, attempt)
PLAN = [("r1", ROOT, "C", None, 1), ("r2", CHILD, "C", None, 1), ("r3", GRAND, "C", None, 1),
        ("r4", GRAND, "C", "r3", 2), ("rb", BROOT, "B", None, 1)]

_TEMPLATE = None


def _template():
    global _TEMPLATE
    if _TEMPLATE is None:
        m = sqlite3.connect(":memory:")
        for f in sorted(glob.glob(os.path.join(MIGRATIONS, "*.sql"))):
            with open(f, encoding="utf-8") as fh:
                m.executescript(fh.read())
        m.execute("CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)")
        m.execute("INSERT INTO schema_migrations VALUES (86, ?)", (TS,))
        m.commit()
        _TEMPLATE = m
    return _TEMPLATE


def event_body(run, arm, jev_ok=True, resolved=None, overridden_by=None, real_api_verified=False, la=True):
    """routing_decided の event json（lane_assist・routing_resolved は record.features.decision の下）。"""
    dec = {"decision_id": f"dec-{run}", "routing_resolved": resolved or (RESOLVED if arm == "C" else RESOLVED_B)}
    if arm == "C" and la:
        dec["lane_assist"] = {
            "mode": "decision_assisted",
            "classifier_status": {"status": "jev_ok"} if jev_ok else {"status": "fell_back_to_fixed", "reason": "client_error"},
            "real_api_verified": real_api_verified,
            "base_lane": "standard", "final_lane": "frontier" if jev_ok else "standard",
            "basis": "classification_raise" if jev_ok else "fixed_fallback",
        }
        if jev_ok:
            dec["lane_assist"]["proposed_lane"] = "frontier"
        if overridden_by:
            dec["lane_assist"]["overridden_by"] = overridden_by
    return {"type": "routing_decided", "run_id": run,
            "record": {"features": {"schema": "celeris.task-features/1", "template": "t", "hash": "f", "decision": dec},
                       "decision": {"lane": "frontier"}}}


class Source:
    """合成の読取元 DB（test double）。"""

    def __init__(self, path):
        self.path = path
        self.c = sqlite3.connect(path)
        _template().backup(self.c)
        self.seq = {}

    def task(self, tid, parent=None):
        self.c.execute("INSERT INTO tasks (id, status, kind, parent_id, priority, created_at, json, title, updated_at) "
                       "VALUES (?, 'done', 'task', ?, 0, ?, ?, ?, ?)", (tid, parent, TS, json.dumps({"id": tid}), tid, TS))

    def campaign_root(self, tid, arm, campaign=CAMPAIGN, mhash=MHASH):
        self.c.execute("INSERT INTO task_campaigns (task_id, campaign_id, case_id, arm, repetition, manifest_hash, "
                       "routing_setting, created_at) VALUES (?, ?, 'p1', ?, 0, ?, ?, ?)",
                       (tid, campaign, arm, mhash, "decision_assisted" if arm == "C" else "fixed", TS))

    def event(self, tid, body):
        n = self.seq.get(tid, 0) + 1
        self.seq[tid] = n
        self.c.execute("INSERT INTO events (task_id, seq, ts, json) VALUES (?, ?, ?, ?)", (tid, n, TS, json.dumps(body)))

    def run(self, rid, tid, arm, parent=None, attempt=1, version=VERSION, cands=CANDS, exec_model="claude-opus-5-5",
            mhash=MHASH, campaign=CAMPAIGN, classifier=True):
        self.c.execute("INSERT INTO runs (run_id, task_id, role, seq, status, model, started_at) "
                       "VALUES (?, ?, 'worker', ?, 'completed', ?, ?)", (rid, tid, attempt, exec_model, TS))
        first = cands[0] if cands else {}
        self.c.execute(
            "INSERT INTO decision_log_runs (run_id, decision_id, campaign_id, arm, task_id, parent_run_id, attempt_no, "
            "routing_mode, classifier_kind, classifier_snapshot, proposed_candidates_json, proposed_first_source_id, "
            "proposed_first_model, proposed_first_harness, exec_source_id, exec_model, run_end, wu_verdict, "
            "campaign_verdict, wall_ms, case_id, repetition, manifest_hash, routing_setting, updated_at) "
            "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'anthropic', ?, 'completed', 'passed', 'pass', 1000, 'p1', 0, ?, ?, ?)",
            (rid, f"dec-{rid}", campaign, arm, tid, parent, attempt,
             "decision_assisted" if arm == "C" else "fixed",
             "jev" if (arm == "C" and classifier) else None, version if arm == "C" else None,
             json.dumps(cands) if cands else None, first.get("source_id"), first.get("model_id"),
             first.get("harness_id"), exec_model, mhash, "decision_assisted" if arm == "C" else "fixed", TS))

    def usage(self, key, role, cost, run=None, task=None, retry=False, campaign=CAMPAIGN, decision=None, shared=False,
              rule=None):
        self.c.execute("INSERT INTO decision_log_usage (dedup_key, run_id, decision_id, campaign_id, task_id, role, "
                       "is_retry, input_tokens, list_cost_usd, cash_cost_usd, wall_ms, attribution_rule, shared_overhead, "
                       "updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, 100, ?, NULL, 500, ?, ?, ?)",
                       (key, run, decision, campaign, task, role, int(retry), cost, rule, int(shared), TS))

    def jev(self, run_key, tid, ok=True, cost=0.01, bound=True, call_no=1):
        rid = RUNS.get(run_key, run_key)
        self.usage(f"decision:jev:{tid}:{'ok' if ok else 'fallback:client_error'}:call-{rid}-{call_no}:{call_no}", "jev",
                   cost, run=rid if bound else None, task=tid, retry=call_no > 1,
                   decision=f"jev:{tid}:{'ok' if ok else 'fallback:client_error'}:call-{rid}-{call_no}",
                   rule="jev_classification_subject")

    def commit(self):
        self.c.commit()

    def close(self):
        self.c.commit()
        self.c.close()


def default_source(path, jev_bound=True):
    """root(C) -> 子 -> 孫 と、孫の retry、B1 の root。cos の共有オーバーヘッド 1 行。"""
    s = Source(path)
    for tid, parent in [(ROOT, None), (CHILD, ROOT), (GRAND, CHILD), (BROOT, None)]:
        s.task(tid, parent)
    s.campaign_root(ROOT, "C")
    s.campaign_root(BROOT, "B")
    for key, tid, arm, parent, attempt in PLAN:
        rid = RUNS[key]
        s.run(rid, tid, arm, RUNS.get(parent), attempt)
        s.event(tid, event_body(rid, arm))
        s.usage(f"run:{rid}:worker", "worker", 0.5, run=rid, task=tid, retry=parent is not None)
        s.usage(f"run:{rid}:reviewer", "reviewer", 0.1, run=rid, task=tid, retry=parent is not None)
        if arm == "C":
            s.jev(key, tid, bound=jev_bound)
    s.usage("decision:cos:sess-1:1", "cos", 0.2, decision="cos:sess-1", shared=True, rule="cos_turn_shared_overhead")
    s.usage(f"run:{RUNS['r1']}:planner", "planner", 0.3, run=RUNS["r1"], task=ROOT)
    s.commit()
    return s


def project(db):
    return rec.project(reader.read_campaign(CAMPAIGN, environ={reader.ENV_SOURCE: db}))


def export_of(db):
    """読取元 → export の形（records/summary/tasks/events）。test double（celerisctl の実物ではない）。"""
    p = project(db)
    return {"records": copy.deepcopy(p["records"]), "summary": copy.deepcopy(p["summary"]),
            "tasks": copy.deepcopy(p["tasks"]), "events": copy.deepcopy(p["routing_events"])}


def write(ex, directory, prov_over=None, raw_records=None):
    os.makedirs(directory, exist_ok=True)
    text = raw_records if raw_records is not None else "".join(
        json.dumps(r, ensure_ascii=False, separators=(",", ":")) + "\n" for r in ex["records"])
    put(directory, "records.jsonl", text)
    put(directory, "summary.json", json.dumps(ex["summary"], indent=2, ensure_ascii=False) + "\n")
    put(directory, "routing_events.jsonl", "".join(json.dumps(e, ensure_ascii=False) + "\n" for e in ex["events"]))
    prov = {"source": "celerisctl campaign export", "campaign_id": CAMPAIGN, "manifest_hash": MHASH,
            "generated_at": TS, "export_command": f"celerisctl campaign export {CAMPAIGN} --out evidence --manifest-hash {MHASH}",
            "tasks": ex["tasks"]}
    prov.update(prov_over or {})
    put(directory, "provenance.json", json.dumps(prov, indent=2))
    return directory


def put(directory, name, text):
    with open(os.path.join(directory, name), "w", encoding="utf-8") as f:
        f.write(text)
