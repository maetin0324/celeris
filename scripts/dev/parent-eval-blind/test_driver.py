#!/usr/bin/env python3
"""対照 pilot の driver（parent-eval-blind.sh・driver.py）の単体試験（ADR 2026-11-24-strategy-pilot-ablation D1・D3）。

daemon も cargo も LLM も使わない。引数と承認の検査は cargo の build より前に終わるので、sh をそのまま起こして exit code を見る。

    python3 -I scripts/dev/parent-eval-blind/test_driver.py -v

守る不変条件:
- 方式の引数は a・b・c だけを受け、時点は事例 replay の C-mid・D-done・F-done だけを受ける（他は exit 2）。
- `--harness config` は Celeris の worker の run の中（CELERIS_RUN_ID がある）では引数の検査より前に exit 2。
- 承認の scope.arms・scope.cases に無い方式・時点は exit 2（pilot_gate.py check-approval に方式の名をそのまま渡す）。
- output.json は verdict・next（Rust の検査の後の effective）・claims（根拠の参照つき）・usage を持つ。方式 a は LLM 0（usage 無し）。
- 方式 c の評価は版 v1 の段の出力（--strategy-dir）を要り、版を書く段は事例の最初の時点より前の slice（replay は A-mid）だけで走る。
- stub は版を書く worker として `celeris.strategy/1` の文書を書いてその名（strategy/<id>/v1.json）を申告し、Critic として文書の
  candidates[].id ごとに判定を書く（申告しない成果物は basename の名で記録され、Critic が起きない）。
既存の試験で検出できない理由: driver と sh には試験が無く、子 6 の形（時点 E-mid・D-done、方式の名 parent-eval-blind 固定、
出力 <out>/<point>/evaluation.json）のままでは、方式 a・b・c の引数・承認の arms・出力の形を誰も確かめなかった。
"""
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
SH = HERE.parent / "parent-eval-blind.sh"
FIXTURE = HERE.parents[2] / "crates/task-api/tests/fixtures/parent_checkpoint_live/blind/replay"
sys.path.insert(0, str(HERE))
import driver  # noqa: E402

STUB = HERE / "stub_reviewer.py"
DRIVER = HERE / "driver.py"

BASE_ARGS = ["--case", "replay", "--points", "C-mid,D-done,F-done"]


def run_sh(args, env_extra=None, drop_run_id=True):
    env = {k: v for k, v in os.environ.items() if not (drop_run_id and k == "CELERIS_RUN_ID")}
    env.update(env_extra or {})
    return subprocess.run(["bash", str(SH), *args], capture_output=True, text=True, env=env, timeout=60)


def approval(arms, cases):
    return {
        "approval_id": "test", "approved_by": "human:test", "approved_at": "2026-11-24T00:00:00Z",
        "expires_at": "2999-01-01T00:00:00Z", "account": "acct", "model": "claude-sonnet-5-5", "harness": "claude_code",
        "scope": {"arms": arms, "cases": cases, "environment": "non-production"},
        "limits": {"max_usd": 25, "max_wall_secs": 600},
    }


