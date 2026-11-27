#!/usr/bin/env python3
"""r1_rule.py の試験。repo の root で: python3 -I agent-docs/progress/2026-11-27-strategy-v1/test_r1_rule.py

守る不変条件:
- 規則の関数（evaluate）は slice の dir の中の file だけを開く。answer key（子 0 の replay/answer_key.json）と研究ケースの時点の表
  （research/research-case.md）を開かない。sys.addaudithook の open の event で、19 点の実の slice について確かめる。
  answer key を読む偽の規則を同じ記録器に当て、記録器が読みを検出すること（否定対照）も確かめる。
- compute は 19 点の発火をすべて出した後にだけ answer key と時点の表を開く（採点だけが読む）。
- 規則の定義（outcome_reached・terminal_children・no_progress_streak・escalate）を合成の slice で確かめる。
既存の試験では検出できない理由: r1_rule.py は新しい script で、子 0 の replay.py verify と research_case.py verify は slice の漏れを
見るだけで、規則の関数が何を読むかを見ない。
"""
from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import os
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
_spec = importlib.util.spec_from_file_location("r1_rule", HERE / "r1_rule.py")
r1 = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(r1)


class OpenRecorder:
    """audit hook の open の event を記録する。hook は外せないので active の間だけ記録する。"""

    def __init__(self):
        self.active = False
        self.log: list[tuple[str, str]] = []
        sys.addaudithook(self._hook)

    def _hook(self, event, args):
        if self.active and event == "open" and isinstance(args[0], (str, bytes, os.PathLike)):
            path = os.fsdecode(args[0])
            self.log.append(("open", str(Path(path).resolve())))

    def mark(self, label: str):
        if self.active:
            self.log.append(("mark", label))

    def run(self, fn, *a):
        self.log = []
        self.active = True
        try:
            return fn(*a)
        finally:
            self.active = False

    def opened(self) -> list[str]:
        return [p for kind, p in self.log if kind == "open"]


REC = OpenRecorder()
FORBIDDEN = {str(r1.ANSWER_KEY.resolve()), str(r1.RESEARCH_TABLE.resolve())}


def outside(paths: list[str], root: Path) -> list[str]:
    root = str(root.resolve())
    return [p for p in paths if not (p == root or p.startswith(root + os.sep))]


class RealSlices(unittest.TestCase):
    """19 点の実の slice（子 0 の build と sections で作る）に規則を当てる。"""

    @classmethod
    def setUpClass(cls):
        os.chdir(r1.REPO)
        cls.tmp = Path(tempfile.mkdtemp(prefix="r1-test-", dir=os.environ.get("TMPDIR")))
        cls.replay_dirs = r1.build_replay(cls.tmp / "replay")
        cls.research_dirs = r1.build_research(cls.tmp / "research") if Path(r1.rc.SRC).is_file() else []

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(cls.tmp, ignore_errors=True)

    def test_rule_opens_only_its_slice_dir(self):
        if not self.research_dirs:
            self.skipTest(f"{r1.rc.SRC} が無い")
        dirs = self.replay_dirs + self.research_dirs
        self.assertEqual(len(dirs), 19)
        r1.evaluate(dirs[0])  # 遅延の import などの open を記録から外す
        for d in dirs:
            REC.run(r1.evaluate, d)
            opened = REC.opened()
            self.assertTrue(opened, f"{d.name}: open が 1 件も記録されない（記録器が動いていない）")
            self.assertEqual(outside(opened, d), [], f"{d.name}: slice の dir の外を開いた")
            self.assertFalse(FORBIDDEN & set(opened), f"{d.name}: answer key か時点の表を開いた")

    def test_negative_control_recorder_detects_answer_key_read(self):
        """answer key を読む偽の規則は、同じ記録器で検出される（試験が空振りでないこと）。"""
        d = self.replay_dirs[0]

        def cheating_rule(slice_dir):
            m = r1.evaluate(slice_dir)
            key = json.loads(r1.ANSWER_KEY.read_text(encoding="utf-8"))
            m["escalate"]["3"] = key["checkpoints"][m["id"]]["ideal_verdict"] == "off_track"
            return m

        REC.run(cheating_rule, d)
        opened = REC.opened()
        self.assertIn(str(r1.ANSWER_KEY.resolve()), opened)
        self.assertNotEqual(outside(opened, d), [])

    def test_compute_reads_answer_key_only_after_all_firings(self):
        if not self.research_dirs:
            self.skipTest(f"{r1.rc.SRC} が無い")
        orig = r1.evaluate

        def logged(slice_dir):
            REC.mark("evaluate")
            return orig(slice_dir)

        r1.evaluate = logged
        try:
            result = REC.run(r1.compute)
        finally:
            r1.evaluate = orig
        log = REC.log
        evals = [i for i, (k, v) in enumerate(log) if k == "mark" and v == "evaluate"]
        reads = [i for i, (k, v) in enumerate(log) if k == "open" and v in FORBIDDEN]
        self.assertEqual(len(evals), 19)
        self.assertEqual({log[i][1] for i in reads}, FORBIDDEN, "採点が answer key と時点の表を読んでいない")
        self.assertLess(max(evals), min(reads), "発火をすべて出す前に answer key か時点の表を開いた")
        self.assertEqual(result["llm_calls"], 0)

    def test_result_file_matches_recompute(self):
        if not self.research_dirs:
            self.skipTest(f"{r1.rc.SRC} が無い")
        self.assertEqual(r1.render(r1.compute()), r1.RESULT.read_bytes())


