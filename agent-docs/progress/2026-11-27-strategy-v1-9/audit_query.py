"""戦略層 v1 子 9 WU audit: 本番 DB を読み取り専用で監査し audit.json を書く。

使い方（repo の root で）:
    python3 -I agent-docs/progress/2026-11-27-strategy-v1-9/audit_query.py \
        --out agent-docs/progress/2026-11-27-strategy-v1-9/audit.json

DB は URI `file:...?mode=ro` で開く（書き込まない）。昇格の記録は DB に無いので、本番 host の
`$CELERIS_STATE_DIR/logs/promote-*.log` と `$CELERIS_STATE_DIR/releases/*/audit/*.json` を読むだけにする。
行の本文（chat の text・決定の note・mcp の request）は出力に写さない。件数・id・時刻・actor と、
note の書き出しから決めた分類（human_via_operator など）だけを出す。
"""

import argparse
import collections
import hashlib
import json
import os
import re
import sqlite3
from datetime import datetime, timezone
from pathlib import Path

DB_URI = "file:/local/celeris/data/db/celeris.sqlite3?mode=ro"
STATE_DIR = Path(os.environ.get("CELERIS_STATE_DIR", "/local/celeris/state"))
PROJECT = "01M2WTS3DKNZBSZ2JMVB4CZMBW"
EXPECTED_REQUEST_SHA256 = "d7b584adb2c5992856f6b6a721c9ec1598b66b3d7ab1fa3610c4feeab871a699"
PARENT = "01M7XQ9PS0AFKKMRJ7P5ZMKQQE"
COS_THREAD = "01M5JJTVPW15CKFRCA0FAYCHK4"
# 子 0〜8 は各進捗の front matter の tasks、子 9 はこの task。
CHILDREN = [
    "01M7XQ9PZQ3DQZMYKNFNPTPBQJ",
    "01M7XQ9Q4P4ZQ38Y509J9XYAQ0",
    "01M7XQ9Q9EB3CH59TSNYM9HBC8",
    "01M7XQ9QE8STND208NXG90F743",
    "01M7XQ9QK1KC34Q2A6PJ960TJB",
    "01M7XQ9QQXY1DY56EXE0TT8X6W",
    "01M7XQ9QWK58ESMW42Q1TGT3WZ",
    "01M7XQ9R1EHA8GQ0TZVRM4FBVX",
    "01M7XQA74FP70G3T3KSTP02S0W",
    "01M7XQA79910VEZ66HWTX4HZVT",
]
FIRST_GATE_CHILD = 1
SECOND_GATE_CHILD = 6
ROADMAP_SINCE = "2026-11-21"
PROMOTION_SINCE = "2026-11-20"
ACTOR_SINCE = "2026-11-21"
NON_HUMAN_PROMOTERS = ("worker", "cos", "cos_project", "cos_general", "mcp", "mcp_client")


def kind_of(actor):
    if not actor:
        return None
    return actor.split(":", 1)[0]


def note_class(note):
    """決定・gate の note の書き出しで、誰の選択かを分類する（本文は写さない）。"""
    text = (note or "").lstrip()
    if re.match(r"^人（[^）]+）の(判断|決定|承認)", text):
        return "human_via_operator"
    if text.startswith("回答者: operator") and "決めたのは人" in text[:120]:
        return "human_via_operator"
    if text.startswith("operator"):
        return "operator_own"
    if text.startswith("CoS"):
        return "cos_own"
    if not text:
        return "no_note"
    return "other"


def descendants(c, tid):
    out, queue = [], [tid]
    while queue:
        t = queue.pop()
        for (cid,) in c.execute("select id from tasks where parent_id=? order by created_at", (t,)):
            out.append(cid)
            queue.append(cid)
    return out


def events_of(c, tid):
    for seq, ts, raw in c.execute("select seq, ts, json from events where task_id=? order by seq", (tid,)):
        yield seq, ts, json.loads(raw)


def runs_started_between(c, unit_ids, start, end):
    n = 0
    for uid in unit_ids:
        n += c.execute(
            "select count(*) from runs where work_unit_id=? and started_at>=? and started_at<?",
            (uid, start, end),
        ).fetchone()[0]
    return n


