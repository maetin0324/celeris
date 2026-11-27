#!/usr/bin/env python3
"""r1_bench.py の試験。repo の root で: python3 -I agent-docs/progress/2026-11-27-strategy-v1/test_r1_bench.py

守る不変条件:
- 写像: root の終端の時点を点にし、terminal_children・outcome_reached・no_progress_streak を r1_rule の関数で出す。
  途中到達で streak が 0 に戻り、その後に終端になった root だけを数える（合成の facts）。
- 除外: 時刻の無い facts（celeris.long-horizon-facts/1 の実の形: statuses が文字列の列、outcomes の欄なし）と roots 0 件の facts は
  推測で補わず、理由の code と文を付けて除外する。achieved_at があっても補わない。
- F1: 到達で streak が戻らない streak の関数（到達を無視する偽の関数）を差し込むと F1 に当たる（否定対照）。正しい関数では当たらない。
- F2: 全到達の trial のうち到達より前に K = 3 で発火した trial が 2 件で当たり、1 件では当たらない。
- inconclusive: 途中到達のある trial が 3 件未満なら、F2 に当たっても falsified にしない。
- 規則は r1_rule.py から読む（r1_bench.py に streak_after_reach・escalate の定義が無い）。
- verify は k の書き換え・除外の理由の欠け・欄の欠けで落ちる。実の 44 trial で run の出力と byte 一致する。
既存の試験では検出できない理由: r1_bench.py は新しい script で、test_r1_rule.py は子 0 の slice の写像だけを試し、
long-horizon の facts の写像・除外・F1・F2 を見ない。
"""
from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

sys.dont_write_bytecode = True
HERE = Path(__file__).resolve().parent
_spec = importlib.util.spec_from_file_location("r1_bench", HERE / "r1_bench.py")
rb = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(rb)


def root(lid: str, outcome: str, *statuses) -> dict:
    """statuses は (status, at) か status の文字列。"""
    sts = [{"status": s[0], "at": s[1]} if isinstance(s, tuple) else s for s in statuses]
    return {"logical_id": lid, "outcome": outcome, "statuses": sts}


def timed(roots: list[dict], outcomes: list[tuple[str, bool, int | None]], achieved_at=None) -> dict:
    return {
        "roots": roots,
        "outcomes": [{"id": i, "required": req, "reached_at": at} for i, req, at in outcomes],
        "achieved_at": achieved_at,
    }


def done(lid, outcome, at):
    return root(lid, outcome, ("ready", 0), ("running", 1), ("done", at))


def failed(lid, outcome, at):
    return root(lid, outcome, ("ready", 0), ("running", 1), ("failed", at))


def midway_trial() -> dict:
    """o1 が 100 で到達（途中）、その後 o2 の root が 3 回失敗し、400 で o2 が到達。"""
    return timed(
        [done("o1:1", "o1", 100), failed("o2:1", "o2", 200), failed("o2:2", "o2", 250), failed("o2:3", "o2", 300), done("o2:4", "o2", 400)],
        [("o1", True, 100), ("o2", True, 400)],
    )


def early_fire_trial() -> dict:
    """到達が無いまま 3 root が終端になり、最後に 1 件の Outcome が到達する（到達の前に K = 3 で発火）。"""
    return timed(
        [failed("o1:1", "o1", 100), failed("o1:2", "o1", 200), failed("o1:3", "o1", 300), done("o1:4", "o1", 400)],
        [("o1", True, 400)],
    )


def row(name: str, facts: dict, arm: str = "new") -> dict:
    r = {"set": "synthetic", "trial": name, "case": "C00", "arm": arm, "seed": 1, "facts_sha256": "0" * 64}
    r.update(rb.judge_trial(facts))
    return r


