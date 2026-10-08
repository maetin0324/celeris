"""Guard the live answer verdict against completed runs without a domain write."""
import json
import sqlite3
import unittest

from cos_chat_answer_bench import ANSWER, QUESTION, verify_answer


class AnswerVerdictTests(unittest.TestCase):
    def setUp(self):
        self.conn = sqlite3.connect(":memory:")
        self.addCleanup(self.conn.close)
        self.conn.executescript("""
            CREATE TABLE tasks(id TEXT,json TEXT);
            CREATE TABLE events(id INTEGER PRIMARY KEY,task_id TEXT,json TEXT);
            CREATE TABLE cos_operations(id TEXT,run_id TEXT,state TEXT,payload_json TEXT,
                                        event_id INTEGER,target_id TEXT,action TEXT);
        """)
        self.conn.execute("INSERT INTO tasks VALUES('task',?)", (json.dumps({
            "status": "ready", "paused_at": "fixture"}),))
        self.conn.execute("INSERT INTO cos_operations VALUES('op','run','applied','{}',99,'task','question.answer')")
        self.event({"type": "answered", "question": QUESTION, "answer": ANSWER})
        self.event({"type": "transitioned", "from": "blocked", "to": "ready", "reason": "answer"})
        self.event({"type": "cos_operation", "operation_id": "op", "actor": "cos",
                    "run_id": "run", "state": "applied"})

    def event(self, value):
        self.conn.execute("INSERT INTO events(task_id,json) VALUES('task',?)", (json.dumps(value),))

    def test_success_requires_domain_answer_transition_and_audit(self):
        self.assertTrue(verify_answer(self.conn, "task", "run")["success"])

    def test_wrong_run_cannot_take_credit(self):
        self.assertFalse(verify_answer(self.conn, "task", "other-run")["success"])

    def test_missing_answer_is_failure(self):
        self.conn.execute("DELETE FROM events WHERE json_extract(json,'$.type')='answered'")
        self.assertFalse(verify_answer(self.conn, "task", "run")["success"])

    def test_missing_audit_is_failure(self):
        self.conn.execute("DELETE FROM events WHERE json_extract(json,'$.type')='cos_operation'")
        self.assertFalse(verify_answer(self.conn, "task", "run")["success"])

    def test_wrong_answer_is_failure(self):
        self.conn.execute("UPDATE events SET json=json_set(json,'$.answer','wrong') WHERE json_extract(json,'$.type')='answered'")
        self.assertFalse(verify_answer(self.conn, "task", "run")["success"])

    def test_rejected_operation_is_failure(self):
        self.conn.execute("UPDATE cos_operations SET state='rejected'")
        self.assertFalse(verify_answer(self.conn, "task", "run")["success"])


if __name__ == "__main__":
    unittest.main()