def decision_rows(c, child_idx, tree):
    out = []
    marks = ",".join("?" * len(tree))
    rows = c.execute(
        f"select id, task_id, key, kind, status, needed_before_json, created_at, answered_at, json "
        f"from decisions where task_id in ({marks}) order by created_at",
        tree,
    ).fetchall()
    for did, tid, key, kind, status, nb_raw, created, answered, raw in rows:
        body = json.loads(raw)
        answer = body.get("answer") or {}
        by = answer.get("by")
        needed = json.loads(nb_raw)
        late_runs = 0
        targets = []
        for target in needed:
            if target == "self":
                n = c.execute(
                    "select count(*) from runs where task_id=? and started_at>=? and started_at<?",
                    (tid, created, answered or "9999"),
                ).fetchone()[0]
                targets.append({"target": "self", "runs_started_while_open": n})
                late_runs += n
                continue
            if target.startswith("stage:"):
                units = [u for (u,) in c.execute(
                    "select id from work_units where task_id=? and phase=?", (tid, target[6:]))]
            else:
                units = [u for (u,) in c.execute(
                    "select id from work_units where task_id=? and key=?", (tid, target))]
            n = runs_started_between(c, units, created, answered or "9999")
            targets.append({"target": target, "units": len(units), "runs_started_while_open": n})
            late_runs += n
        raised = body.get("raised_by") or {}
        out.append({
            "child": child_idx,
            "task_id": tid,
            "decision_id": did,
            "key": key,
            "kind": kind,
            "status": status,
            "option": answer.get("option"),
            "recorded_actor": by,
            "recorded_actor_kind": kind_of(by),
            "chosen_by_per_note": note_class(answer.get("note")),
            "raised_by": {"origin": raised.get("origin"), "run_id": raised.get("run_id")},
            "created_at": created,
            "answered_at": answered,
            "needed_before": needed,
            "needed_before_targets": targets,
            "answered_before_needed": answered is not None and late_runs == 0,
        })
    return out


def gate_events(c, child_idx, tree):
    """plan の gate と blocked の問いへの回答（ADR-0079 D8・status の問い）を時刻・actor・分類で出す。"""
    out = []
    for tid in tree:
        for seq, ts, e in events_of(c, tid):
            ty = e.get("type")
            if ty == "plan_gate_decided":
                out.append({"child": child_idx, "task_id": tid, "seq": seq, "ts": ts, "type": ty,
                            "action": e.get("action"), "recorded_actor": e.get("by"),
                            "recorded_actor_kind": kind_of(e.get("by")),
                            "chosen_by_per_note": note_class(e.get("note"))})
            elif ty == "answered" and not str(e.get("question", "")).startswith("計画の承認"):
                out.append({"child": child_idx, "task_id": tid, "seq": seq, "ts": ts, "type": ty,
                            "recorded_actor": e.get("by"), "recorded_actor_kind": kind_of(e.get("by")),
                            "chosen_by_per_note": note_class(e.get("answer"))})
            elif ty == "edited":
                out.append({"child": child_idx, "task_id": tid, "seq": seq, "ts": ts, "type": ty,
                            "fields": e.get("fields"), "recorded_actor": e.get("by"),
                            "recorded_actor_kind": kind_of(e.get("by"))})
    return out


def done_and_review(c, tid):
    done_at, last_pass = None, None
    for _seq, ts, e in events_of(c, tid):
        if e.get("type") == "transitioned" and e.get("to") == "done":
            done_at = ts
        if e.get("type") == "review_verdict":
            last_pass = {"ts": ts, "pass": e.get("pass"), "run_id": e.get("run_id")}
    return done_at, last_pass


