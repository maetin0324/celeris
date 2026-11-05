#!/usr/bin/env python3
"""pilot_effect.py の broker の試験（ADR 2026-11-05-phase3-achievement-invariants の I10、K17〜K20・K25〜K27）。

daemon の approvals API と task の events API は loopback の偽の server が返す（形は本物の daemon と同じ。本物の
daemon を通す確認は `test-long-horizon-pilot.sh` の段 2b）。broker は本物の process として起こし、client は
worker に公開する wrapper（`pilot-effect publish <logical_id>`）を subprocess で呼ぶ。時間依存の待ちは置かず、
並行は broker の試験 hook（arrived / inside / go の file）で順序を決める。

    python3 -I scripts/dev/long-horizon-pilot/test_pilot_effect.py -v
"""
import http.server
import json
import os
import pathlib
import secrets
import subprocess
import sys
import tempfile
import threading
import time
import unittest

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import pilot_effect  # noqa: E402

PROJECT = "01PROJECT00000000000000000"
LID = "C06:o1:publish"


class FakeDaemon:
    """`GET /api/v1/approvals?project=` と `GET /api/v1/tasks/{id}/events?types=answered` だけを返す。"""

    def __init__(self):
        self.approvals = []
        self.answers = {}
        daemon = self

        class H(http.server.BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def do_GET(self):
                if self.headers.get("Authorization") != "Bearer t0ken":
                    self.send_response(401)
                    self.end_headers()
                    return
                if self.path.startswith("/api/v1/approvals?"):
                    body = {"items": daemon.approvals}
                elif self.path.startswith("/api/v1/tasks/"):
                    task = self.path.split("/")[4]
                    body = {"items": [{"seq": i, "event": e} for i, e in enumerate(daemon.answers.get(task, []))]}
                else:
                    self.send_response(404)
                    self.end_headers()
                    return
                data = json.dumps(body).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(data)

        self.srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
        threading.Thread(target=self.srv.serve_forever, daemon=True).start()
        self.base = f"http://127.0.0.1:{self.srv.server_address[1]}"

    def ask(self, logical_id, task="T1"):
        a = {"id": f"A{len(self.approvals) + 1:025d}", "project_id": PROJECT, "node_id": "x", "task_id": task,
             "question": f"{pilot_effect.MARKER}{logical_id} を公開してよいですか", "created_at": "2026-11-05T00:00:00Z"}
        self.approvals.append(a)
        return a

    def decide(self, a, decision, by="human:pilot"):
        a["decision"] = decision
        a["decided_at"] = f"2026-11-05T00:00:{len(self.approvals):02d}Z"
        self.answers.setdefault(a["task_id"], []).append(
            {"type": "answered", "question": a["question"], "answer": decision, "by": by})

    def close(self):
        self.srv.shutdown()


class BrokerCase(unittest.TestCase):
    def setUp(self):
        self.tmp = pathlib.Path(tempfile.mkdtemp(prefix="pilot-effect-test.", dir=os.environ.get("TMPDIR")))
        self.dir = self.tmp / "record"
        self.key = secrets.token_bytes(32)
        self.socket = f"celeris-pilot-effect-test-{secrets.token_hex(8)}"
        (self.tmp / "token").write_text("t0ken\n")
        self.daemon = FakeDaemon()
        self.cmd = pilot_effect.write_wrapper(self.tmp / "bin" / "pilot-effect", self.socket)
        self.broker = None
        self.reader = None
        self.head = None

    def tearDown(self):
        self.stop()
        self.daemon.close()
        subprocess.run(["chmod", "-R", "u+w", str(self.tmp)], check=False)
        subprocess.run(["rm", "-rf", str(self.tmp)], check=False)

    def start(self, **env):
        self.stop()
        e = {k: v for k, v in os.environ.items() if not k.startswith("PILOT_EFFECT_TEST_")}
        e.update(env)
        self.broker = subprocess.Popen(
            [sys.executable, "-I", str(HERE / "pilot_effect.py"), "serve", "--dir", str(self.dir), "--socket",
             self.socket, "--api", self.daemon.base, "--token-file", str(self.tmp / "token"), "--project", PROJECT],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=e)
        self.broker.stdin.write(self.key.hex() + "\n")
        self.broker.stdin.close()
        if self.broker.stdout.readline().strip() != "ready":
            self.fail(f"the broker did not start: {self.broker.stderr.read()}")
        self.reader = pilot_effect.HeadReader(self.broker.stdout)

    def stop(self):
        if self.broker is not None:
            if self.broker.poll() is None:
                self.broker.terminate()
            self.broker.wait(timeout=30)
            self.head = self.reader.wait() or self.head
            self.broker.stderr.close()
            self.broker.stderr.close()
        self.broker = None

    def publish(self, logical_id=LID):
        """worker と同じく、公開された command だけを呼ぶ。"""
        return subprocess.run([str(self.cmd), "publish", logical_id], capture_output=True, text=True).returncode

    def effects(self):
        return pilot_effect.verify(self.dir, self.key)[0]

    def audit(self):
        return [(a.get("logical_id"), a.get("result") or a.get("event")) for a in pilot_effect.verify(self.dir, self.key)[1]]

    # ---- K17 ----
    def test_k17_publish_before_approval_is_refused_with_no_effect(self):
        self.start()
        self.assertEqual(self.publish(), 3)
        pending = self.daemon.ask(LID)  # 承認を求めたが未決
        self.assertEqual(self.publish(), 3)
        self.daemon.decide(self.daemon.ask("C06:o2:publish"), "once")  # 別の logical_id の承認は効かない
        self.assertEqual(self.publish(), 3)
        self.assertEqual(self.effects(), [])
        self.assertEqual(self.audit(), [(LID, "refused_unapproved")] * 3)
        self.assertIsNone(pending.get("decision"))

    def test_k17_decision_not_made_by_a_human_does_not_count(self):
        self.start()
        self.daemon.decide(self.daemon.ask(LID), "once", by="cos:01THREAD")
        self.assertEqual(self.publish(), 3)
        self.daemon.decide(self.daemon.ask(LID, task="T2"), "standing", by="operator:pilot")
        self.assertEqual(self.publish(), 3)
        self.assertEqual(self.effects(), [])

    # ---- K18 ----
    def test_k18_approved_publishes_exactly_once(self):
        self.start()
        self.assertEqual(self.publish(), 3)
        self.daemon.decide(self.daemon.ask(LID), "once")
        self.assertEqual(self.publish(), 0)
        self.assertEqual(self.publish(), 0)
        effects = self.effects()
        self.assertEqual([e["logical_id"] for e in effects], [LID])
        self.assertEqual(effects[0]["decision"], "once")
        self.assertEqual(self.audit(), [(LID, "refused_unapproved"), (LID, "published"), (LID, "duplicate")])

    # ---- K25 ----
    def test_k25_rejected_by_the_daemon_api_has_no_effect(self):
        self.start()
        self.daemon.decide(self.daemon.ask(LID), "denied")
        self.assertEqual(self.publish(), 4)
        self.assertEqual(self.publish(), 4)
        self.assertEqual(self.effects(), [])
        audit = pilot_effect.verify(self.dir, self.key)[1]
        self.assertEqual([(a["result"], a["decision"]) for a in audit], [("rejected", "denied")] * 2)

    def test_k25_latest_human_decision_wins(self):
        self.start()
        a = self.daemon.ask(LID)
        self.daemon.decide(a, "denied")
        self.assertEqual(self.publish(), 4)
        self.daemon.decide(self.daemon.ask(LID, task="T2"), "once")  # 人が後で承認し直した
        self.assertEqual(self.publish(), 0)
        self.assertEqual(len(self.effects()), 1)

    # ---- K26 ----
    def test_k26_crash_before_the_record_then_rerun_gives_one(self):
        self.daemon.decide(self.daemon.ask(LID), "once")
        self.start(PILOT_EFFECT_TEST_CRASH="before_record")
        self.assertEqual(self.publish(), 5)  # broker が落ちた
        self.assertEqual(self.broker.wait(timeout=30), 9)
        self.assertEqual(self.effects(), [])
        self.start()
        self.assertEqual(self.publish(), 0)
        self.assertEqual(len(self.effects()), 1)

    def test_k26_crash_after_the_record_then_rerun_stays_one(self):
        self.daemon.decide(self.daemon.ask(LID), "once")
        self.start(PILOT_EFFECT_TEST_CRASH="after_record")
        self.assertEqual(self.publish(), 5)
        self.assertEqual(self.broker.wait(timeout=30), 9)
        self.assertEqual(len(self.effects()), 1)
        self.start()
        self.assertEqual(self.publish(), 0)
        self.assertEqual(len(self.effects()), 1)
        self.assertEqual(self.audit()[-1], (LID, "duplicate"))

    def test_k26_torn_write_is_cut_and_rerun_gives_one(self):
        self.daemon.decide(self.daemon.ask(LID), "once")
        self.start(PILOT_EFFECT_TEST_CRASH="torn")
        self.assertEqual(self.publish(), 5)
        self.assertEqual(self.broker.wait(timeout=30), 9)
        with self.assertRaises(pilot_effect.ChainError):
            self.effects()  # 欠けた行のままでは facts を作らない
        self.start()
        self.assertEqual(self.publish(), 0)
        self.assertEqual(len(self.effects()), 1)
        self.assertIn((None, "recovered_torn_tail"), self.audit())

    def test_k26_crash_hook_before_approval_leaves_zero(self):
        self.start(PILOT_EFFECT_TEST_CRASH="before_record")
        self.assertEqual(self.publish(), 3)  # 承認前は記録の地点に届かない
        self.assertIsNone(self.broker.poll())
        self.assertEqual(self.effects(), [])

    # ---- K27 ----
    def test_k27_concurrent_publish_of_one_logical_id_records_once(self):
        self.daemon.decide(self.daemon.ask(LID), "once")
        gate = self.tmp / "gate"
        gate.mkdir()
        self.start(PILOT_EFFECT_TEST_GATE=str(gate))
        first = subprocess.Popen([str(self.cmd), "publish", LID], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.wait_for(gate / "inside.1")  # 1 本目は記録を読み、追記の前で止まっている
        second = subprocess.Popen([str(self.cmd), "publish", LID], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.wait_for(gate / "arrived.2")  # 2 本目は broker に届いた
        self.assertFalse((gate / "inside.2").exists(), "two requests read the record at the same time")
        (gate / "go").write_text("")
        self.assertEqual(first.wait(timeout=60), 0)
        self.assertEqual(second.wait(timeout=60), 0)
        self.assertEqual(len(self.effects()), 1)
        self.assertEqual(sorted(r for _, r in self.audit()), ["duplicate", "published"])

    def wait_for(self, path):
        # 出来事待ち（file が置かれるまで）。broker の process が落ちたら即失敗。上限は保険。
        deadline = time.monotonic() + 60
        while not path.exists():
            self.assertIsNone(self.broker.poll(), "the broker exited")
            self.assertLess(time.monotonic(), deadline, f"{path} did not appear")
            time.sleep(0.02)

    # ---- K19・K20 ----
    def test_k19_worker_cannot_write_the_record_in_place(self):
        if os.geteuid() == 0:
            self.skipTest("root ignores file modes")
        self.start()
        self.assertEqual(self.publish(), 3)
        for name in ("effects.jsonl", "audit.jsonl"):
            with self.assertRaises(PermissionError):
                open(self.dir / name, "a")
        with self.assertRaises(PermissionError):
            (self.dir / "extra.jsonl").write_text("x")
        # worker に公開した command は記録の場所を持たず、場所を渡す引数も受けない。
        self.assertNotIn(str(self.dir), self.cmd.read_text())
        rc = subprocess.run([str(self.cmd), "publish", LID, "--dir", str(self.dir)], capture_output=True).returncode
        self.assertEqual(rc, 2)

    def test_k19_forged_edited_or_deleted_rows_are_detected(self):
        self.daemon.decide(self.daemon.ask(LID), "once")
        self.start()
        self.assertEqual(self.publish(), 0)
        self.stop()
        p = self.dir / "effects.jsonl"
        good = p.read_text()
        os.chmod(self.dir, 0o700)
        os.chmod(p, 0o600)
        row = json.loads(good)
        forged = dict(row, seq=2, logical_id="C06:o2:publish", prev=row["mac"])
        cases = {
            "forged row": good + json.dumps(forged) + "\n",
            "edited row": good.replace(LID, "C06:o2:publish"),
            "deleted row (effects emptied, audit kept)": "",
        }
        for name, text in cases.items():
            with self.subTest(name):
                p.write_text(text)
                with self.assertRaises(pilot_effect.ChainError):
                    pilot_effect.verify(self.dir, self.key)
        # 改ざんの後、broker は記録を直さず、以後の publish も断る。
        # 2 つの file を両方とも空にする（鎖も 1 対 1 も保たれる）と、broker が知らせた末尾で分かる。
        a = self.dir / "audit.jsonl"
        os.chmod(a, 0o600)
        audit_good = a.read_text()
        p.write_text("")
        a.write_text("")
        self.assertEqual(pilot_effect.verify(self.dir, self.key), ([], []))
        with self.assertRaises(pilot_effect.ChainError):
            pilot_effect.verify(self.dir, self.key, self.head)
        # 動いている broker の記録に偽の行を足すと、以後の publish は断られ（exit 5）、記録は直されない。
        a.write_text(audit_good)
        p.write_text(good)
        self.start()
        os.chmod(self.dir, 0o700)
        os.chmod(p, 0o600)
        with open(p, "a") as f:
            f.write(json.dumps(forged) + "\n")
        self.assertEqual(self.publish("C06:o9:publish"), 5)
        self.stop()
        with self.assertRaises(pilot_effect.ChainError):
            pilot_effect.verify(self.dir, self.key)
        # 改ざんされた記録では broker が起動しない（driver は試行を失敗にする）。
        self.broker = subprocess.Popen(
            [sys.executable, "-I", str(HERE / "pilot_effect.py"), "serve", "--dir", str(self.dir), "--socket",
             self.socket, "--api", self.daemon.base, "--token-file", str(self.tmp / "token"), "--project", PROJECT],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        out, err = self.broker.communicate(self.key.hex() + "\n", timeout=60)
        self.broker = None
        self.assertNotEqual(out.strip(), "ready")
        self.assertIn("MAC does not match", err)

    def test_k19_another_key_cannot_produce_a_valid_record(self):
        self.daemon.decide(self.daemon.ask(LID), "once")
        self.start()
        self.assertEqual(self.publish(), 0)
        with self.assertRaises(pilot_effect.ChainError):
            pilot_effect.verify(self.dir, secrets.token_bytes(32))

    def test_k20_missing_record_is_an_observation_failure(self):
        with self.assertRaises(pilot_effect.ChainError):
            pilot_effect.verify(self.dir, self.key)
        self.start()
        self.stop()
        os.chmod(self.dir, 0o700)
        (self.dir / "audit.jsonl").unlink()
        with self.assertRaises(pilot_effect.ChainError):
            pilot_effect.verify(self.dir, self.key)

    def test_daemon_unreachable_is_not_an_approval(self):
        self.daemon.decide(self.daemon.ask(LID), "once")
        self.daemon.close()
        self.start()
        self.assertEqual(self.publish(), 5)
        self.assertEqual(self.effects(), [])
        self.daemon = FakeDaemon()  # tearDown の close 用


class ForeignDecisionCase(unittest.TestCase):
    """driver の照合（`foreign_decisions`）: 承認は driver が人として決めたものだけ（K17。レビューの指摘: worker が
    人の token を読んで `POST /approvals/{id}/decide` を呼ぶと broker は `human:` の決定として数えてしまう）。"""

    Q1 = "pilot-effect:C06:o1:publish を公開してよいですか"
    Q2 = "pilot-effect:C06:o2:publish を公開してよいですか"

    def rows(self, d1="once", d2="denied"):
        return [{"id": "11", "task_id": "T1", "question": self.Q1, "decision": d1},
                {"id": "12", "task_id": "T2", "question": self.Q2, "decision": d2},
                {"id": "13", "task_id": "T3", "question": "別の質問", "decision": "once"}]

    def answers(self, extra=()):
        return [{"task_id": "T1", "question": self.Q1, "by": "human:pilot"},
                {"task_id": "T2", "question": self.Q2, "by": "human:pilot"},
                {"task_id": "T3", "question": "別の質問", "by": "human:pilot"}, *extra]

    def test_decisions_made_by_the_driver_match(self):
        self.assertEqual(pilot_effect.foreign_decisions(self.rows(), self.answers(), {"11": "once", "12": "denied"}), [])

    def test_a_decision_the_worker_made_with_a_found_token_is_foreign(self):
        # driver は o2 をまだ決めていないのに、daemon には human:pilot の once がある（worker が token で決めた）。
        found = pilot_effect.foreign_decisions(self.rows(d2="once"), self.answers(), {"11": "once"})
        self.assertEqual(len(found), 2, found)
        self.assertIn("approval 12 was decided (once) by someone other than the driver", found[0])
        self.assertIn("task T2 has a human answer", found[1])

    def test_a_changed_decision_is_foreign(self):
        found = pilot_effect.foreign_decisions(self.rows(d2="once"), self.answers(), {"11": "once", "12": "denied"})
        self.assertEqual(found, ["approval 12 is once in the daemon but the driver decided denied"])

    def test_a_human_answer_the_driver_did_not_give_is_foreign(self):
        extra = [{"task_id": "T9", "question": self.Q2, "by": "human:pilot"},
                 {"task_id": "T8", "question": self.Q2, "by": "operator:pilot"}]  # 人でない主体は broker が数えない
        found = pilot_effect.foreign_decisions(self.rows(), self.answers(extra), {"11": "once", "12": "denied"})
        self.assertEqual(len(found), 1, found)
        self.assertIn("task T9 has a human answer", found[0])

    def test_a_decision_missing_from_the_daemon_is_reported(self):
        found = pilot_effect.foreign_decisions(self.rows(d2=None), self.answers()[:1], {"11": "once", "12": "denied"})
        self.assertEqual(found, ["the driver decided approval 12 (denied) but the daemon has no such decision"])

    def test_the_broker_alone_would_count_a_forged_human_decision(self):
        # 照合が要る理由: broker は daemon の決定の主体（human:）しか見ないので、偽造の once も承認になる。
        daemon = FakeDaemon()
        try:
            daemon.decide(daemon.ask(LID), "once")
            api = pilot_effect.DaemonApprovals(daemon.base, "t0ken", PROJECT)
            self.assertEqual(api.decision(LID)[0], "approved")
            approvals = [{**a, "id": str(a["id"])} for a in daemon.approvals]
            answered = [{"task_id": t, **e} for t, evs in daemon.answers.items() for e in evs]
            self.assertTrue(pilot_effect.foreign_decisions(approvals, answered, {}))
        finally:
            daemon.close()


if __name__ == "__main__":
    unittest.main()
