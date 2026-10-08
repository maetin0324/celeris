"""Supplemental live answer comparison; run through cos-chat-bench.sh answers.

Fixture writes target only the daemon's temporary SQLite DB. Actual answers must
come from the CoS operation API and be bound to the requested chat run.
"""
import datetime
import json
import os
import sqlite3

QUESTION = "確認用の色を答えてください。"
ANSWER = "青色を選びます。"
COUNT = 3


def verify_answer(conn, task_id, run_id):
    operations = [dict(zip(("id", "run_id", "state", "payload", "event_id"), row))
                  for row in conn.execute(
                      "SELECT id,run_id,state,payload_json,event_id FROM cos_operations "
                      "WHERE target_id=? AND action='question.answer'", (task_id,))]
    events = {row[0]: json.loads(row[1]) for row in conn.execute(
        "SELECT id,json FROM events WHERE task_id=?", (task_id,))}
    answered = [e for e in events.values() if e.get("type") == "answered"]
    transitions = [e for e in events.values() if e.get("type") == "transitioned"
                   and e.get("reason") == "answer"]
    task = json.loads(conn.execute("SELECT json FROM tasks WHERE id=?", (task_id,)).fetchone()[0])
    applied = [o for o in operations if o["state"] == "applied" and o["run_id"] == run_id]
    audit_ok = any(e.get("type") == "cos_operation" and e.get("operation_id") == o["id"]
                   and e.get("run_id") == run_id and e.get("actor") == "cos"
                   and e.get("state") == "applied" for o in applied for e in events.values())
    ok = (len(applied) == 1 and audit_ok
          and answered == [{"type": "answered", "question": QUESTION, "answer": ANSWER}]
          and any(e.get("from") == "blocked" and e.get("to") == "ready" for e in transitions)
          and task["status"] == "ready" and task.get("paused_at") is not None)
    return {"task_id": task_id, "run_id": run_id, "success": ok,
            "operations": operations, "answered_events": answered,
            "answer_transitions": transitions, "audit_applied": audit_ok,
            "task_status": task["status"], "paused_at": task.get("paused_at")}


def run_answers(call, turn, new_thread, db, evidence):
    results = []
    for idx in range(1, COUNT + 1):
        task = call("POST", "/tasks", {
            "title": f"bench: answer fixture {idx}", "objective": "回答比較用。作業不要。",
            "acceptance": [{"type": "command", "cmd": "true", "expect_exit": 0}]})
        task_id = task["id"]
        call("POST", f"/tasks/{task_id}/pause", {})
        now = datetime.datetime.now(datetime.timezone.utc).isoformat().replace("+00:00", "Z")
        # No worker run is needed to seed a question. Preserve the pause and record
        # both the fixture question and its transition in the temporary event store.
        with sqlite3.connect(db, timeout=30) as conn:
            row = conn.execute("SELECT json,status FROM tasks WHERE id=?", (task_id,)).fetchone()
            snap = json.loads(row[0])
            snap.update(status="blocked", updated_at=now)
            conn.execute("UPDATE tasks SET status='blocked',json=? WHERE id=?", (json.dumps(snap), task_id))
            seq = conn.execute("SELECT max(seq) FROM events WHERE task_id=?", (task_id,)).fetchone()[0] or 0
            for event in ({"type": "question_raised", "run_id": "bench-fixture", "text": QUESTION},
                          {"type": "transitioned", "from": row[1], "to": "blocked", "reason": "bench_fixture"}):
                seq += 1
                conn.execute("INSERT INTO events(task_id,seq,ts,json) VALUES(?,?,?,?)",
                             (task_id, seq, now, json.dumps(event)))
        thread = new_thread(f"answer-{idx}")
        text = (f"試験用 task {task_id} の質問「{QUESTION}」へ「{ANSWER}」と回答してください。"
                "回答を適用し、適用結果を1行で返してください。taskの一時停止は維持してください。")
        turn({}, "answers", thread, 1, "answer", text)
        run = call("GET", f"/chat/threads/{thread}/runs?limit=100")["items"]
        human_run = next(r for r in run if r.get("thread_id") == thread)
        with sqlite3.connect(f"file:{db}?mode=ro", uri=True) as conn:
            result = verify_answer(conn, task_id, human_run["id"])
        result.update(thread_id=thread, state=human_run["state"],
                      account_id=human_run.get("account_id"), llm_source=human_run.get("llm_source"))
        result["success"] = (result["success"] and result["state"] == "completed"
                             and result["account_id"] == "claude_max_lab"
                             and result["llm_source"] == "claude_oauth")
        results.append(result)
        json.dump({"requested": COUNT, "verified": len(results),
                   "succeeded": sum(r["success"] for r in results), "results": results},
                  open(os.path.join(evidence, "answers.json"), "w"), ensure_ascii=False, indent=2)
    if not all(r["success"] for r in results):
        raise RuntimeError("live answer verification failed; see answers.json")