class Mapping(unittest.TestCase):
    def test_points_and_streak_reset_on_midway_reach(self):
        t = rb.judge_trial(midway_trial())
        self.assertTrue(t["included"])
        got = [(p["root"], p["terminal_children"], p["outcomes_reached"], p["outcome_reached"], p["no_progress_streak"]) for p in t["points"]]
        self.assertEqual(
            got,
            [
                ("o1:1", 1, 1, False, 0),  # 到達と同じ時刻の root は到達の後に数えない
                ("o2:1", 2, 1, False, 1),
                ("o2:2", 3, 1, False, 2),
                ("o2:3", 4, 1, False, 3),
                ("o2:4", 5, 2, True, 0),
            ],
        )
        self.assertEqual(t["fired"]["3"], [4])
        self.assertEqual(t["fired"]["2"], [3, 4])
        self.assertEqual(t["reach"]["midway"], [{"at": 100, "streak_right_after": 0}])
        self.assertEqual(t["reach"]["full_reach_at"], 400)
        self.assertEqual(t["f1_violations"], [])
        self.assertEqual(t["fired_before_full_reach"]["3"], [4])

    def test_tie_order_and_unrequired_outcome(self):
        # 同じ時刻の root は logical_id の順。必要でない Outcome（o2）の到達は数えない。
        f = timed([done("o1:1", "o1", 50), root("o2:1", "o2", ("ready", 0), ("cancelled", 50))], [("o1", True, 50), ("o2", False, None)])
        t = rb.judge_trial(f)
        self.assertEqual([p["root"] for p in t["points"]], ["o1:1", "o2:1"])
        self.assertEqual([p["outcomes"] for p in t["points"]], [1, 1])
        self.assertTrue(t["points"][0]["outcome_reached"])
        self.assertEqual(t["reach"]["midway"], [])

    def test_rule_functions_come_from_r1_rule(self):
        src = (HERE / "r1_bench.py").read_text(encoding="utf-8")
        self.assertNotIn("def streak_after_reach", src)
        self.assertNotIn("def escalate", src)
        self.assertEqual(Path(rb.r1.__file__).resolve(), (HERE / "r1_rule.py").resolve())


class Exclusion(unittest.TestCase):
    def test_real_shape_without_times_is_excluded_even_with_achieved_at(self):
        f = {"roots": [root("o1:1", "o1", "ready", "running", "done")], "achieved_at": 3900}
        t = rb.judge_trial(f)
        self.assertFalse(t["included"])
        self.assertEqual([e["code"] for e in t["exclusion"]], ["root_terminal_time_missing", "outcome_reach_time_missing"])
        self.assertTrue(all(e["reason"] for e in t["exclusion"]))
        self.assertEqual(t["points"], [])
        self.assertEqual(t["evidence"]["achieved_at"], 3900)

    def test_no_roots_and_no_terminal_root(self):
        t = rb.judge_trial({"roots": [], "achieved_at": None})
        self.assertEqual([e["code"] for e in t["exclusion"]], ["no_roots", "outcome_reach_time_missing"])
        f = timed([root("o1:1", "o1", ("ready", 0), ("running", 5))], [("o1", True, None)])
        self.assertEqual([e["code"] for e in rb.judge_trial(f)["exclusion"]], ["no_terminal_root"])

    def test_one_root_without_time_excludes_the_trial(self):
        f = timed([done("o1:1", "o1", 100), root("o2:1", "o2", "ready", "done")], [("o1", True, 100), ("o2", True, None)])
        self.assertEqual([e["code"] for e in rb.judge_trial(f)["exclusion"]], ["root_terminal_time_missing"])

    def test_no_required_outcome(self):
        f = timed([done("o1:1", "o1", 100)], [("o1", False, 100)])
        self.assertEqual([e["code"] for e in rb.judge_trial(f)["exclusion"]], ["no_required_outcome"])


