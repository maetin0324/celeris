"""prod_shadow.py の試験。合成の snapshot で 5 つの判定と、誤検知・先行の境界を確かめる。

    python3 -I agent-docs/progress/2026-11-27-strategy-v1/test_prod_shadow.py

守る不変条件: 判定は prod-shadow-rule.md の順（unsafe → inconclusive → falsified → useful → no_signal）で最初に
当たったもの。誤検知は「発火より後・export の時刻まで」の同じ案件の reached、先行は「root の起票以後・発火の時刻まで」に
同じ案件の guard の escalate が無い発火。export は読み取り専用で開き、許した key だけを写す。
既存の試験（test_r1_rule.py・test_r1_bench.py・shadow_bench）は本番の記録の数え方を扱わないので、これらを検出できない。
"""

import copy
import importlib.util
import json
import sqlite3
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
_spec = importlib.util.spec_from_file_location("prod_shadow", HERE / "prod_shadow.py")
ps = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(ps)

P, Q, R = "PROJ_P", "PROJ_Q", "ROOT_R"
T_ROOT = "2026-12-01T00:00:00Z"
T_FIRE = "2026-12-02T00:00:00.000001Z"
T_END = "2026-12-05T00:00:00Z"


def child(i, status="done"):
    return {"task_id": f"C{i}", "status": status, "updated_at": "2026-12-01T12:00:00Z"}


def fire(did="D1", at=T_FIRE, effect="suppressed", project=P, root=R):
    return {"decision_id": did, "project_id": project, "created_at": at, "rule_id": ps.R1_RULE,
            "effect_state": effect, "mode": "shadow", "root_id": root}


def base(fires=(), guards=(), reached=(), mode="shadow", children=None):
    kids = [child(1), child(2, "failed"), child(3, "cancelled")] if children is None else children
    return {
        "schema_id": ps.SNAPSHOT_SCHEMA,
        "exported_at": T_END,
        "k": ps.K,
        "projects": [
            {"project_id": P, "controller_mode": mode, "controller_phase": "active",
             "strategy_roots": [{"root_id": R, "status": "running", "created_at": T_ROOT, "children": kids}]},
            {"project_id": Q, "controller_mode": None, "controller_phase": None, "strategy_roots": []},
        ],
        "decisions": list(fires),
        "guard_escalates": [{"decision_id": f"G{i}", "project_id": p, "created_at": at,
                             "rule_id": "pc/guard/no-progress", "effect_state": "suppressed"}
                            for i, (p, at) in enumerate(guards)],
        "outcome_reached": [{"event_id": str(i), "project_id": p, "outcome_id": f"O{i}", "status": "reached",
                             "created_at": at} for i, (p, at) in enumerate(reached)],
        "controller_tasks": [],
        "controller_cos_inputs": [],
        "controller_inbox_items": [],
        "script_domains": [{"domain_id": "project_controller", "mode": "compare", "generation_id": "GEN",
                            "updated_at": T_ROOT}],
        "counts": {"n_projects": 2, "n_decisions": len(fires), "n_r1_waiting": 0, "n_strategy_creates": 1},
    }


def verdict(snap):
    return ps.judge(snap)["verdict"]


