#!/usr/bin/env python3
"""対照 pilot の比較（parent-eval-blind-compare.py --pilot）の単体試験（ADR 2026-11-24-strategy-pilot-ablation D4・D6）。

daemon も cargo も LLM も使わない。入力は合成の testdata（testdata/pilot-runs・pilot-runs-leak。make_pilot_runs.py の出力）だけ。

    python3 -I scripts/dev/parent-eval-blind/test_pilot_compare.py -v

守る不変条件:
- --pilot は <runs>/<case>/<point>/<arm>/ を読み、方式ごとの指標（answer key との一致、誤った pivot・見逃し、根拠の無い claim、
  未達発見までの task 数、検知の後の史実の worker run と USD、usage、secret の形の数、run の記録の mutation）を metrics.json に
  書く。同じ入力で同じ byte で、時刻を書かない。
- input/ と出力に、sentinels.json の語（時点より後）・研究ケースの SENTINELS・answer key の印と理由の文・cutoff 後の event・
  禁じた読みがあれば exit 1（漏れの否定対照。漏れがあっても metrics.json は書く）。
- 漏れの規則は子 0 の replay.py・research_case.py を import し、secret の形は task_core::strategy の正規表現を Rust の source
  から読む（Python に写さない）。
- 子 6 の呼び方（--pilot なし）は今のまま動く: commit した fixture から同じ compare.json を書く。
- 比較は run の記録（--runs）を書き換えない。
既存の試験で検出できない理由: parent-eval-blind-compare.py には試験が無く、子 6 の呼び方は方式 b の verdict・next と読みしか
見ない。方式 a・c と研究ケースの指標、出力側の漏れ、secret・mutation は誰も数えていなかった。
"""
import hashlib
import importlib.util
import json
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parents[2]
COMPARE = REPO / "scripts/dev/parent-eval-blind-compare.py"
TESTDATA = HERE / "testdata"
CLEAN = TESTDATA / "pilot-runs/iter-1"
LEAK = TESTDATA / "pilot-runs-leak/iter-1"
KEY = REPO / "agent-docs/progress/2026-11-21-strategy-v1-0/replay/answer_key.json"
SYNTH = TESTDATA / "research/c-results.md"
BLIND6 = REPO / "crates/task-api/tests/fixtures/parent_checkpoint_live/blind"


def load_compare():
    spec = importlib.util.spec_from_file_location("compare", COMPARE)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def compare(out, *runs, extra=()):
    args = [sys.executable, "-I", str(COMPARE), "--pilot", "--answer-key", str(KEY), "--out", str(out),
            "--research-src", str(SYNTH), *extra]
    for r in runs:
        args += ["--runs", str(r)]
    r = subprocess.run(args, capture_output=True, text=True, timeout=120)
    doc = json.loads((pathlib.Path(out) / "metrics.json").read_text()) if (pathlib.Path(out) / "metrics.json").is_file() else None
    return r, doc


def tree_digest(root):
    h = hashlib.sha256()
    for p in sorted(pathlib.Path(root).rglob("*")):
        if p.is_file():
            h.update(p.relative_to(root).as_posix().encode() + b"\0" + p.read_bytes())
    return h.hexdigest()