class Args(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = pathlib.Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def test_config_inside_worker_run_is_refused_before_args(self):
        r = run_sh(["--harness", "config", "--bogus"], {"CELERIS_RUN_ID": "01RUN"}, drop_run_id=False)
        self.assertEqual(r.returncode, 2, r.stderr)
        self.assertIn("CELERIS_RUN_ID", r.stderr)

    def test_unknown_arm_is_refused(self):
        r = run_sh(["--harness", "stub", "--arms", "a,x", *BASE_ARGS, "--out", str(self.dir / "o")])
        self.assertEqual(r.returncode, 2, r.stderr)
        self.assertIn("unknown arm x", r.stderr)

    def _driver(self, *extra):
        args = ["--case", "replay", "--state", str(self.dir / "s"), "--out", str(self.dir / "o"), "--celeris", "x",
                "--script-deploy", "x", "--bundle", "x", "--seed-bin", "x", "--shadow", "x", "--harness", "stub", *extra]
        return subprocess.run([sys.executable, "-I", str(DRIVER), *args], capture_output=True, text=True, timeout=60)

    def test_arm_c_needs_the_strategy_stage_output(self):
        r = self._driver("--point", "C-mid", "--arm", "c")
        self.assertEqual(r.returncode, 1, r.stderr)
        self.assertIn("--arm c needs --strategy-dir", r.stderr)

    def test_strategy_stage_runs_only_on_the_slice_before_the_first_point(self):
        r = self._driver("--stage", "strategy-v1", "--point", "C-mid", "--arm", "c", "--skill", str(SH))
        self.assertEqual(r.returncode, 1, r.stderr)
        self.assertIn("--point A-mid", r.stderr)
        self.assertEqual(driver.STRATEGY_POINT["replay"], "A-mid")
        r = self._driver("--point", "C-mid", "--arm", "x")
        self.assertEqual(r.returncode, 1, r.stderr)
        self.assertIn("not an arm", r.stderr)

    def test_point_outside_the_case_is_refused(self):
        r = run_sh(["--harness", "stub", "--arms", "a", "--case", "replay", "--points", "E-mid", "--out",
                    str(self.dir / "o")])
        self.assertEqual(r.returncode, 2, r.stderr)
        self.assertIn("unknown point E-mid", r.stderr)

    def test_unknown_case_and_missing_arms_are_refused(self):
        r = run_sh(["--harness", "stub", "--arms", "a", "--case", "other", "--points", "C-mid", "--out",
                    str(self.dir / "o")])
        self.assertEqual(r.returncode, 2, r.stderr)
        r = run_sh(["--harness", "stub", *BASE_ARGS, "--out", str(self.dir / "o")])
        self.assertEqual(r.returncode, 2, r.stderr)

    def _config(self, arms, scope_arms, scope_cases):
        ap = self.dir / "approval.json"
        ap.write_text(json.dumps(approval(scope_arms, scope_cases)))
        toml = self.dir / "llm.toml"
        toml.write_text("# not read before the approval check\n")
        return run_sh(["--harness", "config", "--arms", arms, *BASE_ARGS, "--out", str(self.dir / "o"),
                       "--approval", str(ap), "--llm-config", str(toml)])

    def test_approval_without_the_arm_is_refused(self):
        r = self._config("a,b", ["a"], ["C-mid", "D-done", "F-done"])
        self.assertEqual(r.returncode, 2, r.stderr)
        self.assertIn("scope.arms does not cover b", r.stderr)

    def test_approval_without_the_point_is_refused(self):
        r = self._config("a,b", ["a", "b", "c"], ["C-mid", "D-done"])
        self.assertEqual(r.returncode, 2, r.stderr)
        self.assertIn("scope.cases does not cover F-done", r.stderr)

    def test_config_without_approval_is_refused(self):
        r = run_sh(["--harness", "config", "--arms", "a,b", *BASE_ARGS, "--out", str(self.dir / "o")])
        self.assertEqual(r.returncode, 2, r.stderr)
        self.assertIn("approval", r.stderr)


class Output(unittest.TestCase):
    def test_arm_b_output_has_verdict_next_claims_with_refs_and_usage(self):
        committed = json.loads((FIXTURE / "D-done/b/output.json").read_text())
        run = json.loads((FIXTURE / "D-done/b/run.json").read_text())
        out = driver.output_from_record(committed["record"], run["usage"], "replay", "D-done", "b")
        self.assertEqual(out, committed, "commit した子 6 の記録が output_from_record の形と違う")
        self.assertEqual((out["verdict"], out["next"]), ("at_risk", "ask_human"))
        self.assertEqual(out["usage"]["cost_usd"], run["usage"]["cost_usd"])
        kinds = {c["kind"] for c in out["claims"]}
        self.assertTrue({"measured", "learned", "refuted", "uncertainty"} <= kinds, kinds)
        self.assertTrue(all(c["statement"] for c in out["claims"]))
        with_refs = [c for c in out["claims"] if c["kind"] != "uncertainty"]
        self.assertTrue(with_refs and all(c["refs"] for c in with_refs), with_refs)
        self.assertTrue(all(r.get("kind") == "task" and r.get("task_id") for c in with_refs for r in c["refs"]))

    def test_arm_a_output_is_the_fixed_plan_without_llm(self):
        rows = [{"task": "A", "type": "reviewer_passed"}, {"task": "B", "type": "transitioned", "to": "running"}]
        decision = {"id": "d1", "effect_state": "suppressed", "proposal": {"kind": "evaluate"}}
        out = driver.output_fixed_plan(decision, rows, {"A": "TA", "B": "TB"}, "replay", "C-mid")
        self.assertIsNone(out["verdict"])
        self.assertEqual(out["next"], "continue")
        self.assertIsNone(out["usage"])
        self.assertEqual(out["claims"], [{"kind": "local_review", "statement": "子 A の局所の review が合格（史実の記録）",
                                          "refs": [{"kind": "task", "task_id": "TA"}]}])
        self.assertEqual(out["controller_decision"]["effect_state"], "suppressed")

    def test_suppressed_evaluate_picks_only_the_suppressed_evaluate_decision(self):
        rows = [("d0", json.dumps({"action": {"kind": "wait"}}), "suppressed"),
                ("d1", json.dumps({"action": {"kind": "evaluate"}}), "pending"),
                ("d2", json.dumps({"action": {"kind": "evaluate"}}), "suppressed")]
        self.assertEqual(driver.suppressed_evaluate(rows)["id"], "d2")
        self.assertIsNone(driver.suppressed_evaluate(rows[:2]))

    def test_tree_listing_excludes_git_and_hashes_files(self):
        with tempfile.TemporaryDirectory() as d:
            root = pathlib.Path(d)
            driver.git_seed({"goal.md": b"g", "events.jsonl": b"{}\n"}, root / "repo")
            files = driver.tree_listing(root / "repo")
        self.assertEqual([f["path"] for f in files], ["events.jsonl", "goal.md"])
        self.assertEqual(files[1]["bytes"], 1)
        self.assertEqual(len(files[1]["sha256"]), 64)

    def test_driver_cases_and_arms_match_the_shell(self):
        self.assertEqual(driver.CASES["replay"], ("C-mid", "D-done", "F-done"))
        self.assertEqual(driver.ARMS, ("a", "b", "c"))
        sh = SH.read_text()
        self.assertIn('replay) known_points="C-mid D-done F-done"', sh)


class Stub(unittest.TestCase):
    """stub の 3 つの役（版を書く worker・Critic を兼ねる final review・評価）の出力の形。"""

    def run_stub(self, work, objective, review=None):
        art = work / "artifacts"
        req = {"workspace": str(work), "artifacts_dir": str(art), "context": {"review": review},
               "task": {"objective": objective, "project_id": "P1"}}
        r = subprocess.run([sys.executable, "-I", str(STUB)], input=json.dumps(req), capture_output=True, text=True,
                           timeout=60)
        self.assertEqual(r.returncode, 0, r.stderr)
        return [json.loads(line) for line in r.stdout.splitlines()], art

    def test_worker_writes_and_declares_the_strategy_document_and_critic_judges_its_candidates(self):
        with tempfile.TemporaryDirectory() as d:
            work = pathlib.Path(d)
            (work / "goal.md").write_text("g")
            lines, art = self.run_stub(work, "成果物 `strategy/pilot-replay/v1.json` にする")
            self.assertIn({"type": "artifact", "name": "strategy/pilot-replay/v1.json",
                           "path": "artifacts/strategy/pilot-replay/v1.json"}, lines)
            self.assertEqual(lines[-1]["type"], "done")
            doc = json.loads((art / "strategy/pilot-replay/v1.json").read_text())
            self.assertEqual((doc["schema"], doc["strategy_id"], doc["version"]), ("celeris.strategy/1", "pilot-replay", 1))
            self.assertGreaterEqual(len(doc["candidates"]), 2)
            review = {"criteria": [0], "strategy_critic": {"criterion_idx": 1, "subjects": [
                {"name": "strategy/pilot-replay/v1.json", "path": "artifacts/strategy/pilot-replay/v1.json",
                 "sha256": "sha256:x", "strategy_id": "pilot-replay", "version": 1}], "rubric": []}}
            self.run_stub(work, "", review)
            out = json.loads((art / "review.json").read_text())
            self.assertEqual(out["verdicts"], [{"criterion": 0, "pass": True, "reason": "stub"}])
            subject = out["critic"]["subjects"][0]
            self.assertEqual([c["id"] for c in subject["candidates"]], [c["id"] for c in doc["candidates"]])
            self.assertTrue(subject["comparison"]["ok"] and subject["revisit"]["ok"])

    def test_evaluation_role_writes_parent_evaluation(self):
        with tempfile.TemporaryDirectory() as d:
            work = pathlib.Path(d)
            self.run_stub(work, "", {"criteria": [], "parent_checkpoint": {"x": 1}})
            out = json.loads((work / "artifacts/review.json").read_text())
            self.assertEqual(out["parent_evaluation"]["schema"], "celeris.parent-evaluation/1")


if __name__ == "__main__":
    unittest.main()
