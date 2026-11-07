#!/usr/bin/env python3
"""pilot_drain の単体試験（ADR 2026-11-06-cos-run-launch-accounting-and-drain D3）。注入した時計と sleep だけを使い、実時間は待たない。

    python3 -I scripts/dev/long-horizon-pilot/test_pilot_drain.py -v
"""
import pathlib
import sqlite3
import sys
import unittest

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import pilot_drain as d  # noqa: E402


class Clock:
    """sleep で進む時計。"""

    def __init__(self, t=1000.0):
        self.t = t

    def now(self):
        return self.t

    def sleep(self, s):
        self.t += s


def row(run="r1", finished=None, usage=None, launched="2026-01-01T00:00:00Z", reason=None, tracked=1):
    return {"run_id": run, "state": "running", "reason": None, "started_at": "2026-01-01T00:00:00Z",
            "finished_at": finished, "usage_json": usage, "launched_at": launched, "prelaunch_reason": reason,
            "launch_tracked": tracked}


class LaunchState(unittest.TestCase):
    def test_states(self):
        self.assertEqual(d.launch_state(row()), "launched")
        self.assertEqual(d.launch_state(row(launched=None, finished="x", reason="slot_wait_expired")), "not_launched")
        for bad in (dict(launched=None, finished="x", reason=None), dict(launched=None, finished="x", reason="zzz"),
                    dict(launched=None, finished="x", reason="route_unavailable", tracked=0),
                    dict(launched=None, finished=None, reason="route_unavailable")):
            self.assertEqual(d.launch_state(row(**bad)), "unrecorded", bad)

    def test_settled_needs_usage_unless_not_launched(self):
        self.assertFalse(d.settled(row(finished="x")))  # 終わったが usage がまだ（永続化の遅れ）
        self.assertFalse(d.settled(row(usage="{}")))  # usage だけで終わっていない
        self.assertTrue(d.settled(row(finished="x", usage="{}")))
        self.assertTrue(d.settled(row(launched=None, finished="x", reason="route_unavailable")))


class Drain(unittest.TestCase):
    def run_drain(self, script, deadline, settle=2.0, start=1000.0):
        """script(t) -> その時刻の未完了 run の一覧。"""
        clk = Clock(start)
        res = d.drain(lambda: script(clk.t), clk.now, clk.sleep, deadline, settle_secs=settle)
        return res, clk

    def test_nothing_pending_returns_after_the_settle_window(self):
        res, clk = self.run_drain(lambda t: [], deadline=1100)
        self.assertEqual((res["timed_out"], res["waited_secs"]), ([], 2.0))

    def test_slow_cos_finishing_just_before_the_deadline_is_waited_for(self):
        res, clk = self.run_drain(lambda t: [row()] if t < 1089 else [], deadline=1090, settle=0.0)
        self.assertEqual(res["timed_out"], [])
        self.assertGreaterEqual(clk.t, 1089)

    def test_persistence_delay_after_finish_is_waited_for(self):
        # finished_at は 1030、usage は 1045 に書かれる。drain はその間 pending のまま待つ。
        pend = lambda t: [row(finished="x")] if t < 1045 else []
        res, _ = self.run_drain(pend, deadline=1100)
        self.assertEqual(res["timed_out"], [])
        self.assertGreaterEqual(res["waited_secs"], 45)

    def test_deadline_boundary_times_out_and_lists_the_run(self):
        res, clk = self.run_drain(lambda t: [row("slow", finished="x")], deadline=1010)
        self.assertEqual(clk.t, 1010)  # 期限を超えて sleep しない
        self.assertEqual([(x["run_id"], x["finished"], x["usage"]) for x in res["timed_out"]],
                         [("slow", True, False)])

    def test_finishing_exactly_at_the_deadline_is_a_timeout_only_if_still_pending(self):
        res, _ = self.run_drain(lambda t: [row()] if t < 1010 else [], deadline=1010, settle=0.0)
        self.assertEqual(res["timed_out"], [])

    def test_a_run_born_during_the_settle_window_is_waited_for(self):
        # t=1001 に新しい run が現れ 1020 に終わる。settle 2 秒の間に現れた run を見落として SIGTERM を送らない。
        pend = lambda t: [row("new")] if 1001 <= t < 1020 else []
        res, clk = self.run_drain(pend, deadline=1100)
        self.assertEqual(res["timed_out"], [])
        self.assertGreaterEqual(clk.t, 1022)  # 1020 の後に settle 2 秒

    def test_run_born_late_and_still_open_at_the_deadline_times_out(self):
        res, _ = self.run_drain(lambda t: [row("late")] if t >= 1005 else [], deadline=1020, settle=10)
        self.assertEqual([x["run_id"] for x in res["timed_out"]], ["late"])

    def test_deadline_already_passed_with_pending_runs_times_out_at_once(self):
        res, clk = self.run_drain(lambda t: [row()], deadline=900)
        self.assertEqual((len(res["timed_out"]), res["waited_secs"], res["rounds"]), (1, 0.0, 1))


class Deadline(unittest.TestCase):
    def test_trial_cap_only(self):
        self.assertEqual(d.compute_deadline(100, 300), 400)

    def test_approval_total_is_not_reset_per_trial(self):
        # 承認の総量 1000 秒。1 本目が 700 秒使ったので、2 本目（上限 600）は残り 300 秒で打ち切る。
        approval_start, total = 5000, 1000
        second_start = approval_start + 700
        self.assertEqual(d.compute_deadline(second_start, 600, approval_start, total), 6000)
        self.assertEqual(d.compute_deadline(approval_start, 600, approval_start, total), 5600)

    def test_expiry_caps_the_deadline(self):
        self.assertEqual(d.compute_deadline(100, 300, 100, 1000, expires_at=250), 250)

    def test_exhausted_total_gives_a_past_deadline(self):
        self.assertLess(d.compute_deadline(7000, 600, 5000, 1000), 7000)


class ReadChatRuns(unittest.TestCase):
    def test_reads_all_rows_including_unstarted_and_survives_old_schema(self):
        c = sqlite3.connect(":memory:")
        c.executescript("CREATE TABLE chat_threads (id TEXT, project_id TEXT);"
                        "CREATE TABLE chat_runs (run_id TEXT, thread_id TEXT, state TEXT, reason TEXT, started_at TEXT, "
                        "finished_at TEXT, usage_json TEXT);"
                        "INSERT INTO chat_threads VALUES ('H1','P1'),('H2','P2');"
                        "INSERT INTO chat_runs VALUES ('a','H1','queued',NULL,NULL,NULL,NULL),"
                        "('b','H2','running',NULL,'t',NULL,NULL);")
        rows = d.read_chat_runs(c, "P1")
        self.assertEqual([r["run_id"] for r in rows], ["a"])
        self.assertEqual(len(d.unsettled(rows)), 1)  # 旧い DB の行は起動前と言い切れず、終わるまで待つ


if __name__ == "__main__":
    unittest.main()
