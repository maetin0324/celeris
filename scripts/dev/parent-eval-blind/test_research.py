#!/usr/bin/env python3
"""対照 pilot の研究ケース（research.py・driver.py・parent-eval-blind.sh の --case research）の単体試験（ADR 2026-11-24-strategy-pilot-ablation
D2.2・D4・D9）。

daemon も cargo も LLM も使わない。本文は合成の testdata（testdata/research/c-results.md。節の見出しの形だけを研究ケースに合わせた
作り物）だけを使い、研究ケースの実データを開かない。

    python3 -I scripts/dev/parent-eval-blind/test_research.py -v

守る不変条件:
- slice の切り出し・未来の語・sha256 は子 0 の research_case.py の sections・slices・leaks・SENTINELS・SHA を path で import して使い、
  research.py に写さない。
- 時点 p の入力（作業 tree の file と seed の spec）は 11.15 節から p 節までだけで、p より後の節の本文・見出し・初出が p より後の
  SENTINELS を含まない。混ぜた入力（後の節の語・見出し）は run の前に Leak で止まる。
- Outcome（問いと判定方法）は 11.15 節（全時点の cutoff 以前）だけから作る。子は節ごとに 1 件、報告は節の本文。
- `--harness config` は --research-src の sha256 が research_case.SHA と違えば exit 2（承認の検査の後、build の前）。
  承認の scope.cases の名は research-<時点>。
既存の試験で検出できない理由: driver と sh は事例 replay しか受けず（--case research は exit 2）、研究ケースの slice・漏れの検査・
sha256 の検査を誰も確かめていなかった。
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
SYNTH = HERE / "testdata/research/c-results.md"
sys.path.insert(0, str(HERE))
import driver  # noqa: E402
import research  # noqa: E402

MARKS = {"11.15": "SYN-1115", "11.16": "SYN-1116", "11.17": "SYN-1117", "11.18": "SYN-1118", "11.19": "SYN-1119",
         "11.20": "SYN-1120", "11.21": "SYN-1121", "16": "SYN-16"}
ORDER = list(MARKS)


def run_sh(args, env_extra=None):
    env = {k: v for k, v in os.environ.items() if k != "CELERIS_RUN_ID"}
    env.update(env_extra or {})
    return subprocess.run(["bash", str(SH), *args], capture_output=True, text=True, env=env, timeout=60)


def approval(arms, cases):
    return {
        "approval_id": "test", "approved_by": "human:test", "approved_at": "2026-11-24T00:00:00Z",
        "expires_at": "2999-01-01T00:00:00Z", "account": "acct", "model": "claude-sonnet-5-5", "harness": "claude_code",
        "scope": {"arms": arms, "cases": cases, "environment": "non-production"},
        "limits": {"max_usd": 25, "max_wall_secs": 600},
    }


def blob(tree, spec):
    return "\n".join([*(v.decode() for v in tree.values()), json.dumps(spec, ensure_ascii=False)])


class Slice(unittest.TestCase):
    def setUp(self):
        self.text = SYNTH.read_text(encoding="utf-8")

    def test_research_case_is_imported_by_path_not_copied(self):
        self.assertEqual(pathlib.Path(research.rc.__file__).resolve(), research.RESEARCH_CASE.resolve())
        for name in ("sections", "slices", "leaks"):
            self.assertEqual(getattr(research.rc, name).__code__.co_filename, str(research.RESEARCH_CASE))
        src = (HERE / "research.py").read_text()
        for defined in ("def sections", "def slices", "def leaks", "SENTINELS =", "SHA ="):
            self.assertNotIn(defined, src)
        self.assertEqual(research.rc.POINTS[0], research.STRATEGY_POINT)
        self.assertTrue(set(research.POINTS) <= set(research.rc.POINTS))

    def test_slice_is_the_sections_up_to_the_point(self):
        for point in ("11.16", "11.17", "11.20"):
            b = research.build(self.text, point)
            upto = ORDER[: ORDER.index(point) + 1]
            self.assertEqual(b["meta"]["sections"], upto)
            self.assertEqual([c["key"] for c in b["children"]], upto)
            self.assertEqual(b["slice"], research.rc.slices(research.rc.sections(self.text))[point])
            for key in upto:
                self.assertIn(MARKS[key], b["slice"])
            for c in b["children"]:
                self.assertIn(MARKS[c["key"]], c["report"])
                self.assertTrue(c["title"].startswith(c["key"] + " "))
                self.assertEqual(len(c["criteria"]), 1)

    def test_outcome_is_the_question_and_method_of_11_15_only(self):
        outcomes = {p: research.build(self.text, p)["outcome"] for p in research.POINTS}
        o = outcomes["11.17"]
        self.assertTrue(all(v == o for v in outcomes.values()), "Outcome が時点で変わる")
        self.assertEqual(o["title"], "11.15 合成 run 1: 共有の core で合成の node が止まるか")
        self.assertEqual(o["acceptance"], ["合成の問いの判定は、合成の cell ごとの停止の数を数え、止まった cell の割合で決める。"])
        goal = research.build(self.text, "11.17")["goal"]
        self.assertIn("目的：" + o["title"], goal)
        self.assertIn("判定方法：" + o["acceptance"][0], goal)
        for key in ORDER[1:]:
            self.assertNotIn(MARKS[key], goal + json.dumps(o, ensure_ascii=False))

    def test_inputs_have_no_section_after_the_cutoff(self):
        secs = research.rc.sections(self.text)
        for point in ("11.17", "11.20"):
            tree, spec = research.files(SYNTH, point)
            self.assertEqual(sorted(tree), ["goal.md", research.SLICE_FILE, "slice.json"])
            text = blob(tree, spec)
            for key in ORDER[ORDER.index(point) + 1:]:
                self.assertNotIn(MARKS[key], text, f"{point}: {key} 節の本文")
                self.assertNotIn(research.heading(secs[key]), text, f"{point}: {key} 節の見出し")
            self.assertEqual(research.rc.leaks(point, text), [])
        # 11.17 の入力には 11.20 で初めて現れる語が無く、11.20 の入力には有る（検査が語を見ている対照）。
        self.assertNotIn("1 / 488", blob(*research.files(SYNTH, "11.17")))
        self.assertIn("1 / 488", blob(*research.files(SYNTH, "11.20")))

    def test_mixed_inputs_are_stopped_before_the_run(self):
        secs = research.rc.sections(self.text)
        # 否定対照 1: 11.17 の節に 16 節（総括）の語を混ぜる（research_case.py verify の否定対照と同じ）。
        mixed = self.text.replace(secs["11.17"], secs["11.17"] + secs["16"].split("\n", 1)[1])
        with self.assertRaises(research.Leak) as e:
            research.build(mixed, "11.17")
        self.assertIn("1 / 488", str(e.exception))
        # 否定対照 2: 11.16 の節に 11.21 節の見出しを混ぜる。
        mixed = self.text.replace(secs["11.16"], secs["11.16"] + "\n" + research.heading(secs["11.21"]) + "\n")
        with self.assertRaises(research.Leak) as e:
            research.build(mixed, "11.16")
        self.assertIn("later heading", str(e.exception))

    def test_driver_stops_a_leaking_slice_with_exit_1(self):
        secs = research.rc.sections(self.text)
        with tempfile.TemporaryDirectory() as d:
            src = pathlib.Path(d) / "mixed.md"
            src.write_text(self.text.replace(secs["11.17"], secs["11.17"] + secs["16"].split("\n", 1)[1]))
            args = ["--case", "research", "--research-src", str(src), "--point", "11.17", "--arm", "b",
                    "--state", str(pathlib.Path(d) / "s"), "--out", str(pathlib.Path(d) / "o"), "--celeris", "x",
                    "--script-deploy", "x", "--bundle", "x", "--seed-bin", "x", "--shadow", "x", "--harness", "stub"]
            r = subprocess.run([sys.executable, "-I", str(HERE / "driver.py"), *args], capture_output=True, text=True,
                               timeout=60)
        self.assertEqual(r.returncode, 1, r.stderr)
        self.assertIn("leaks the future", r.stderr)


class Shell(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = pathlib.Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def config(self, cases, src=SYNTH, points="11.17,11.20"):
        ap = self.dir / "approval.json"
        ap.write_text(json.dumps(approval(["a", "b", "c"], cases)))
        toml = self.dir / "llm.toml"
        toml.write_text("# not read before the sha256 check\n")
        return run_sh(["--harness", "config", "--arms", "a,b,c", "--case", "research", "--research-src", str(src),
                       "--points", points, "--out", str(self.dir / "o"), "--approval", str(ap),
                       "--llm-config", str(toml)])

    def test_config_with_a_research_src_of_another_sha256_is_exit_2(self):
        r = self.config(["research-11.17", "research-11.20"])
        self.assertEqual(r.returncode, 2, r.stderr)
        self.assertIn("research_case.SHA", r.stderr)
        self.assertIn(research.sha256_of(SYNTH), r.stderr)
        self.assertFalse((self.dir / "o").exists(), "sha256 の検査の前に出力の dir を作った")
        r = subprocess.run([sys.executable, "-I", str(HERE / "research.py"), "check-sha", "--src", str(SYNTH)],
                           capture_output=True, text=True, timeout=60)
        self.assertEqual(r.returncode, 2, r.stderr)

    def test_approval_scope_uses_research_point_names(self):
        r = self.config(["research-11.17"])
        self.assertEqual(r.returncode, 2, r.stderr)
        self.assertIn("scope.cases does not cover research-11.20", r.stderr)

    def test_research_needs_its_src_and_its_points(self):
        r = run_sh(["--harness", "stub", "--arms", "a", "--case", "research", "--points", "11.17", "--out",
                    str(self.dir / "o")])
        self.assertEqual(r.returncode, 2, r.stderr)
        self.assertIn("--research-src", r.stderr)
        r = run_sh(["--harness", "stub", "--arms", "a", "--case", "research", "--research-src", str(SYNTH),
                    "--points", "11.18", "--out", str(self.dir / "o")])
        self.assertEqual(r.returncode, 2, r.stderr)
        self.assertIn("unknown point 11.18", r.stderr)
        r = run_sh(["--harness", "stub", "--arms", "a", "--case", "replay", "--research-src", str(SYNTH),
                    "--points", "C-mid", "--out", str(self.dir / "o")])
        self.assertEqual(r.returncode, 2, r.stderr)

    def test_driver_and_shell_agree_on_the_research_points(self):
        self.assertEqual(driver.CASES["research"], ("11.16", "11.17", "11.20"))
        self.assertEqual(driver.STRATEGY_POINT["research"], "11.15")
        sh = SH.read_text()
        self.assertIn('research) known_points="11.16 11.17 11.20" strategy_point="11.15"', sh)


if __name__ == "__main__":
    unittest.main()
