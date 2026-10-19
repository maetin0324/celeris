"""実 `celerisctl campaign export` 形式の evidence dir を組む（試験用）。形は decision_log_export.rs に合わせる。"""
import copy
import hashlib
import json
import os

CAMPAIGN = "jev-b1c1-side-1"
MHASH = "a" * 64
ROOT, CHILD, GRAND = "01M5A0000000000000000RT001", "01M5A0000000000000000CH001", "01M5A0000000000000000GR001"
BROOT = "01M5A0000000000000000BR001"
CANDS = [
    {"source_id": "anthropic", "model_id": "claude-sonnet-5-5", "harness_id": "claude_code"},
    {"source_id": "anthropic", "model_id": "claude-opus-5-5", "harness_id": "claude_code"},
]
RESOLVED = {"mode": "decision_assisted", "source": "task", "lower_policy": "route_defaults",
            "evidence_mode": "none", "online_update": False}
RESOLVED_B = dict(RESOLVED, mode="fixed")


def _event(arm, jev_ok=True, resolved=None, overridden_by=None, real_api_verified=False):
    rec = {"features": {"decision": {"routing_resolved": resolved or (RESOLVED if arm == "C" else RESOLVED_B)}}}
    if arm == "C":
        rec["lane_assist"] = {
            "mode": "decision_assisted",
            "classifier_status": {"status": "jev_ok"} if jev_ok else {"status": "fell_back_to_fixed", "reason": "client_error"},
            "real_api_verified": real_api_verified,
            "base_lane": "standard", "proposed_lane": "frontier" if jev_ok else None,
            "final_lane": "frontier" if jev_ok else "standard",
            "basis": "classification_raise" if jev_ok else "fixed_fallback",
        }
        if overridden_by:
            rec["lane_assist"]["overridden_by"] = overridden_by
    return {"type": "routing_decided", "record": rec}


def trial(run, task, arm, retry=False, version="claude-haiku-5-5-20261001", cands=CANDS, exec_model="claude-opus-5-5", campaign=CAMPAIGN):
    return {
        "record_type": "trial", "schema": "celeris.decision-log/1", "dedup_key": f"trial:{run}",
        "campaign_id": campaign, "run_id": run, "task_id": task, "arm": arm, "case_id": "p1", "repetition": 0,
        "manifest_hash": MHASH, "routing_mode": "decision_assisted" if arm == "C" else "fixed",
        "is_retry": retry, "proposed_candidates": cands,
        "proposed_first": cands[0] if cands else None, "exec_model": exec_model,
        "fallback_reason": None, "classifier_snapshot": version if arm == "C" else None,
        "run_end": "completed", "grader_verdict": "pass",
    }


def usage(run, task, role, cost, retry=False, campaign=CAMPAIGN):
    return {
        "record_type": "usage", "schema": "celeris.decision-log/1", "dedup_key": f"run:{run}:{role}",
        "campaign_id": campaign, "run_id": run, "task_id": task, "role": role, "is_retry": retry,
        "shared_overhead": False, "list_cost_usd": cost, "cash_cost_usd": cost,
    }


def run_rec(run, task, arm, parent=None, retry=False):
    return {"record_type": "run", "schema": "celeris.decision-log/1", "dedup_key": f"run:{run}",
            "campaign_id": CAMPAIGN, "run_id": run, "task_id": task, "arm": arm,
            "parent_run_id": parent, "is_retry": retry}


def stat(vals):
    v = [x for x in vals if x is not None]
    return {"sum": sum(v) if v else None, "n_observed": len(v), "n_missing": len(vals) - len(v), "n_estimated": 0}


def build_summary(records):
    us = [r for r in records if r["record_type"] == "usage"]
    by_role = {}
    for role in ["cos", "planner", "worker", "reviewer", "jev"]:
        rows = [u for u in us if u["role"] == role]
        by_role[role] = {"n_rows": len(rows), "list_cost_usd": stat([u["list_cost_usd"] for u in rows]),
                         "cash_cost_usd": stat([u["cash_cost_usd"] for u in rows])}
    rrows = [u for u in us if u["is_retry"]]
    return {"schema": "celeris.decision-log/1", "campaign_id": CAMPAIGN, "manifest_hash": MHASH,
            "n_runs": sum(1 for r in records if r["record_type"] == "trial"), "by_role": by_role,
            "by_retry": {"retry": {"n_rows": len(rrows), "list_cost_usd": stat([u["list_cost_usd"] for u in rrows])}},
            "incomplete": ["quota"]}


def default_case():
    """root(C) -> 子 -> 孫 と、孫の retry、B1 の root。全て live 形。"""
    rows, events = [], []
    plan = [("r1", ROOT, "C", None, False), ("r2", CHILD, "C", "r1", False),
            ("r3", GRAND, "C", "r2", False), ("r4", GRAND, "C", "r3", True), ("rb", BROOT, "B", None, False)]
    for run, task, arm, parent, retry in plan:
        rows.append(trial(run, task, arm, retry))
        rows.append(run_rec(run, task, arm, parent, retry))
        rows.append(usage(run, task, "worker", 0.5, retry))
        rows.append(usage(run, task, "reviewer", 0.1, retry))
        if arm == "C":
            rows.append(usage(run, task, "jev", 0.01, retry))
        events.append({"task_id": task, "run_id": run, "event": _event(arm)})
    tasks = {ROOT: {"parent": None}, CHILD: {"parent": ROOT}, GRAND: {"parent": CHILD}, BROOT: {"parent": None}}
    return {"records": rows, "events": events, "tasks": tasks}


def write(case, directory, summary=None, prov_over=None, summary_over=None):
    os.makedirs(directory, exist_ok=True)
    rec_text = "".join(json.dumps(r, ensure_ascii=False) + "\n" for r in case["records"])
    s = summary or build_summary(case["records"])
    s = dict(s, **(summary_over or {}))
    sum_text = json.dumps(s, indent=2) + "\n"
    put(directory, "records.jsonl", rec_text)
    put(directory, "summary.json", sum_text)
    put(directory, "routing_events.jsonl", "".join(json.dumps(e) + "\n" for e in case["events"]))
    prov = {"source": "celerisctl campaign export", "campaign_id": CAMPAIGN, "manifest_hash": MHASH,
            "generated_at": "2026-10-19T12:00:00Z",
            "export_command": f"celerisctl campaign export {CAMPAIGN} --out evidence --manifest-hash {MHASH}",
            "files_sha256": {"records.jsonl": hashlib.sha256(rec_text.encode()).hexdigest(),
                             "summary.json": hashlib.sha256(sum_text.encode()).hexdigest()},
            "tasks": case["tasks"]}
    prov.update(prov_over or {})
    put(directory, "provenance.json", json.dumps(prov, indent=2))
    return directory


def put(directory, name, text):
    with open(os.path.join(directory, name), "w", encoding="utf-8") as f:
        f.write(text)


def mutate(case):
    return copy.deepcopy(case)


def set_event(case, run, **kw):
    for e in case["events"]:
        if e["run_id"] == run:
            e["event"] = _event(kw.pop("arm", "C"), **kw)