def promotions():
    logs = STATE_DIR / "logs"
    audits = {}
    for path in sorted((STATE_DIR / "releases").glob("*/audit/*.json")):
        d = json.loads(path.read_text())
        if d.get("operation") in ("promote", "rollback"):
            audits[d.get("to")] = {"actor": d.get("actor"), "actor_kind": d.get("actor_kind"),
                                   "status": d.get("status"), "from": d.get("from"),
                                   "started_at": d.get("started_at"),
                                   "task_refs": len(d.get("task_refs") or []),
                                   "decision_refs": len(d.get("decision_refs") or [])}
    # sd_release_record が promoted.json の写しを足す台帳。release の dir が刈られても残る。
    records = {}
    ledger = logs / "release-records.jsonl"
    if ledger.exists():
        for line in ledger.read_text(errors="replace").splitlines():
            try:
                d = json.loads(line)
            except ValueError:
                continue
            if d.get("kind") == "promoted" and d.get("sha12"):
                records[d["sha12"]] = {"actor": d.get("actor"), "promoted_at": d.get("promoted_at"),
                                       "from": d.get("from"), "status": d.get("status"),
                                       "has_policy_ref": bool(d.get("policy_ref")),
                                       "included_tasks": len(d.get("included_tasks") or [])}
    out = []
    for path in sorted(logs.glob("*.log")):
        m = re.match(r"^(promote|rollback)-(\d{8})-(\d{6})\.log$", path.name)
        if not m or m.group(2) < PROMOTION_SINCE.replace("-", ""):
            continue
        text = path.read_text(errors="replace")
        sha = re.search(r"\[(?:promote|rollback)\] (?:promote|rollback) ([0-9a-f]{12})", text)
        sha12 = sha.group(1) if sha else None
        lines = [ln for ln in text.splitlines() if ln.strip()]
        last_ts = lines[-1].split(" ", 1)[0] if lines else None
        audit = audits.get(sha12)
        record = records.get(sha12)
        actor = (record or {}).get("actor") or (audit or {}).get("actor")
        sources = [n for n, v in (("logs/release-records.jsonl", record), ("releases/<sha>/audit/*.json", audit)) if v]
        out.append({"operation": m.group(1), "log": path.name, "release": sha12,
                    "last_log_line_at": last_ts,
                    "record": record,
                    "audit": audit,
                    "actor": actor,
                    "actor_kind": kind_of(actor),
                    "actor_source": sources or
                    "unconfirmed: no promoted record in release-records.jsonl, release dir pruned, log has no actor"})
    return out


