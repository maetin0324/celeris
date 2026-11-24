#!/usr/bin/env python3
"""対照 pilot の盲検の採点材料（pilot_score.py）の単体試験（ADR 2026-11-24-strategy-pilot-ablation D5・D7、rubric.md）。

daemon も cargo も LLM も使わない。入力は合成の testdata（testdata/pilot-runs）と、それを compare --pilot に通した metrics.json。

    python3 -I scripts/dev/parent-eval-blind/test_pilot_score.py -v

守る不変条件:
- blind は方式を事例ごとの乱数の id に替え、方式の名・run id・判断 id・harness の名・answer key の値・史実の参照の点を書かない。
  rubric.md の採点の手順・軸と基準・合格線の節はそのまま写し、pilot-key.json の sha256 を書く。--seed で同じ byte。
- check-blank は節がそろい採点欄が空で方式の名が無いときだけ通る（方式の名の混入・採点欄の記入で落ちる）。repo 用の写しは
  研究ケースの本文を持たず、check-blank を通る。
- check-scored は空欄・0〜2 の外・根拠の空・human: でない採点者で落ちる。
- tally は pilot-key.json の sha256 が採点の file の値と違えば開封しない（exit 1）。一致すれば方式 × 軸 × 採点者の表と合格線の判定を出す。
既存の試験で検出できない理由: 採点材料の script が無く、盲検の伏せ方・採点欄・開封の手順を確かめる試験が無かった。
"""
import json
import pathlib
import re
import subprocess
import sys
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parents[2]
COMPARE = REPO / "scripts/dev/parent-eval-blind-compare.py"
SCORE = HERE / "pilot_score.py"
CLEAN = HERE / "testdata/pilot-runs/iter-1"
KEY = REPO / "agent-docs/progress/2026-11-21-strategy-v1-0/replay/answer_key.json"
sys.path.insert(0, str(HERE))
import pilot_score  # noqa: E402


def score(*args):
    return subprocess.run([sys.executable, "-I", str(SCORE), *map(str, args)], capture_output=True, text=True, timeout=120)


def fill(md, scorer="human:rmaeda", points=None):
    """採点欄を埋める（points: {(case, 方式の id): {軸: 点}}、無ければ 1 点）。"""
    def block(m):
        case, rid, body = m.group(1), m.group(2), m.group(3)
        pts = (points or {}).get((case, rid), {})
        lines = []
        for line in body.splitlines():
            cells = [c.strip() for c in line.strip().strip("|").split("|")]
            if line.startswith("| ") and len(cells) == 3 and cells[0] in pilot_score.axes():
                line = f"| {cells[0]} | {pts.get(cells[0], 1)} | 合成の根拠 {case}/{rid} |"
            elif line.startswith("- 採点者:"):
                line = f"- 採点者: {scorer}"
            elif line.startswith("- レビュー時間（分）:"):
                line = "- レビュー時間（分）: 12"
            lines.append(line)
        return f"<!-- score case={case} id={rid} -->\n" + "\n".join(lines) + "\n<!-- /score -->"
    return pilot_score.SCORE_BLOCK.sub(block, md)