def write_replay_slice(root: Path, children_done, events, goal=True) -> Path:
    d = root / "S"
    d.mkdir(parents=True)
    meta = {"id": "S", "cutoff": "2026-11-18T00:00:00.000000000Z", "children_done": children_done}
    (d / "slice.json").write_text(json.dumps(meta), encoding="utf-8")
    (d / "events.jsonl").write_text("".join(json.dumps(e) + "\n" for e in events), encoding="utf-8")
    if goal:
        (d / "goal.md").write_text("# goal\n", encoding="utf-8")
    return d


def merged(task, hh):
    return {"ts": f"2026-11-17T{hh:02d}:00:00.000000000Z", "task": task, "seq": hh, "type": "integration_merged"}


class SyntheticRule(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="r1-syn-", dir=os.environ.get("TMPDIR")))

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_no_reach_record_counts_all_children_and_excludes_parent(self):
        ev = [merged("A", 1), merged("B", 2), merged("C", 3), merged("P", 4)]
        m = r1.evaluate(write_replay_slice(self.tmp, ["A", "B", "C", "P"], ev))
        self.assertEqual((m["outcomes"], m["outcomes_reached"], m["outcome_reached"]), (1, 0, False))
        self.assertEqual((m["terminal_children"], m["no_progress_streak"]), (3, 3))
        self.assertEqual(m["escalate"], {"2": True, "3": True, "4": False})

    def test_reach_record_sets_outcome_reached_and_resets_streak(self):
        reach = {"ts": "2026-11-17T02:30:00.000000000Z", "task": "P", "seq": 9, "type": "reviewer_passed"}
        ev = [merged("A", 1), merged("B", 2), reach, merged("C", 3)]
        m = r1.evaluate(write_replay_slice(self.tmp, ["A", "B", "C"], ev))
        self.assertTrue(m["outcome_reached"])
        self.assertEqual((m["terminal_children"], m["no_progress_streak"]), (3, 1))
        self.assertEqual(m["escalate"], {"2": False, "3": False, "4": False})

    def test_streak_below_k_blocks_escalate_even_when_outcome_not_reached(self):
        self.assertFalse(r1.escalate({"outcome_reached": False, "terminal_children": 5, "no_progress_streak": 2}, 3))
        self.assertTrue(r1.escalate({"outcome_reached": False, "terminal_children": 3, "no_progress_streak": 3}, 3))
        self.assertFalse(r1.escalate({"outcome_reached": True, "terminal_children": 9, "no_progress_streak": 9}, 3))

    def test_missing_outcome_record_is_not_reached(self):
        m = r1.evaluate(write_replay_slice(self.tmp, [], [], goal=False))
        self.assertEqual((m["outcomes"], m["outcome_reached"]), (0, False))

    def test_research_section_needs_job_and_ratio(self):
        d = self.tmp / "11.17"
        d.mkdir()
        text = ("### 11.15 job 50001 の結果\n\n停止 0/8。\n\n"
                "### 11.16 計画だけ\n\n次は 50002 を投げる。\n\n"
                "### 11.17 比だけ\n\n1 / 4 の core。\n")
        (d / r1.RESULTS_FILE).write_text(text, encoding="utf-8")
        (d / "slice.json").write_text(json.dumps({"kind": r1.RESEARCH_KIND, "point": "11.17"}), encoding="utf-8")
        m = r1.evaluate(d)
        self.assertEqual(m["sections"], ["11.15", "11.16", "11.17"])
        self.assertEqual(m["sections_reported"], ["11.15"])
        self.assertEqual((m["terminal_children"], m["no_progress_streak"], m["outcome_reached"]), (1, 1, False))

    def test_unknown_slice_shape_is_an_error(self):
        d = self.tmp / "X"
        d.mkdir()
        (d / "slice.json").write_text("{}", encoding="utf-8")
        with self.assertRaises(ValueError):
            r1.evaluate(d)


class VerifyFields(unittest.TestCase):
    def test_missing_field_and_wrong_k_are_reported(self):
        doc = json.loads(r1.RESULT.read_text(encoding="utf-8"))
        self.assertEqual(r1.missing_fields(doc), [])
        del doc["falsified"]
        doc["replay"]["points"][0]["escalate"].pop("3")
        bad = r1.missing_fields(doc)
        self.assertIn("top.falsified", bad)
        self.assertTrue(any(b.endswith(".escalate.3") for b in bad))

    def test_verify_fails_on_tampered_result(self):
        if not Path(r1.rc.SRC).is_file():
            self.skipTest(f"{r1.rc.SRC} が無い")
        tmp = Path(tempfile.mkdtemp(prefix="r1-verify-", dir=os.environ.get("TMPDIR")))
        orig = r1.RESULT
        try:
            doc = json.loads(orig.read_text(encoding="utf-8"))
            doc["rule"]["k"] = 2
            bad = tmp / "r1-result.json"
            bad.write_bytes(r1.render(doc))
            r1.RESULT = bad
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                self.assertEqual(r1.cmd_verify(), 1)
            self.assertIn("FAIL rule の k=2", out.getvalue())
        finally:
            r1.RESULT = orig
            shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    unittest.main()