def actors(c, tree_ids):
    threads = dict(c.execute("select id, kind from chat_threads"))
    msgs = collections.Counter()
    human_recorded_external = 0
    for tid, role, meta_raw in c.execute(
            "select thread_id, role, metadata_json from chat_messages where created_at>=?", (ACTOR_SINCE,)):
        meta = json.loads(meta_raw)
        author = meta.get("author")
        relay = bool(meta.get("relay"))
        mcp = bool(meta.get("mcp_receipt"))
        akind = kind_of(author)
        if author is None:
            akind = "none(daemon/cron/legacy)" if role == "user" else "none(" + (meta.get("posted_by") or "run") + ")"
        msgs[(threads.get(tid, "?"), "cos_project_thread" if tid == COS_THREAD else "other", role, akind,
              "relay" if relay else "-", "mcp_receipt" if mcp else "-")] += 1
        if role == "user" and (mcp or relay) and akind == "human":
            human_recorded_external += 1
    chat_events = collections.Counter(t for (t,) in c.execute(
        "select type from chat_events where created_at>=?", (ACTOR_SINCE,)))
    clients = {cid: name for cid, name in c.execute("select id, name from mcp_clients")}
    mcp_calls = collections.Counter()
    for cid, tool, ok in c.execute("select client_id, tool, ok from mcp_calls where at>=?", (ACTOR_SINCE,)):
        mcp_calls[("mcp:" + clients.get(cid, "?"), tool, "ok" if ok else "error")] += 1
    mcp_scopes = {name: scopes for name, scopes in c.execute(
        "select name, scopes from mcp_clients where id in (select distinct client_id from mcp_calls where at>=?)",
        (ACTOR_SINCE,))}
    task_actor = collections.Counter()
    for tid in tree_ids:
        for _seq, _ts, e in events_of(c, tid):
            for field in ("by", "actor", "created_by"):
                v = e.get(field)
                if isinstance(v, str):
                    task_actor[(e.get("type"), field, kind_of(v))] += 1
    decided_by_mcp = sum(
        1 for (raw,) in c.execute("select json from decisions where answered_at>=?", (ACTOR_SINCE,))
        if kind_of((json.loads(raw).get("answer") or {}).get("by")) == "mcp")
    return {
        "since": ACTOR_SINCE + "T00:00:00Z",
        "chat_messages_by_actor": [
            {"thread_kind": k[0], "thread": k[1], "role": k[2], "author_kind": k[3], "relay": k[4],
             "mcp": k[5], "count": v} for k, v in sorted(msgs.items(), key=lambda x: str(x[0]))],
        "chat_events_by_type": dict(chat_events.most_common()),
        "chat_events_note": "chat_events.payload_json has no actor field; the actor of a chat event is its message's",
        "mcp_calls_by_client_tool": [
            {"actor": k[0], "tool": k[1], "result": k[2], "count": v}
            for k, v in sorted(mcp_calls.items())],
        "mcp_client_scopes": mcp_scopes,
        "tree_task_events_by_actor_kind": [
            {"event": k[0], "field": k[1], "actor_kind": k[2], "count": v}
            for k, v in sorted(task_actor.items(), key=lambda x: str(x[0]))],
        "external_llm_or_mcp_recorded_as_human": {
            "chat_user_messages_with_mcp_or_relay_and_human_author": human_recorded_external,
            "decisions_answered_by_mcp_since": decided_by_mcp,
        },
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    c = sqlite3.connect(DB_URI, uri=True)
    now = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")

    request, proj_updated = c.execute(
        "select request, updated_at from projects where id=?", (PROJECT,)).fetchone()
    request_sha = hashlib.sha256(request.encode("utf-8")).hexdigest()

    db_children = [r[0] for r in c.execute(
        "select id from tasks where parent_id=? order by created_at", (PARENT,))]
    scope = {"parent": PARENT, "children": {}, "children_match_db": db_children == CHILDREN}
    all_ids = [PARENT]
    decisions, gates = [], []
    for idx, child in enumerate(CHILDREN):
        tree = [child] + descendants(c, child)
        all_ids.extend(tree)
        status = c.execute("select status from tasks where id=?", (child,)).fetchone()[0]
        marks = ",".join("?" * len(tree))
        wus = c.execute(f"select count(*) from work_units where task_id in ({marks})", tree).fetchone()[0]
        runs = collections.Counter(r for (r,) in c.execute(
            f"select role from runs where task_id in ({marks})", tree))
        evs = c.execute(f"select count(*) from events where task_id in ({marks})", tree).fetchone()[0]
        done_at, last_review = done_and_review(c, child)
        scope["children"][str(idx)] = {
            "task_id": child, "status": status, "descendants": tree[1:], "work_units": wus,
            "runs_by_role": dict(runs), "events": evs, "done_at": done_at, "last_review_verdict": last_review}
        decisions.extend(decision_rows(c, idx, tree))
        gates.extend(gate_events(c, idx, tree))
    parent_status = c.execute("select status from tasks where id=?", (PARENT,)).fetchone()[0]
    scope["parent_status"] = parent_status
    scope["parent_decisions"] = c.execute(
        "select count(*) from decisions where task_id=?", (PARENT,)).fetchone()[0]
    scope["tasks_total"] = len(all_ids)

    roadmap_versions = c.execute(
        "select version, status, proposed_by, proposed_at, decided_by, decided_at from project_roadmap_versions "
        "where project_id=? order by version", (PROJECT,)).fetchall()
    roadmap_changes = [
        {"source": "project_roadmap_versions", "version": v, "status": s, "proposed_by": pb,
         "proposed_at": pa, "decided_by": db, "decided_at": da}
        for v, s, pb, pa, db, da in roadmap_versions
        if (pa or "") >= ROADMAP_SINCE or (da or "") >= ROADMAP_SINCE]
    roadmap_changes += [
        {"source": "project_roadmap_outcome_events", "id": i, "outcome_id": o, "kind": k, "status": st,
         "actor": a, "created_at": ca}
        for i, o, k, st, a, ca in c.execute(
            "select id, outcome_id, kind, status, actor, created_at from project_roadmap_outcome_events "
            "where project_id=? and created_at>=? order by id", (PROJECT, ROADMAP_SINCE))]

    def gate_view(idx):
        child = CHILDREN[idx]
        info = scope["children"][str(idx)]
        return {"child": idx, "task_id": child,
                "decisions": [d for d in decisions if d["child"] == idx],
                "gate_and_question_events": [g for g in gates if g["child"] == idx],
                "done_at": info["done_at"], "last_review_verdict": info["last_review_verdict"]}

    signoffs = c.execute(
        "select id, task_id, status, created_at, answered_at, json from decisions "
        "where key like 'publish-signoff-v%' order by created_at").fetchall()
    signoff_rows = []
    for did, tid, st, ca, aa, raw in signoffs:
        by = (json.loads(raw).get("answer") or {}).get("by")
        signoff_rows.append({"decision_id": did, "task_id": tid, "status": st, "created_at": ca,
                             "answered_at": aa, "actor": by, "actor_kind": kind_of(by)})
    promos = promotions()
    out = {
        "generated_at": now,
        "db": DB_URI,
        "reproduce": "python3 -I agent-docs/progress/2026-11-27-strategy-v1-9/audit_query.py --out <path>",
        "request_sha256": request_sha,
        "request_sha256_expected": EXPECTED_REQUEST_SHA256,
        "request_sha256_matches": request_sha == EXPECTED_REQUEST_SHA256,
        "project_updated_at": proj_updated,
        "roadmap_changes": roadmap_changes,
        "roadmap_state": {"project_roadmap_versions_total": len(roadmap_versions),
                          "project_roadmap_outcome_events_total": c.execute(
                              "select count(*) from project_roadmap_outcome_events where project_id=?",
                              (PROJECT,)).fetchone()[0],
                          "since": ROADMAP_SINCE},
        "scope": scope,
        "decisions": {
            "all": decisions,
            "first_gate": gate_view(FIRST_GATE_CHILD),
            "second_gate": gate_view(SECOND_GATE_CHILD),
            "summary": {
                "count": len(decisions),
                "answered_before_needed": sum(1 for d in decisions if d["answered_before_needed"]),
                "by_recorded_actor_kind": dict(collections.Counter(d["recorded_actor_kind"] for d in decisions)),
                "by_chosen_by_per_note": dict(collections.Counter(d["chosen_by_per_note"] for d in decisions)),
                "plan_gate_decided_by_actor_kind": dict(collections.Counter(
                    g["recorded_actor_kind"] for g in gates if g["type"] == "plan_gate_decided")),
            },
        },
        "promotions_publications": {
            "since": PROMOTION_SINCE,
            "promotions": promos,
            "promotions_total": len(promos),
            "promotions_with_recorded_actor": sum(1 for p in promos if p["actor"]),
            "promotions_by_actor_kind": dict(collections.Counter(p["actor_kind"] for p in promos)),
            "promotions_by_worker_cos_mcp": [p for p in promos if (p["actor_kind"] or "") in NON_HUMAN_PROMOTERS],
            "promotion_or_publish_event_types_in_db": dict(c.execute(
                "select json_extract(json, '$.type') t, count(*) from events where ts>=? and "
                "(t like '%promot%' or t like '%publish%' or t like '%signoff%' or t like 'release%') group by t",
                (PROMOTION_SINCE,)).fetchall()),
            "publish_signoffs": signoff_rows,
            "publish_signoffs_by_worker_cos_mcp": [s for s in signoff_rows
                                                   if (s["actor_kind"] or "") in NON_HUMAN_PROMOTERS],
        },
        "actors": actors(c, all_ids),
    }
    Path(args.out).write_text(json.dumps(out, ensure_ascii=False, indent=2) + "\n")


if __name__ == "__main__":
    main()