class Pilot(unittest.TestCase):
    def test_runs_container_matches_direct_iteration(self):
        direct, expected = compare(self.dir / "direct", CLEAN)
        container, actual = compare(self.dir / "container", CLEAN.parent)
        self.assertEqual(direct.returncode, 0, direct.stderr)
        self.assertEqual(container.returncode, 0, container.stderr)
        self.assertEqual(actual, expected)
        leaked, _ = compare(self.dir / "leaked", LEAK.parent)
        self.assertEqual(leaked.returncode, 1, leaked.stderr)

    def test_container_with_two_iterations_matches_explicit_roots(self):
        root = self.dir / "runs"
        for name in ("iter-1", "iter-2"):
            shutil.copytree(CLEAN, root / name)
        a, expected = compare(self.dir / "explicit", root / "iter-1", root / "iter-2")
        b, actual = compare(self.dir / "parent", root)
        self.assertEqual(a.returncode, 0, a.stderr)
        self.assertEqual(b.returncode, 0, b.stderr)
        self.assertEqual(actual, expected)
        duplicate, _ = compare(self.dir / "duplicate", root, root / "iter-1")
        self.assertEqual(duplicate.returncode, 2, duplicate.stderr)
        self.assertIn("distinct names", duplicate.stderr)

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = pathlib.Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def copy_clean(self, name="iter-1"):
        dest = self.dir / "runs" / name
        shutil.copytree(CLEAN, dest)
        return dest

    def test_testdata_is_what_the_generator_writes(self):
        subprocess.run([sys.executable, "-I", str(TESTDATA / "make_pilot_runs.py"), str(self.dir)], check=True, timeout=60)
        for name in ("pilot-runs", "pilot-runs-leak"):
            self.assertEqual(tree_digest(self.dir / name), tree_digest(TESTDATA / name), name)

    def test_clean_runs_write_metrics_without_time_and_same_bytes(self):
        r, doc = compare(self.dir / "o1", CLEAN)
        self.assertEqual(r.returncode, 0, r.stderr)
        compare(self.dir / "o2", CLEAN)
        first = (self.dir / "o1/metrics.json").read_bytes()
        self.assertEqual(first, (self.dir / "o2/metrics.json").read_bytes())
        self.assertIsNone(re.search(rb"\d{4}-\d\d-\d\dT\d\d:\d\d", first), "metrics.json に時刻がある")
        self.assertEqual((doc["leaks"], doc["failed"], doc["pass"]), ([], [], True))
        self.assertEqual(doc["history"], {"first_detectable": "E-mid", "worker_runs": 137, "usd": 264.2525})
        arms = doc["iterations"]["iter-1"]["arms"]
        a, b, c = arms["a"]["replay"], arms["b"]["replay"], arms["c"]["replay"]
        # 方式 a は親の途中評価が無い: 検知は無く、史実の 8 件と、検知できた時点の後の史実の 137 run が不要な実行。
        self.assertEqual((a["first_detection"], a["tasks_to_detection"], a["llm_runs"]), (None, 8, 0))
        self.assertEqual(a["unneeded_after_first_detectable"], {"worker_runs": 137, "usd": 264.2525})
        self.assertEqual((a["misses"], a["verdict_match"]), (2, 0))
        self.assertEqual((b["first_detection"], b["tasks_to_detection"], b["verdict_match"], b["next_match"]),
                         ("D-done", 4, 3, 3))
        self.assertEqual((c["first_detection"], c["tasks_to_detection"]), ("C-mid", 2))
        self.assertEqual(c["unneeded_after_first_detectable"], {"worker_runs": 0, "usd": 0})
        # 方式 c の usage は 3 時点の評価と版 v1 の段（2 run）。
        self.assertEqual((c["llm_runs"], c["usd"]), (5, 0.625))
        self.assertEqual(arms["c"]["research"]["tasks_to_detection"], 3)
        point = doc["iterations"]["iter-1"]["points"]["replay/C-mid/b"]
        self.assertEqual({k: point["claims"][k] for k in ("claims", "uncertainty", "no_ref", "outside")},
                         {"claims": 3, "uncertainty": 1, "no_ref": 1, "outside": 1})
        self.assertEqual(point["secrets"], {"input": 0, "output": 0})
        self.assertFalse(point["mutation"]["mutated"])

    def test_leak_testdata_is_exit_1_with_each_kind(self):
        r, doc = compare(self.dir / "o", LEAK)
        self.assertEqual(r.returncode, 1, r.stdout)
        leaks = "\n".join(doc["leaks"])
        for want in ("replay/C-mid/b: input:input/tree/events.jsonl: cutoff 後の event",
                     "replay/C-mid/c: input:input/prompt.md: answer key の理由の文",
                     "replay/D-done/b: output:output.json: 未来にだけ現れる語 'parent-unmet'",
                     "research/11.17/b: input:input/tree/results-slice.md: 11.17: '1 / 488'"):
            self.assertIn(want, leaks)
        self.assertFalse(doc["pass"])

    def inject(self, rel, name, text):
        root = self.copy_clean()
        with open(root / rel / name, "a") as f:
            f.write(text)
        return root

    def test_each_negative_control_alone_is_exit_1(self):
        key = json.loads(KEY.read_text())
        cases = {
            "answer key の印": ("replay/D-done/b", "input/prompt.md", "\nideal_verdict\n"),
            "answer key の理由の文": ("replay/F-done/c", "output.json", ""),
            "期待の組": ("replay/C-mid/b", "input/prompt.md", "\nat_risk・continue\n"),
            "後の節の見出し": ("research/11.17/c", "input/prompt.md", "\n11.20 合成 run 6: 合成の反復で 1 cell が止まった\n"),
            "参照の表の文": ("research/11.17/b", "input/prompt.md", "\n同じ優先度の共有で止まる（8/24）。relief は効かない（4/8）\n"),
            "時点より後の event の時刻": ("replay/C-mid/a", "output.json", ""),
        }
        for want, (rel, name, text) in cases.items():
            with self.subTest(want):
                shutil.rmtree(self.dir / "runs", ignore_errors=True)
                root = self.inject(rel, name, text)
                if want == "answer key の理由の文":
                    out = json.loads((root / rel / name).read_text())
                    out["claims"][0]["statement"] = key["checkpoints"]["E-done"]["why"]
                    (root / rel / name).write_text(json.dumps(out, ensure_ascii=False))
                if want == "時点より後の event の時刻":
                    out = json.loads((root / rel / name).read_text())
                    out["claims"].append({"kind": "measured", "statement": "F の統合 2026-11-19T04:55:10.553298044Z", "refs": []})
                    (root / rel / name).write_text(json.dumps(out, ensure_ascii=False))
                r, doc = compare(self.dir / f"o-{len(want)}", root)
                self.assertEqual(r.returncode, 1, f"{want}: {r.stdout}")
                self.assertTrue(any(want in x for x in doc["leaks"]), doc["leaks"])

    def test_forbidden_read_is_a_leak(self):
        root = self.copy_clean()
        reads = json.loads((root / "replay/C-mid/b/reads.json").read_text())
        reads["reads"].append({"tool": "Read", "path": "/synthetic/home/slice-repo/answer_key.json"})
        (root / "replay/C-mid/b/reads.json").write_text(json.dumps(reads))
        r, doc = compare(self.dir / "o", root)
        self.assertEqual(r.returncode, 1)
        self.assertTrue(any("reads: answer_key" in x for x in doc["leaks"]), doc["leaks"])

    def test_secret_shapes_are_counted_without_their_value(self):
        root = self.copy_clean()
        secret = "Bearer abcdefghijklmnopqrstuvwxyz012345"
        out = json.loads((root / "replay/D-done/c/output.json").read_text())
        out["claims"][0]["statement"] += f" Authorization: {secret} と https://hooks.example.invalid/x"
        (root / "replay/D-done/c/output.json").write_text(json.dumps(out, ensure_ascii=False))
        r, doc = compare(self.dir / "o", root)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(doc["iterations"]["iter-1"]["points"]["replay/D-done/c"]["secrets"]["output"], 2)
        self.assertEqual(doc["iterations"]["iter-1"]["arms"]["c"]["replay"]["secrets"], 2)
        self.assertNotIn(secret, (self.dir / "o/metrics.json").read_text())

    def test_secret_pattern_is_read_from_the_rust_source(self):
        mod = load_compare()
        pattern = mod.secret_pattern().pattern
        rust = (REPO / "crates/task-core/src/strategy/mod.rs").read_text()
        self.assertIn(f'r"{pattern}"', rust)
        self.assertIn("hooks", pattern)

    def test_mutation_of_the_record_is_reported(self):
        root = self.copy_clean()
        (root / "replay/F-done/b/input/tree/goal.md").write_text("書き換えた\n")
        reads = json.loads((root / "replay/D-done/b/reads.json").read_text())
        reads["reads"].append({"tool": "Write", "path": "/synthetic/home/slice-repo/goal.md"})
        reads["reads"].append({"tool": "Bash", "command": "echo x > /synthetic/home/slice-repo/slice.json"})
        (root / "replay/D-done/b/reads.json").write_text(json.dumps(reads))
        r, doc = compare(self.dir / "o", root)
        self.assertEqual(r.returncode, 0, r.stderr)
        pts = doc["iterations"]["iter-1"]["points"]
        self.assertEqual(pts["replay/F-done/b"]["mutation"]["input_changed"], ["goal.md"])
        self.assertEqual(pts["replay/D-done/b"]["mutation"]["writes"],
                         ["/synthetic/home/slice-repo/goal.md", "/synthetic/home/slice-repo/slice.json"])
        self.assertTrue(doc["iterations"]["iter-1"]["arms"]["b"]["replay"]["mutated"])
        self.assertFalse(pts["replay/C-mid/b"]["mutation"]["mutated"], "review.json への書き込みは mutation でない")

    def test_iterations_agreement_and_pre_fix_input(self):
        one = self.copy_clean("iter-1")
        two = self.copy_clean("iter-2")
        out = json.loads((two / "replay/C-mid/b/output.json").read_text())
        out["next"] = "pivot"
        (two / "replay/C-mid/b/output.json").write_text(json.dumps(out))
        with open(two / "replay/D-done/b/input/prompt.md", "a") as f:
            f.write("\n（Outcome の記録は無い）\n")
        r, doc = compare(self.dir / "o", one, two)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(doc["agreement"]["replay/C-mid/b"], {"iterations": 2, "same": False})
        self.assertEqual(doc["agreement"]["replay/D-done/b"], {"iterations": 2, "same": True})
        self.assertIn("iter-2/replay/D-done/b: 評価器の入力の修正の前（（Outcome の記録は無い））", doc["excluded"])
        self.assertEqual(doc["usage"], {"llm_runs": 24, "usd": 3.0})
        r, _ = compare(self.dir / "o2", one, one)
        self.assertEqual(r.returncode, 2, "同じ名の --runs は使い方の誤り")

    def test_compare_does_not_write_the_runs(self):
        root = self.copy_clean()
        before = tree_digest(root)
        compare(self.dir / "o", root)
        self.assertEqual(tree_digest(root), before)

    def test_child6_compare_without_pilot_is_unchanged(self):
        out = self.dir / "blind"
        shutil.copytree(BLIND6 / "replay", out / "replay")
        r = subprocess.run([sys.executable, "-I", str(COMPARE), "--out", str(out), "--answer-key", str(KEY)],
                           capture_output=True, text=True, timeout=60)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual((out / "compare.json").read_bytes(), (BLIND6 / "compare.json").read_bytes())
        r = subprocess.run([sys.executable, "-I", str(COMPARE), "--out", str(out), "--answer-key", str(KEY),
                            "--runs", str(CLEAN)], capture_output=True, text=True, timeout=60)
        self.assertEqual(r.returncode, 2, "--runs は --pilot が要る")


if __name__ == "__main__":
    unittest.main()