class Judgement(unittest.TestCase):
    def three_midway(self) -> list[dict]:
        return [row(f"m{i}", midway_trial()) for i in range(3)]

    def test_correct_streak_does_not_hit_f1(self):
        j = rb.judge(self.three_midway())
        self.assertFalse(j["f1"]["hit"])
        self.assertFalse(j["inconclusive"])

    def test_f1_detects_a_streak_that_ignores_reach(self):
        orig = rb.r1.streak_after_reach
        try:
            rb.r1.streak_after_reach = lambda done_ts, reach_ts: len(done_ts)  # 到達を無視する偽の関数
            j = rb.judge(self.three_midway())
        finally:
            rb.r1.streak_after_reach = orig
        self.assertTrue(j["f1"]["hit"])
        self.assertEqual(len(j["f1"]["violations"]), 3)
        self.assertTrue(j["falsified"])

    def test_f2_needs_two_trials(self):
        # 途中到達の 3 trial は、全到達の前に 1 点ずつ発火する（o2 の失敗 3 回の後）。これだけで F2 に当たる。
        j = rb.judge(self.three_midway())
        self.assertEqual(j["f2"]["by_k"]["3"]["count"], 3)
        self.assertTrue(j["f2"]["hit"])
        self.assertTrue(j["falsified"])
        # 早い発火が 1 trial だけで、途中到達の 3 trial が到達の前に発火しない場合は当たらない。
        quiet = timed(
            [done("o1:1", "o1", 100), failed("o2:1", "o2", 200), done("o2:2", "o2", 300)],
            [("o1", True, 100), ("o2", True, 300)],
        )
        rows = [row(f"q{i}", quiet) for i in range(3)] + [row("e", early_fire_trial())]
        j = rb.judge(rows)
        self.assertEqual(j["f2"]["by_k"]["3"]["count"], 1)
        self.assertFalse(j["f2"]["hit"])
        self.assertFalse(j["inconclusive"])
        self.assertFalse(j["falsified"])
        rows.append(row("e2", early_fire_trial()))
        self.assertTrue(rb.judge(rows)["falsified"])

    def test_inconclusive_below_three_midway_trials_is_not_falsified(self):
        rows = [row("e1", early_fire_trial()), row("e2", early_fire_trial()), row("m", midway_trial())]
        j = rb.judge(rows)
        self.assertTrue(j["f2"]["hit"])
        self.assertEqual(j["counts"]["midway_trials"], 1)
        self.assertTrue(j["inconclusive"])
        self.assertFalse(j["falsified"])

    def test_arm_false_positives(self):
        rows = [row("e1", early_fire_trial(), arm="old.passive"), row("m", midway_trial(), arm="new")]
        a = rb.arms(rows)
        self.assertEqual(a["old.passive"]["by_k"]["3"], {"false_positive_trials": 1, "false_positive_points": 1})
        self.assertEqual(a["new"]["by_k"]["3"], {"false_positive_trials": 1, "false_positive_points": 1})
        self.assertEqual(a["new"]["by_k"]["4"], {"false_positive_trials": 0, "false_positive_points": 0})


class Verify(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="r1-bench-test-"))
        self.orig = rb.RESULT

    def tearDown(self):
        rb.RESULT = self.orig
        shutil.rmtree(self.tmp, ignore_errors=True)

    def run_verify(self, mutate=None) -> int:
        doc = json.loads(self.orig.read_bytes())
        if mutate:
            mutate(doc)
        path = self.tmp / "r1-bench.json"
        path.write_bytes(rb.render(doc))
        rb.RESULT = path
        with contextlib.redirect_stdout(io.StringIO()):
            try:
                return rb.cmd_verify()
            except ValueError:
                return 1

    def test_committed_result_matches(self):
        self.assertEqual(self.run_verify(), 0)

    def test_real_trials_all_have_reasons_when_excluded(self):
        doc = json.loads(self.orig.read_bytes())
        self.assertEqual(len(doc["trials"]), 44)
        self.assertEqual(doc["llm_calls"], 0)
        for t in doc["trials"]:
            if not t["included"]:
                self.assertTrue(t["exclusion"], t["trial"])

    def test_k_change_fails(self):
        self.assertEqual(self.run_verify(lambda d: d["rule"].__setitem__("k", 2)), 1)

    def test_missing_reason_fails(self):
        def drop(d):
            d["trials"][0]["exclusion"] = []
        self.assertEqual(self.run_verify(drop), 1)

    def test_missing_field_fails(self):
        self.assertEqual(self.run_verify(lambda d: d.pop("inconclusive")), 1)


if __name__ == "__main__":
    unittest.main(verbosity=1)