class Verdicts(unittest.TestCase):
    def test_unsafe_when_r1_effect_not_suppressed(self):
        self.assertEqual(verdict(base([fire(effect="applied")])), "unsafe")
        self.assertEqual(verdict(base([fire(effect="none")])), "unsafe")

    def test_unsafe_for_each_controller_artefact_and_before_inconclusive(self):
        for key, row in (("controller_tasks", {"task_id": "T", "project_id": P, "status": "ready",
                                               "created_at": T_FIRE}),
                         ("controller_cos_inputs", {"message_id": "M", "thread_id": "TH", "run_id": None,
                                                    "state": "queued", "created_at": T_FIRE}),
                         ("controller_inbox_items", {"item_id": "I", "state": "pending", "created_at": T_FIRE})):
            snap = base(mode="off")  # 安全違反は inconclusive より先に当たる
            snap[key] = [row]
            self.assertEqual(verdict(snap), "unsafe", key)

    def test_inconclusive_without_shadow_project_root_or_k_terminal_children(self):
        self.assertEqual(verdict(base([fire()], mode="enabled")), "inconclusive")
        no_root = base([fire()])
        no_root["projects"][0]["strategy_roots"] = []
        self.assertEqual(verdict(no_root), "inconclusive")
        two = base([fire()], children=[child(1), child(2), child(3, "running")])
        self.assertEqual(verdict(two), "inconclusive")
        self.assertEqual(verdict(base([fire()], children=[child(1), child(2), child(3, "running"), child(4)])),
                         "useful")

    def test_falsified_with_two_false_positives_even_with_leads(self):
        snap = base([fire("D1"), fire("D2", at="2026-12-03T00:00:00Z")], reached=[(P, "2026-12-04T00:00:00Z")])
        result = ps.judge(snap)
        self.assertEqual(result["counts"]["false_positives"], 2)
        self.assertEqual(result["counts"]["leads"], 2)
        self.assertEqual(result["verdict"], "falsified")

    def test_useful_with_a_lead_and_no_false_positive(self):
        self.assertEqual(verdict(base([fire()])), "useful")

    def test_no_signal_without_fires_or_with_one_false_positive(self):
        self.assertEqual(verdict(base()), "no_signal")
        self.assertEqual(verdict(base([fire()], reached=[(P, "2026-12-03T00:00:00Z")])), "no_signal")
        # 先行が 0（guard が先）で誤検知 0 も no_signal。
        self.assertEqual(verdict(base([fire()], guards=[(P, "2026-12-01T06:00:00Z")])), "no_signal")

    def test_waiting_decisions_are_not_fires(self):
        snap = base()
        snap["counts"]["n_r1_waiting"] = 4
        result = ps.judge(snap)
        self.assertEqual(result["counts"]["r1_fires"], 0)
        self.assertEqual(result["counts"]["r1_waiting"], 4)


class FalsePositiveBoundary(unittest.TestCase):
    def fp(self, *reached):
        return ps.judge(base([fire()], reached=list(reached)))["fires"][0]["false_positive"]

    def test_reached_at_the_fire_time_is_not_after(self):
        self.assertFalse(self.fp((P, T_FIRE)))
        # 小数 9 桁の同時刻（6 桁に切ると同じ）も「後」ではない。
        self.assertFalse(self.fp((P, "2026-12-02T00:00:00.000001999Z")))

    def test_reached_just_after_the_fire_counts(self):
        self.assertTrue(self.fp((P, "2026-12-02T00:00:00.000002Z")))

    def test_reached_at_export_time_counts_and_after_does_not(self):
        self.assertTrue(self.fp((P, T_END)))
        self.assertTrue(self.fp((P, "2026-12-05T09:00:00+09:00")))  # 同じ時刻の別の書き方
        self.assertFalse(self.fp((P, "2026-12-05T00:00:00.000001Z")))

    def test_reached_before_the_fire_or_in_another_project_does_not_count(self):
        self.assertFalse(self.fp((P, "2026-12-01T23:59:59Z")))
        self.assertFalse(self.fp((Q, "2026-12-03T00:00:00Z")))


class LeadBoundary(unittest.TestCase):
    def lead(self, *guards):
        return ps.judge(base([fire()], guards=list(guards)))["fires"][0]["lead"]

    def test_no_guard_is_a_lead(self):
        self.assertTrue(self.lead())

    def test_guard_at_the_fire_time_is_not_after_the_fire(self):
        self.assertFalse(self.lead((P, T_FIRE)))

    def test_guard_just_after_the_fire_keeps_the_lead(self):
        self.assertTrue(self.lead((P, "2026-12-02T00:00:00.000002Z")))

    def test_guard_between_root_creation_and_fire_is_not_a_lead(self):
        self.assertFalse(self.lead((P, T_ROOT)))  # root の起票と同時刻は「以後」に入る
        self.assertFalse(self.lead((P, "2026-12-01T18:00:00Z")))

    def test_guard_before_the_root_or_in_another_project_is_ignored(self):
        self.assertTrue(self.lead((P, "2026-11-30T23:59:59Z")))
        self.assertTrue(self.lead((Q, "2026-12-01T18:00:00Z")))

    def test_root_missing_from_snapshot_uses_every_guard_of_the_project(self):
        snap = base([fire(root="ROOT_GONE")], guards=[(P, "2026-11-30T00:00:00Z")])
        self.assertFalse(ps.judge(snap)["fires"][0]["lead"])


