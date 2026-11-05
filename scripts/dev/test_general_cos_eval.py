"""Synthetic collector tests; these observations are NOT model measurements."""
import copy
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from general_cos_eval import METRICS, RUBRIC, evaluate


def observation(arm="new", kind="consult", **changes):
    row = dict(schema="celeris.general-cos-eval/1", tier="fixture", arm=arm,
               case="c1", repeat=1, kind=kind, evidence="synthetic:test", policy=arm,
               model="fixture", harness="fixture", account="fixture", dataset="p1h-v1",
               tasks_created=int(kind == "delegate"), authority_preserved=True,
               runs=1, tool_calls=0, wall_seconds=2, tokens=10, cost_usd=None)
    row.update({key: True for key in RUBRIC[kind]})
    row.update(changes)
    return row


class EvaluationTests(unittest.TestCase):
    def test_pair_and_all_metrics(self):
        rows = [observation("old", tasks_created=1, runs=2, tool_calls=3), observation()]
        report = evaluate(rows)["fixture"]
        self.assertEqual(report["paired"]["old"]["success_rate"], 0)
        self.assertEqual(report["paired"]["old"]["waste_tasks"], 1)
        self.assertEqual(report["paired"]["new"]["success_rate"], 1)
        self.assertEqual(report["paired"]["old"]["runs"], 2)
        self.assertEqual(report["paired"]["old"]["tool_calls"], 3)
        self.assertEqual(report["paired"]["new"]["wall_seconds"], 2)
        self.assertEqual(report["paired"]["new"]["tokens"], 10)
        self.assertIsNone(report["paired"]["new"]["cost_usd"])

    def test_no_observation_is_not_zero(self):
        report = evaluate([observation()])
        self.assertIsNone(report["llm"]["observed"]["new"]["success_rate"])
        self.assertIsNone(report["fixture"]["paired"]["new"]["waste_tasks"])
        self.assertEqual(report["fixture"]["unpaired"][0]["missing"], "old")

    def test_rubric_failures(self):
        for kind, keys in RUBRIC.items():
            for key in ("authority_preserved", *keys):
                with self.subTest(kind=kind, key=key):
                    row = observation(kind=kind, **{key: False})
                    self.assertEqual(evaluate([row])["fixture"]["observed"]["new"]["success_rate"], 0)
        for count in (0, 2):
            self.assertEqual(evaluate([observation(kind="delegate", tasks_created=count)])["fixture"]["observed"]["new"]["success_rate"], 0)

    def test_same_conditions_required(self):
        for key in ("model", "harness", "account", "dataset", "kind"):
            with self.subTest(key=key), self.assertRaises(ValueError):
                new = observation(kind="read") if key == "kind" else observation(**{key: "different"})
                evaluate([observation("old"), new])

    def test_duplicate_and_invalid_data_rejected(self):
        with self.assertRaises(ValueError):
            evaluate([observation(), observation()])
        for key, value in (("analysis", "yes"), ("authority_preserved", 1),
                           ("tokens", -1), ("runs", True), ("tokens", 1.5),
                           ("wall_seconds", float("nan")), ("evidence", ""),
                           ("tasks_created", True), ("repeat", 0)):
            with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                evaluate([observation(**{key: value})])

    def test_tiers_never_pair(self):
        report = evaluate([observation("old"), observation(tier="llm")])
        for tier in ("fixture", "llm"):
            self.assertEqual(len(report[tier]["unpaired"]), 1)
            self.assertEqual(report[tier]["paired"]["new"]["observed"], 0)

    def test_partial_telemetry_does_not_make_a_total(self):
        for key in METRICS:
            rows = [observation(), observation(case="c2", **{key: None})]
            report = evaluate(rows)["fixture"]["observed"]["new"]
            self.assertIsNone(report[key])
            self.assertEqual(report["missing_telemetry"][key], 1 if key != "cost_usd" else 2)

    def test_case_corpus(self):
        path = Path(__file__).resolve().parents[2] / "tests/fixtures/general-cos-eval/cases.json"
        corpus = json.loads(path.read_text())
        self.assertEqual(corpus["dataset"], "p1h-v1")
        cases = corpus["cases"]
        self.assertEqual(len({case["id"] for case in cases}), 8)
        self.assertEqual({case["kind"] for case in cases}, set(RUBRIC))
        for case in cases:
            self.assertTrue(case["text"] and case["expected"])

    def test_cli_exit_and_json(self):
        script = Path(__file__).with_name("general_cos_eval.py")
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "observations.jsonl"
            for rows, expected_exit in (([], 0), ([observation()], 0), ([{}], 2)):
                path.write_text("".join(json.dumps(row) + "\n" for row in rows))
                result = subprocess.run([sys.executable, str(script), str(path)],
                                        capture_output=True, text=True, check=False)
                self.assertEqual(result.returncode, expected_exit, result.stderr)
                if expected_exit == 0:
                    self.assertIsNone(json.loads(result.stdout)["llm"]["paired"]["new"]["success_rate"])

    def test_input_is_not_mutated(self):
        rows = [observation("old"), observation()]
        before = copy.deepcopy(rows)
        evaluate(rows)
        self.assertEqual(rows, before)


if __name__ == "__main__":
    unittest.main()