class Score(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.dir = pathlib.Path(cls.tmp.name)
        r = subprocess.run([sys.executable, "-I", str(COMPARE), "--pilot", "--runs", str(CLEAN), "--answer-key", str(KEY),
                            "--out", str(cls.dir)], capture_output=True, text=True, timeout=120)
        assert r.returncode == 0, r.stderr
        cls.metrics = cls.dir / "metrics.json"

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def blind(self, name, seed=7):
        d = self.dir / name
        d.mkdir(exist_ok=True)
        args = ["blind", "--runs", CLEAN, "--metrics", self.metrics, "--out", d / "pilot-scoring.md",
                "--key-out", d / "pilot-key.json", "--repo-copy", d / "repo.md"]
        r = score(*args, *(["--seed", seed] if seed is not None else []))
        self.assertEqual(r.returncode, 0, r.stderr)
        return d

    def test_blind_accepts_runs_container(self):
        d = self.dir / "container"
        d.mkdir(exist_ok=True)
        r = score("blind", "--runs", CLEAN.parent, "--metrics", self.metrics,
                  "--out", d / "scoring.md", "--key-out", d / "key.json", "--seed", 1)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(score("check-blank", d / "scoring.md").returncode, 0)

    def test_blind_hides_the_arms_and_passes_check_blank(self):
        d = self.blind("b1")
        md = (d / "pilot-scoring.md").read_text()
        key = json.loads((d / "pilot-key.json").read_text())
        self.assertEqual(score("check-blank", d / "pilot-scoring.md").returncode, 0)
        self.assertEqual(sorted(key["cases"]), ["replay", "research"])
        for case, ids in key["cases"].items():
            self.assertEqual(sorted(ids.values()), ["a", "b", "c"])
            for rid in ids:
                self.assertRegex(rid, r"^[0-9a-z]{8}$")
                self.assertIn(f"<!-- score case={case} id={rid} -->", md)
        self.assertIn(f"pilot-key.json の sha256: `{pilot_score.sha256_bytes((d / 'pilot-key.json').read_bytes())}`", md)
        outside_rubric = pilot_score.RUBRIC_BLOCK.sub("", md)
        for word in ("fixed_plan", "01SYNRUN", "01SYNDECISION", "stub", "ideal_verdict", "expected_", "wrong_pivot",
                     "史実（方式 (a)）の参照の点", "方式 (a)", "arm"):
            self.assertNotIn(word, outside_rubric, word)
        why = json.loads(KEY.read_text())["checkpoints"]["C-mid"]["why"]
        self.assertNotIn(why, md)
        self.assertIn(pilot_score.rubric_section("合格線"), md)

    def test_seed_gives_the_same_bytes_and_shuffles_records(self):
        a, b = self.blind("s1", 7), self.blind("s2", 7)
        self.assertEqual((a / "pilot-scoring.md").read_bytes(), (b / "pilot-scoring.md").read_bytes())
        self.assertEqual((a / "pilot-key.json").read_bytes(), (b / "pilot-key.json").read_bytes())
        orders = set()
        for seed in range(1, 6):
            md = (self.blind(f"o{seed}", seed) / "pilot-scoring.md").read_text()
            key = json.loads((self.dir / f"o{seed}" / "pilot-key.json").read_text())["cases"]["replay"]
            section = md.split("#### 時点 C-mid", 1)[1].split("####", 1)[0]
            orders.add(tuple(key[i] for i in re.findall(r"##### 記録 `(\w+)`", md.split("#### 時点 C-mid", 1)[1])[:3]))
            self.assertTrue(section)
        self.assertGreater(len(orders), 1, "時点の中の記録の順が乱数で変わらない")

    def test_arm_name_in_the_material_fails_check_blank(self):
        d = self.blind("n1")
        md = (d / "pilot-scoring.md").read_text()
        for leak in ("方式 b", "Evidence + 複数戦略", "fixed_plan", '"arm": "c"'):
            with self.subTest(leak):
                bad = d / "bad.md"
                bad.write_text(md.replace("## 記録\n", f"## 記録\n\n{leak}\n", 1))
                r = score("check-blank", bad)
                self.assertEqual(r.returncode, 1, leak)
                self.assertIn("方式の名", r.stderr)

    def test_arm_names_in_record_text_are_redacted(self):
        self.assertEqual(pilot_score.one_line("方式 c の評価と固定の Task tree"), "［伏せ字］の評価と［伏せ字］")

    def test_filled_or_changed_rubric_fails_check_blank(self):
        d = self.blind("f1")
        md = (d / "pilot-scoring.md").read_text()
        filled = d / "filled.md"
        filled.write_text(fill(md))
        r = score("check-blank", filled)
        self.assertEqual(r.returncode, 1)
        self.assertIn("採点欄が空でない", r.stderr)
        changed = d / "changed.md"
        changed.write_text(md.replace("3 点以上上回ること", "1 点以上上回ること"))
        r = score("check-blank", changed)
        self.assertEqual(r.returncode, 1)
        self.assertIn("rubric の節 合格線 が rubric.md と違う", r.stderr)

    def test_repo_copy_has_no_research_text_and_passes_check_blank(self):
        d = self.blind("r1")
        repo = (d / "repo.md").read_text()
        self.assertEqual(score("check-blank", d / "repo.md").returncode, 0)
        self.assertIn("repo-copy", repo)
        research = repo.split("### 事例 research", 1)[1].split("## id ごとの決定的な指標", 1)[0]
        self.assertNotIn("合成の理由", research)
        self.assertNotIn("合成の候補", research)
        self.assertIn(pilot_score.RESEARCH_OMITTED, research)
        self.assertIn("合成の理由（C-mid）", repo, "replay の本文は残す")

    def test_check_scored_fails_on_blank_and_bad_fields(self):
        d = self.blind("c1")
        md = (d / "pilot-scoring.md").read_text()
        r = score("check-scored", d / "pilot-scoring.md")
        self.assertEqual(r.returncode, 1)
        self.assertIn("点が 0〜2 でない", r.stderr)
        ok = d / "ok.md"
        ok.write_text(fill(md))
        self.assertEqual(score("check-scored", ok).returncode, 0, score("check-scored", ok).stderr)
        for bad, want in ((fill(md, scorer="operator:x"), "採点者が human:<名> でない"),
                          (fill(md).replace("| コスト | 1 |", "| コスト | 3 |", 1), "点が 0〜2 でない"),
                          (re.sub(r"合成の根拠 [^|]*", "", fill(md), count=1), "根拠が空")):
            p = d / "bad.md"
            p.write_text(bad)
            r = score("check-scored", p)
            self.assertEqual(r.returncode, 1, want)
            self.assertIn(want, r.stderr)

    def test_tally_refuses_a_key_with_another_sha256(self):
        d = self.blind("t1")
        scored = d / "scored.md"
        scored.write_text(fill((d / "pilot-scoring.md").read_text()))
        other = d / "other-key.json"
        other.write_bytes((d / "pilot-key.json").read_bytes().replace(b'"seed": 7', b'"seed": 8'))
        r = score("tally", "--scoring", scored, "--key", other)
        self.assertEqual(r.returncode, 1)
        self.assertIn("sha256", r.stderr)
        r = score("tally", "--scoring", d / "pilot-scoring.md", "--key", d / "pilot-key.json")
        self.assertEqual(r.returncode, 1, "空欄の採点は開封しない")

    def test_tally_opens_and_judges_the_pass_line(self):
        d = self.blind("t2")
        key = json.loads((d / "pilot-key.json").read_text())["cases"]
        by_arm = {(case, arm): rid for case, ids in key.items() for rid, arm in ids.items()}
        good = {"execution success": 2, "epistemic progress": 2, "parent goal achievement": 1, "unsupported claims": 1,
                "pivot の時点": 2, "コスト": 2}
        base = {"execution success": 2, "epistemic progress": 0, "parent goal achievement": 1, "unsupported claims": 0,
                "pivot の時点": 0, "コスト": 0}
        points = {}
        for case in ("replay", "research"):
            points[(case, by_arm[(case, "a")])] = base
            points[(case, by_arm[(case, "b")])] = good
            points[(case, by_arm[(case, "c")])] = {**good, "parent goal achievement": 0}
        md = fill((d / "pilot-scoring.md").read_text(), points=points)
        md2 = fill((d / "pilot-scoring.md").read_text(), scorer="human:second")
        (d / "s1.md").write_text(md)
        (d / "s2.md").write_text(md2)
        r = score("tally", "--scoring", d / "s1.md", "--scoring", d / "s2.md", "--key", d / "pilot-key.json",
                  "--metrics", self.metrics, "--out", d / "tally.json")
        self.assertEqual(r.returncode, 0, r.stderr)
        res = json.loads((d / "tally.json").read_text())
        j = res["scorers"]["human:rmaeda"]["judgement"]
        self.assertTrue(j["b"]["pass"], j)
        self.assertFalse(j["c"]["pass"], "parent goal achievement 0 は条件を満たさない")
        self.assertTrue(j["c"]["disqualified"], "execution 2・goal 0 は失格")
        self.assertEqual(res["scorers"]["human:rmaeda"]["points"]["replay/b"]["pivot の時点"], 2)
        self.assertIn("| replay | b | human:rmaeda |", r.stdout)
        self.assertTrue(res["rescore"], "2 名の合計の差が 4 点を超えたらやり直し")


if __name__ == "__main__":
    unittest.main()