class SnapshotShape(unittest.TestCase):
    def test_base_snapshot_has_no_problems(self):
        self.assertEqual(ps.snapshot_problems(base([fire()])), [])

    def test_disallowed_key_and_long_string_are_problems(self):
        snap = base([fire()])
        snap["decisions"][0]["summary"] = "x"
        self.assertTrue(any("key not allowed" in p for p in ps.snapshot_problems(snap)))
        snap = base([fire(did="x" * 201)])
        self.assertTrue(any("over 200" in p for p in ps.snapshot_problems(snap)))
        self.assertEqual(ps.snapshot_problems(base([fire(did="x" * 200)])), [])

    def test_count_and_container_types(self):
        snap = base()
        snap["counts"]["n_decisions"] = "3"
        snap["k"] = True
        problems = ps.snapshot_problems(snap)
        self.assertEqual(sum("count must be an integer" in p for p in problems), 2)
        snap = base()
        snap["projects"][0]["status"] = {"nested": 1}
        self.assertTrue(any("scalar key holds a container" in p for p in ps.snapshot_problems(snap)))

    def test_verify_recounts_and_fails_on_drift_or_bad_snapshot(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            self.assertEqual(ps.verify(d), 1)  # snapshot が無い
            snap = base([fire()])
            (d / "x-snapshot.json").write_text(ps.dumps(snap))
            self.assertEqual(ps.verify(d), 1)  # 結果が無い
            (d / "x.json").write_text(ps.dumps(ps.judge(snap)))
            self.assertEqual(ps.verify(d), 0)
            drift = ps.judge(snap)
            drift["verdict"] = "falsified"
            (d / "x.json").write_text(ps.dumps(drift))
            self.assertEqual(ps.verify(d), 1)
            bad = copy.deepcopy(snap)
            bad["decisions"][0]["question"] = "body"
            (d / "x-snapshot.json").write_text(ps.dumps(bad))
            (d / "x.json").write_text(ps.dumps(ps.judge(bad)))
            self.assertEqual(ps.verify(d), 1)

    def test_result_is_sorted_indented_json_with_verdict(self):
        text = ps.dumps(ps.judge(base([fire()])))
        self.assertEqual(text, json.dumps(json.loads(text), indent=2, sort_keys=True) + "\n")
        self.assertIn(json.loads(text)["verdict"], ps.VERDICTS)


SCHEMA = """
CREATE TABLE projects (id TEXT PRIMARY KEY, created_at TEXT NOT NULL);
CREATE TABLE tasks (id TEXT PRIMARY KEY, status TEXT NOT NULL, parent_id TEXT, created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL DEFAULT '', json TEXT NOT NULL, project_id TEXT, labels_json TEXT);
CREATE TABLE cos_operations (id TEXT PRIMARY KEY, target_id TEXT NOT NULL, action TEXT NOT NULL,
  payload_json TEXT NOT NULL, state TEXT NOT NULL, created_at TEXT NOT NULL);
CREATE TABLE project_controller_state (project_id TEXT PRIMARY KEY, mode TEXT NOT NULL, phase TEXT NOT NULL);
CREATE TABLE project_controller_decisions (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, mode TEXT NOT NULL,
  validation TEXT NOT NULL, effect_json TEXT NOT NULL, effect_state TEXT NOT NULL, created_at TEXT NOT NULL);
CREATE TABLE project_roadmap_outcome_events (id INTEGER PRIMARY KEY, project_id TEXT NOT NULL,
  outcome_id TEXT NOT NULL, kind TEXT NOT NULL, status TEXT, created_at TEXT NOT NULL);
CREATE TABLE chat_messages (id TEXT PRIMARY KEY, thread_id TEXT NOT NULL, client_message_id TEXT,
  run_id TEXT, state TEXT NOT NULL, created_at TEXT NOT NULL, text TEXT NOT NULL DEFAULT '');
CREATE TABLE feed_sources (source_key TEXT PRIMARY KEY, notice_id TEXT NOT NULL);
CREATE TABLE cos_inbox_items (id TEXT PRIMARY KEY, source_kind TEXT NOT NULL, source_key TEXT NOT NULL,
  state TEXT NOT NULL, created_at TEXT NOT NULL, summary TEXT NOT NULL DEFAULT '');
CREATE TABLE script_domain_modes (domain TEXT PRIMARY KEY, mode TEXT NOT NULL, generation_id TEXT,
  updated_at TEXT NOT NULL);
"""


def seed(path):
    c = sqlite3.connect(path)
    c.executescript(SCHEMA)
    root_json = json.dumps({"tree": None, "conversation": None})
    strat = json.dumps({"strategy": {"name": "strategy/s1/v2.json", "task_id": "DOC"}})
    c.executemany("INSERT INTO projects VALUES (?, ?)", [(P, "2026-01-01T00:00:00Z"), (Q, "2026-01-02T00:00:00Z")])
    c.executemany("INSERT INTO tasks VALUES (?,?,?,?,?,?,?,?)", [
        (R, "running", None, T_ROOT, T_ROOT, root_json, P, "[]"),
        ("C1", "done", R, T_ROOT, T_ROOT, root_json, P, "[]"),
        ("C2", "running", R, T_ROOT, T_ROOT, root_json, P, "[]"),
        ("PLAIN", "ready", None, T_ROOT, T_ROOT, root_json, P, "[]"),          # strategy の参照なし
        ("BADNAME", "ready", None, T_ROOT, T_ROOT, root_json, P, "[]"),        # 版の先頭 0
        ("CONV", "ready", None, T_ROOT, T_ROOT, json.dumps({"conversation": {"x": 1}}), P, "[]"),
        ("PCROOT", "ready", None, T_ROOT, T_ROOT, root_json, Q, json.dumps(["project-controller"])),
    ])
    c.executemany("INSERT INTO cos_operations VALUES (?,?,?,?,?,?)", [
        ("O1", R, "task.create", strat, "applied", "2026-12-01T00:00:01Z"),
        ("O2", "BADNAME", "task.create", json.dumps({"strategy": {"name": "strategy/s1/v02.json"}}), "applied",
         "2026-12-01T00:00:02Z"),
        ("O3", "CONV", "task.create", strat, "applied", "2026-12-01T00:00:03Z"),
        ("O4", "PLAIN", "task.create", strat, "rejected", "2026-12-01T00:00:04Z"),
        ("O5", "C1", "task.create", strat, "applied", "2026-12-01T00:00:05Z"),     # root でない
    ])
    c.execute("INSERT INTO project_controller_state VALUES (?, 'shadow', 'active')", (P,))
    esc = lambda key: json.dumps({"kind": "open_escalation", "question_key": key, "category": "c", "summary": "s"})
    c.executemany("INSERT INTO project_controller_decisions VALUES (?,?,?,?,?,?,?)", [
        ("D1", P, "shadow", "pc/r1-stall", esc(f"r1_stall:{R}:3"), "suppressed", T_FIRE),
        ("D2", P, "shadow", "pc/r1-stall/waiting", json.dumps({"kind": "none"}), "none", T_FIRE),
        ("D3", P, "shadow", "pc/guard/revoked:" + "O" * 300, esc("g"), "suppressed", T_FIRE),
        ("D4", P, "shadow", "pc/no-progress", esc("no_progress:1"), "suppressed", T_FIRE),
        ("D5", P, "shadow", "pc/guard/paused", json.dumps({"kind": "none"}), "none", T_FIRE),
        ("D6", P, "shadow", "pc/continue", json.dumps({"kind": "none"}), "none", T_FIRE),
    ])
    c.executemany("INSERT INTO project_roadmap_outcome_events VALUES (?,?,?,?,?,?)", [
        (1, P, "o1", "status", "reached", T_END), (2, P, "o2", "status", "in_progress", T_END),
        (3, P, "o3", "root_linked", None, T_END)])
    c.executemany("INSERT INTO chat_messages (id, thread_id, client_message_id, run_id, state, created_at, text) "
                  "VALUES (?,?,?,?,?,?,?)", [
                      ("M1", "TH", "pc-input:P:replan-1", "RUN", "completed", T_FIRE, "secret body"),
                      ("M2", "TH", "user-1", None, "completed", T_FIRE, "x")])
    c.executemany("INSERT INTO feed_sources VALUES (?, ?)", [
        ("project_controller:escalate:P:g", "N1"), ("event:1", "N2")])
    c.executemany("INSERT INTO cos_inbox_items (id, source_kind, source_key, state, created_at) VALUES (?,?,?,?,?)",
                  [("I1", "notice", "N1", "pending", T_FIRE), ("I2", "notice", "N2", "pending", T_FIRE),
                   ("I3", "question", "N1", "pending", T_FIRE)])
    c.execute("INSERT INTO script_domain_modes VALUES ('project_controller', 'rust', NULL, ?)", (T_ROOT,))
    c.commit()
    c.close()


class Export(unittest.TestCase):
    def test_export_picks_the_same_rows_as_the_controller_and_stays_read_only(self):
        with tempfile.TemporaryDirectory() as tmp:
            db = Path(tmp) / "db.sqlite3"
            seed(db)
            before = db.read_bytes()
            out = Path(tmp) / "out" / "x-snapshot.json"
            self.assertEqual(ps.main(["export", "--db", str(db), "--out", str(out)]), 0)
            self.assertEqual(db.read_bytes(), before)
            snap = json.loads(out.read_text())
            self.assertEqual(ps.snapshot_problems(snap), [])
            p = {x["project_id"]: x for x in snap["projects"]}
            self.assertEqual([r["root_id"] for r in p[P]["strategy_roots"]], [R])
            self.assertEqual([c["task_id"] for c in p[P]["strategy_roots"][0]["children"]], ["C1", "C2"])
            self.assertEqual((p[P]["controller_mode"], p[Q]["controller_mode"]), ("shadow", None))
            self.assertEqual([(d["decision_id"], d["root_id"]) for d in snap["decisions"]], [("D1", R)])
            self.assertEqual([(g["decision_id"], g["rule_id"]) for g in snap["guard_escalates"]],
                             [("D3", "pc/guard/revoked"), ("D4", "pc/no-progress")])
            self.assertEqual([e["outcome_id"] for e in snap["outcome_reached"]], ["o1"])
            self.assertEqual([t["task_id"] for t in snap["controller_tasks"]], ["PCROOT"])
            self.assertEqual([m["message_id"] for m in snap["controller_cos_inputs"]], ["M1"])
            self.assertEqual([i["item_id"] for i in snap["controller_inbox_items"]], ["I1"])
            self.assertEqual(snap["counts"], {"n_projects": 2, "n_decisions": 6, "n_r1_waiting": 1,
                                              "n_strategy_creates": 4})
            self.assertNotIn("secret body", out.read_text())
            self.assertEqual(ps.judge(snap)["verdict"], "unsafe")  # controller の起票・入力・受信箱がある

    def test_read_only_connection_refuses_writes(self):
        with tempfile.TemporaryDirectory() as tmp:
            db = Path(tmp) / "db.sqlite3"
            seed(db)
            conn = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
            with self.assertRaises(sqlite3.OperationalError):
                conn.execute("DELETE FROM tasks")
            conn.close()


if __name__ == "__main__":
    unittest.main()
