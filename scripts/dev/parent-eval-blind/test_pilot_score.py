#!/usr/bin/env python3
"""対照 pilot の盲検の採点材料（pilot_score.py）の単体試験（ADR 2026-11-24-strategy-pilot-ablation D5・D7、rubric.md）。

daemon も cargo も LLM も使わない。入力は合成の testdata（testdata/pilot-runs）と、それを compare --pilot に通した metrics.json。

    python3 -I scripts/dev/parent-eval-blind/test_pilot_score.py -v

守る不変条件:
- blind は方式を事例ごとの乱数の id に替え、方式の名・run id・判断 id・harness の名・answer key の値・史実の参照の点を書かない。
  rubric.md の採点の手順・軸と基準・合格線の節はそのまま写し、pilot-key.json の sha256 を書く。--seed で同じ byte。
- check-blank は節がそろい採点欄が空で方式の名が無いときだけ通る（方式の名の混入・採点欄の記入で落ちる）。repo 用の写しは
  研究ケースの本文を持たず、check-blank を通る。
- check-graded（人の決定 2026-11-25 と 2026-11-27、ADR D5 付記 1・2）は採点者の 4 軸の空欄・0〜2 の外・根拠の空・4 軸の「機械:」の
  根拠・数でないレビュー時間・採点者が埋めた機械の 2 軸の行で落ち、機械の 2 軸が空なら通る。採点者は human:<名> なら決定無しで通り、
  model:<id> は --decision の人の決定の記録（scorer が同じ・decided_by が human:<名>・decided_at が YYYY-MM-DD・axes が 4 軸）が
  あるときだけ通る。operator: など他の採点者は落ちる。check-scored・machine-axes・tally も同じ --decision を受ける。
- tally は model の採点者の分に「model の wall・人の時間ではない」と出し、json に kind・決めた人・日を書く。
- commit した scorer-decision.json（子 8 の人の決定の記録）は、今の採点者 model:claude-fable-5-1 を認める形である。
- 埋め込んだ rubric の節のうち「採点の手順」は有無だけを見る（材料の後に人が手順 3 を変えた）。「軸と基準」「合格線」は一致を見る。
- machine-axes は check-graded・key・metrics・answer key の sha256 を確かめてから、pivot の時点とコストの 2 行だけを rubric の基準で
  埋める（ほかの byte は同じ、もう一度流しても同じ byte）。点の境界・反復の最小・wrong_pivots 2 で 0・invalid の記録を検知に数えない。
- check-scored は 6 軸の空欄・0〜2 の外・人の軸の根拠の空・機械の 2 軸の「機械:」で始まらない根拠・human: でない採点者で落ちる。
- tally は pilot-key.json の sha256 が採点の file の値と違えば開封しない（exit 1）。一致すれば方式 × 軸 × 採点者の表と合格線の判定を出す。
  pivot の時点は採点の file の点のまま（wrong_pivots による上書きは machine-axes に移した。--metrics は受けない）。
- record の status が invalid の記録は、判定と次の手が gate の既定値で評価器の答えでないことを示す（ADR D4.1 付記 2）。
既存の試験で検出できない理由: 採点材料の script が無く、盲検の伏せ方・採点欄・開封の手順を確かめる試験が無かった。
採点の分担の試験（machine_axes）は、分担の前の script に該当する subcommand と規則が無いので落ちる。
採点者の試験（check_graded）は、人の決定 2026-11-27 の前の script に check-graded・--decision が無く、model の採点者を
human: でないとだけ見て落としていたので落ちる。
"""
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
SCORE = HERE / "pilot_score.py"
CLEAN = HERE / "testdata/pilot-runs/iter-1"
KEY = REPO / "agent-docs/progress/2026-11-21-strategy-v1-0/replay/answer_key.json"
REPO_COPY = REPO / "agent-docs/progress/2026-11-24-strategy-v1-8/pilot-scoring.md"
DECISION = REPO / "agent-docs/progress/2026-11-24-strategy-v1-8/scorer-decision.json"
MODEL = "model:claude-fable-5-1"
sys.path.insert(0, str(HERE))
import pilot_score  # noqa: E402


def score(*args):
    return subprocess.run([sys.executable, "-I", str(SCORE), *map(str, args)], capture_output=True, text=True, timeout=120)


def fill(md, scorer="human:rmaeda", points=None, machine=False):
    """採点欄の人の 4 軸を埋める（points: {(case, 方式の id): {軸: 点}}、無ければ 1 点）。machine なら機械の 2 軸も「機械:」で埋める。"""
    def block(m):
        case, rid, body = m.group(1), m.group(2), m.group(3)
        pts = (points or {}).get((case, rid), {})
        lines = []
        for line in body.splitlines():
            cells = [c.strip() for c in line.strip().strip("|").split("|")]
            if line.startswith("| ") and len(cells) == 3 and cells[0] in pilot_score.MACHINE_AXES:
                if machine:
                    line = f"| {cells[0]} | {pts.get(cells[0], 1)} | 機械: 合成の規則 {case}/{rid} |"
            elif line.startswith("| ") and len(cells) == 3 and cells[0] in pilot_score.axes():
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

    def blind(self, name, seed=7, metrics=None, runs=CLEAN):
        d = self.dir / name
        d.mkdir(exist_ok=True)
        args = ["blind", "--runs", runs, "--metrics", metrics or self.metrics, "--out", d / "pilot-scoring.md",
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

    def test_invalid_record_says_the_verdict_is_the_gate_default(self):
        # 不変条件（ADR D4.1 付記 2、人の決定 3）: record の status が invalid の記録には、run.json の status と violations を添えて、
        # 判定と次の手が gate の既定値で評価器の答えでないことを 1 行で示す。方式の名は出さず check-blank を通る。checked の記録には出さない。
        # 既存の試験で検出できない理由: testdata の記録は全て checked で、invalid の記録を採点材料に出す試験が無かった（変更前に落ちる）。
        runs = self.dir / "invalid-runs" / "iter-1"
        shutil.copytree(CLEAN, runs)
        point = runs / "replay/D-done/c"
        out = json.loads((point / "output.json").read_text())
        out.update({"status": "invalid", "verdict": "unknown", "next": "ask_human"})
        out["record"].update({"status": "invalid", "violations": ["shape parent_evaluation: 節が無い"],
                              "effective": {"verdict": "unknown", "next": "ask_human"}, "evaluation": {}})
        (point / "output.json").write_text(json.dumps(out, ensure_ascii=False))
        run = json.loads((point / "run.json").read_text())
        (point / "run.json").write_text(json.dumps({**run, "status": "budget_exhausted"}, ensure_ascii=False))
        d = self.dir / "invalid"
        r = subprocess.run([sys.executable, "-I", str(COMPARE), "--pilot", "--runs", str(runs), "--answer-key", str(KEY),
                            "--out", str(d)], capture_output=True, text=True, timeout=120)
        self.assertEqual(r.returncode, 0, r.stderr)
        r = score("blind", "--runs", runs, "--metrics", d / "metrics.json", "--out", d / "s.md", "--key-out", d / "k.json",
                  "--seed", 3)
        self.assertEqual(r.returncode, 0, r.stderr)
        md = (d / "s.md").read_text()
        line = ("- 評価の記録の状態: invalid（run: budget_exhausted、shape parent_evaluation: 節が無い）。"
                "判定と次の手は gate の既定値で、評価器の答えではない")
        self.assertEqual(md.count("評価の記録の状態"), 1, md)
        rid = next(i for i, arm in json.loads((d / "k.json").read_text())["cases"]["replay"].items() if arm == "c")
        record = md.split("#### 時点 D-done", 1)[1].split(f"##### 記録 `{rid}`", 1)[1].split("#####", 1)[0]
        self.assertIn(line, record)
        self.assertEqual(score("check-blank", d / "s.md").returncode, 0)

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
        ok.write_text(fill(md, machine=True))
        self.assertEqual(score("check-scored", ok).returncode, 0, score("check-scored", ok).stderr)
        for bad, want in ((fill(md, scorer="operator:x", machine=True), "採点者が human:<名> でない"),
                          (fill(md, machine=True).replace("| epistemic progress | 1 |", "| epistemic progress | 3 |", 1),
                           "点が 0〜2 でない"),
                          (re.sub(r"合成の根拠 [^|]*", "", fill(md, machine=True), count=1), "根拠が空"),
                          (fill(md), "機械の軸が埋まっていない"),
                          (fill(md, machine=True).replace("| コスト | 1 | 機械: ", "| コスト | 1 | 人: ", 1), "機械の軸が")):
            p = d / "bad.md"
            p.write_text(bad)
            r = score("check-scored", p)
            self.assertEqual(r.returncode, 1, want)
            self.assertIn(want, r.stderr)

    def test_tally_refuses_a_key_with_another_sha256(self):
        d = self.blind("t1")
        scored = d / "scored.md"
        scored.write_text(fill((d / "pilot-scoring.md").read_text(), machine=True))
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
        md = fill((d / "pilot-scoring.md").read_text(), points=points, machine=True)
        md2 = fill((d / "pilot-scoring.md").read_text(), scorer="human:second", machine=True)
        (d / "s1.md").write_text(md)
        (d / "s2.md").write_text(md2)
        r = score("tally", "--scoring", d / "s1.md", "--scoring", d / "s2.md", "--key", d / "pilot-key.json",
                  "--out", d / "tally.json")
        self.assertEqual(r.returncode, 0, r.stderr)
        res = json.loads((d / "tally.json").read_text())
        j = res["scorers"]["human:rmaeda"]["judgement"]
        self.assertTrue(j["b"]["pass"], j)
        self.assertFalse(j["c"]["pass"], "parent goal achievement 0 は条件を満たさない")
        self.assertTrue(j["c"]["disqualified"], "execution 2・goal 0 は失格")
        self.assertEqual(res["scorers"]["human:rmaeda"]["points"]["replay/b"]["pivot の時点"], 2)
        self.assertIn("| replay | b | human:rmaeda |", r.stdout)
        self.assertIn("| pivot の時点（機械） | コスト（機械） |", r.stdout, "表で機械の 2 軸を区別する")
        self.assertEqual(res["machine_axes"], ["pivot の時点", "コスト"])
        self.assertTrue(res["rescore"], "2 名の合計の差が 4 点を超えたらやり直し")
        # wrong_pivots による上書きは machine-axes に移した（二重にしない）。tally は metrics を受けない。
        r = score("tally", "--scoring", d / "s1.md", "--key", d / "pilot-key.json", "--metrics", self.metrics)
        self.assertEqual(r.returncode, 2, r.stderr)


    # ------------------------------------------------------------ 採点の分担（人の決定 2026-11-25、ADR D5・D7 の付記）
    def machine_axes(self, d, scoring, metrics=None, out=None, decision=None):
        return score("machine-axes", "--scoring", scoring, "--key", d / "pilot-key.json", "--metrics", metrics or self.metrics,
                     "--answer-key", KEY, *(["--out", out] if out else []), *(["--decision", decision] if decision else []))

    def graded(self, name, scorer="human:rmaeda", **kw):
        d = self.blind(name, **kw)
        scoring = d / "scored.md"
        scoring.write_text(fill((d / "pilot-scoring.md").read_text(), scorer=scorer))
        return d, scoring

    def changed_metrics(self, name, change, runs=CLEAN):
        """compare --pilot の metrics.json を change(doc) で変えた写し（blind に渡し、key の metrics_sha256 に結ぶ）。"""
        doc = json.loads(self.metrics.read_text()) if runs == CLEAN else None
        if doc is None:
            out = self.dir / f"{name}-compare"
            r = subprocess.run([sys.executable, "-I", str(COMPARE), "--pilot", "--runs", str(runs), "--answer-key", str(KEY),
                                "--out", str(out)], capture_output=True, text=True, timeout=120)
            self.assertEqual(r.returncode, 0, r.stderr)
            doc = json.loads((out / "metrics.json").read_text())
        change(doc)
        path = self.dir / f"{name}-metrics.json"
        path.write_text(json.dumps(doc, ensure_ascii=False, indent=2, sort_keys=True) + "\n")
        return path

    def test_check_graded_accepts_four_axes_with_blank_machine_rows(self):
        # 不変条件: 採点者は 4 軸・採点者・レビュー時間を埋め、機械の 2 軸は空のまま。check-graded は通り、check-scored と check-blank は落ちる。
        d, scoring = self.graded("h1")
        r = score("check-graded", scoring)
        self.assertEqual(r.returncode, 0, r.stderr)
        r = score("check-scored", scoring)
        self.assertEqual(r.returncode, 1)
        self.assertIn("機械の軸が埋まっていない", r.stderr)
        self.assertEqual(score("check-blank", scoring).returncode, 1)
        self.assertEqual(score("check-graded", d / "pilot-scoring.md").returncode, 1, "空の材料は通らない")

    def test_check_graded_rejects_bad_fields(self):
        d, scoring = self.graded("h2")
        md = scoring.read_text()
        blank = (d / "pilot-scoring.md").read_text()
        cases = (
            (md.replace("| epistemic progress | 1 |", "| epistemic progress | 3 |", 1), "点が 0〜2 でない"),
            (md.replace("| parent goal achievement | 1 | 合成の根拠 ", "| parent goal achievement |  | 合成の根拠 ", 1), "点が 0〜2 でない"),
            (re.sub(r"合成の根拠 [^|]*", "", md, count=1), "根拠が空"),
            (md.replace("| unsupported claims | 1 | 合成の根拠 ", "| unsupported claims | 1 | 機械: ", 1), "人の軸の根拠が"),
            (fill(blank, scorer="operator:claude-code-ops-session"), "採点者が human:<名> でない"),
            (md.replace("- レビュー時間（分）: 12", "- レビュー時間（分）: 約 12", 1), "レビュー時間（分）が数でない"),
            (md.replace("| pivot の時点 |  |  |", "| pivot の時点 | 2 | 人が付けた |", 1), "機械の軸が空でも"),
            (md.replace("| コスト |  |  |", "| コスト | 2 |  |", 1), "機械の軸が空でも"),
        )
        for bad, want in cases:
            with self.subTest(want):
                path = d / "bad.md"
                path.write_text(bad)
                r = score("check-graded", path)
                self.assertEqual(r.returncode, 1, want)
                self.assertIn(want, r.stderr)

    def test_check_graded_rubric_procedure_presence_only_and_pass_line_exact(self):
        # 不変条件: 「採点の手順」は有無だけを見る（人が材料の後に手順 1・3 を変え、材料の作り直しを禁じた）。「軸と基準」「合格線」は一致を見る。
        d, scoring = self.graded("h3")
        md = scoring.read_text()
        old_proc = "<!-- rubric:採点の手順 -->\n1. 採点者は人 1 名以上。古い手順の写し。\n<!-- /rubric -->\n"
        changed_proc = re.sub(r"<!-- rubric:採点の手順 -->\n.*?<!-- /rubric -->\n", lambda m: old_proc, md, count=1, flags=re.S)
        self.assertNotEqual(changed_proc, md)
        no_proc = re.sub(r"<!-- rubric:採点の手順 -->\n.*?<!-- /rubric -->\n", "", md, count=1, flags=re.S)
        cases = ((changed_proc, 0, None),
                 (no_proc, 1, "rubric の節 採点の手順 が無い"),
                 (md.replace("3 点以上上回ること", "1 点以上上回ること"), 1, "rubric の節 合格線 が rubric.md と違う"),
                 (md.replace("25 % 以下", "50 % 以下", 1), 1, "rubric の節 軸と基準 が rubric.md と違う"))
        for text, code, want in cases:
            with self.subTest(want):
                path = d / "rubric.md"
                path.write_text(text)
                r = score("check-graded", path)
                self.assertEqual(r.returncode, code, r.stderr)
                if want:
                    self.assertIn(want, r.stderr)
                r = self.machine_axes(d, path)
                self.assertEqual(r.returncode, code, r.stderr)

    def test_committed_repo_copy_passes_check_blank(self):
        # 不変条件: score の段の repo 用の写し（作り直さない）は、rubric の手順 3 を変えた後も check-blank を通る。
        r = score("check-blank", REPO_COPY)
        self.assertEqual(r.returncode, 0, r.stderr)

    def test_machine_axes_point_boundaries(self):
        # 不変条件（rubric の「軸と基準」と「合格線」）: 差 1 以下で 2（前の時点の検知は負）、2〜3 で 1、4 以上か検知無しで 0、
        # wrong_pivots 2 以上で 0。比 25 % 以下で 2、75 % 以下で 1、超えたら 0。
        for diff, wrong, want in ((-5, 0, 2), (0, 0, 2), (1, 1, 2), (2, 0, 1), (3, 0, 1), (4, 0, 0), (9, 0, 0), (None, 0, 0),
                                  (0, 2, 0), (1, 3, 0), (2, 2, 0)):
            self.assertEqual(pilot_score.pivot_points(diff, wrong), want, (diff, wrong))
        for ratio, want in ((0, 2), (0.25, 2), (0.2500001, 1), (0.75, 1), (0.7500001, 0), (1.0, 0)):
            self.assertEqual(pilot_score.cost_points(ratio), want, ratio)

    def test_machine_axes_fills_only_the_two_rows_and_is_idempotent(self):
        d, scoring = self.graded("m1")
        before = scoring.read_text()
        r = self.machine_axes(d, scoring, out=d / "machine.json")
        self.assertEqual(r.returncode, 0, r.stderr)
        after = scoring.read_text()
        old, new = before.split("\n"), after.split("\n")
        self.assertEqual(len(old), len(new))
        changed = [(a, b) for a, b in zip(old, new) if a != b]
        key = json.loads((d / "pilot-key.json").read_text())["cases"]
        self.assertEqual(len(changed), 2 * sum(len(ids) for ids in key.values()), "id ごとに 2 行だけ変わる")
        for a, b in changed:
            axis = a.strip("|").split("|")[0].strip()
            self.assertIn(axis, pilot_score.MACHINE_AXES)
            self.assertEqual(a, f"| {axis} |  |  |")
            self.assertRegex(b, rf"^\| {axis} \| [012] \| 機械: [^|]+ \|$")
        self.assertEqual(score("check-scored", scoring).returncode, 0, score("check-scored", scoring).stderr)
        r = self.machine_axes(d, scoring, out=d / "machine2.json")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(scoring.read_text(), after, "2 回目で byte が変わらない")
        self.assertEqual((d / "machine.json").read_bytes(), (d / "machine2.json").read_bytes())
        doc = json.loads((d / "machine.json").read_text())
        self.assertEqual(doc["history"], {"first_detectable": "E-mid", "usd": 264.2525, "worker_runs": 137})
        self.assertEqual(doc["first_detectable"], {"replay": "E-mid", "research": "11.20"})
        self.assertEqual(sorted(doc["ids"]), sorted(f"{c}/{i}" for c, ids in key.items() for i in ids))

    def test_machine_axes_points_on_the_synthetic_runs(self):
        # 合成の testdata（iter-1）: replay の検知は a 無し・b D-done（E-mid の 1 時点後）・c C-mid（前）。研究ケースは c だけ 11.17 で検知。
        d, scoring = self.graded("m2")
        r = self.machine_axes(d, scoring, out=d / "machine.json")
        self.assertEqual(r.returncode, 0, r.stderr)
        doc = json.loads((d / "machine.json").read_text())
        got = {(v["case"], v["arm"]): (v["pivot の時点"]["points"], v["コスト"]["points"]) for v in doc["ids"].values()}
        self.assertEqual(got, {("replay", "a"): (0, 0), ("replay", "b"): (2, 2), ("replay", "c"): (2, 2),
                               ("research", "a"): (0, 0), ("research", "b"): (0, 0), ("research", "c"): (2, 0)})
        by_arm = {(v["case"], v["arm"]): v for v in doc["ids"].values()}
        self.assertEqual(by_arm[("replay", "b")]["pivot の時点"]["iterations"]["iter-1"]["diff"], 1)
        self.assertEqual(by_arm[("replay", "c")]["pivot の時点"]["iterations"]["iter-1"]["diff"], -3)
        self.assertEqual(by_arm[("replay", "a")]["コスト"]["iterations"]["iter-1"]["ratio"], 1.0)
        md = scoring.read_text()
        self.assertIn("史実 137 run・264.2525 USD", md)
        self.assertIn("25 % 以下で 2・75 % 以下で 1", md)
        self.assertIn("検知できた時点 E-mid", md)
        self.assertIn("検知できた時点 11.20", md)
        self.assertIn("研究ケースは史実の基準が無い・未確認のため 0", md)

    def test_machine_axes_takes_the_minimum_over_iterations_and_wrong_pivots_two_gives_zero(self):
        runs = self.dir / "two-iterations"
        for it in ("iter-1", "iter-2"):
            shutil.copytree(CLEAN, runs / it)

        def change(doc):
            arms = doc["iterations"]["iter-2"]["arms"]
            arms["b"]["replay"]["wrong_pivots"] = 2
            arms["c"]["replay"]["unneeded_after_first_detectable"] = {"worker_runs": 60, "usd": 10.0}
            arms["c"]["research"]["first_detection"] = None
        metrics = self.changed_metrics("min", change, runs=runs)
        d, scoring = self.graded("m3", metrics=metrics, runs=runs)
        r = self.machine_axes(d, scoring, metrics=metrics, out=d / "machine.json")
        self.assertEqual(r.returncode, 0, r.stderr)
        by_arm = {(v["case"], v["arm"]): v for v in json.loads((d / "machine.json").read_text())["ids"].values()}
        b = by_arm[("replay", "b")]["pivot の時点"]
        self.assertEqual((b["iterations"]["iter-1"]["points"], b["iterations"]["iter-2"]["points"], b["points"]), (2, 0, 0))
        c = by_arm[("replay", "c")]["コスト"]
        self.assertEqual(c["iterations"]["iter-2"]["ratio"], round((60 + 5) / 137, 6))
        self.assertEqual((c["iterations"]["iter-1"]["points"], c["iterations"]["iter-2"]["points"], c["points"]), (2, 1, 1))
        rc = by_arm[("research", "c")]["pivot の時点"]
        self.assertEqual((rc["iterations"]["iter-1"]["points"], rc["iterations"]["iter-2"]["points"], rc["points"]), (2, 0, 0))
        self.assertIn("iter-2: 検知無し → 0", by_arm[("research", "c")]["pivot の時点"]["why"])

    def test_machine_axes_does_not_count_an_invalid_record_as_detection(self):
        # 不変条件: invalid の記録（評価器が答えを出せなかった記録）は検知に数えない。次の検知（F-done、E-mid の 4 時点後）で 0 点。
        def change(doc):
            doc["iterations"]["iter-1"]["points"]["replay/D-done/b"]["status"] = "invalid"
        metrics = self.changed_metrics("invalid", change)
        d, scoring = self.graded("m4", metrics=metrics)
        r = self.machine_axes(d, scoring, metrics=metrics, out=d / "machine.json")
        self.assertEqual(r.returncode, 0, r.stderr)
        by_arm = {(v["case"], v["arm"]): v for v in json.loads((d / "machine.json").read_text())["ids"].values()}
        pv = by_arm[("replay", "b")]["pivot の時点"]
        self.assertEqual(pv["iterations"]["iter-1"]["counted_detection"], "F-done")
        self.assertEqual((pv["iterations"]["iter-1"]["diff"], pv["points"]), (4, 0))
        self.assertIn("invalid の記録の検知 D-done を数えない", scoring.read_text())

    def test_machine_axes_refuses_unscored_files_and_other_inputs(self):
        d, scoring = self.graded("m5")
        md = scoring.read_text()
        other_key = d / "other-key.json"
        other_key.write_bytes((d / "pilot-key.json").read_bytes().replace(b'"seed": 7', b'"seed": 8'))
        other_metrics = self.changed_metrics("other", lambda doc: doc.update({"note": "other"}))
        other_answer = d / "answer_key.json"
        other_answer.write_bytes(KEY.read_bytes() + b"\n")
        blank = d / "blank.md"
        blank.write_bytes((d / "pilot-scoring.md").read_bytes())
        cases = (
            (["--scoring", blank, "--key", d / "pilot-key.json", "--metrics", self.metrics, "--answer-key", KEY], "check-graded"),
            (["--scoring", scoring, "--key", other_key, "--metrics", self.metrics, "--answer-key", KEY], "pilot-key.json の sha256"),
            (["--scoring", scoring, "--key", d / "pilot-key.json", "--metrics", other_metrics, "--answer-key", KEY], "metrics_sha256"),
            (["--scoring", scoring, "--key", d / "pilot-key.json", "--metrics", self.metrics, "--answer-key", other_answer],
             "answer_key_sha256"),
        )
        for args, want in cases:
            with self.subTest(want):
                r = score("machine-axes", *args)
                self.assertEqual(r.returncode, 1, r.stderr)
                self.assertIn(want, r.stderr)
        self.assertEqual(scoring.read_text(), md, "断ったときは採点の file を変えない")
        self.assertEqual(blank.read_bytes(), (d / "pilot-scoring.md").read_bytes())

    # ------------------------------------------------------------ 採点者（人の決定 2026-11-27、ADR D5 付記 2、rubric の手順 1・3）
    def decision(self, name, **change):
        """commit した scorer-decision.json の欄を change で替えた写し（値が None の欄は消す）。"""
        doc = json.loads(DECISION.read_text())
        for k, v in change.items():
            if v is None:
                doc.pop(k, None)
            else:
                doc[k] = v
        path = self.dir / f"{name}-decision.json"
        path.write_text(json.dumps(doc, ensure_ascii=False, indent=2) + "\n")
        return path

    def test_check_graded_model_scorer_passes_only_with_the_human_decision(self):
        # 不変条件: model:<id> の採点者は、--decision の人の決定（scorer が同じ・decided_by が human:<名>・decided_at が YYYY-MM-DD・
        # axes が採点者の 4 軸）があるときだけ通る。決定が無い・欄が違うと落ちる。
        d, scoring = self.graded("g1", scorer=MODEL)
        r = score("check-graded", scoring, "--decision", self.decision("g1"))
        self.assertEqual(r.returncode, 0, r.stderr)
        r = score("check-graded", scoring)
        self.assertEqual(r.returncode, 1)
        self.assertIn(f"採点者 {MODEL} を認めた人の決定の記録（--decision）が無い", r.stderr)
        axes4 = ["execution success", "epistemic progress", "parent goal achievement", "unsupported claims"]
        cases = (
            (dict(decided_by="operator:claude-code-ops-session"), "--decision の decided_by が human:<名> でない"),
            (dict(decided_by=MODEL), "--decision の decided_by が human:<名> でない"),
            (dict(decided_by=None), "--decision の decided_by が human:<名> でない"),
            (dict(scorer="model:claude-sonnet-5-5"), f"採点者 {MODEL} が --decision の scorer"),
            (dict(scorer=None), "--decision の scorer が model:<id> でない"),
            (dict(axes=axes4[:3]), "--decision の axes が採点者の 4 軸"),
            (dict(axes=[*axes4, "pivot の時点"]), "--decision の axes が採点者の 4 軸"),
            (dict(axes=None), "--decision の axes が採点者の 4 軸"),
            (dict(decided_at="2026/11/27"), "--decision の decided_at が YYYY-MM-DD でない"),
            (dict(decided_at="2026-13-27"), "--decision の decided_at が YYYY-MM-DD でない"),
            (dict(decided_at=None), "--decision の decided_at が YYYY-MM-DD でない"),
        )
        for i, (change, want) in enumerate(cases):
            with self.subTest(want=want, change=change):
                r = score("check-graded", scoring, "--decision", self.decision(f"g1-{i}", **change))
                self.assertEqual(r.returncode, 1, r.stderr)
                self.assertIn(want, r.stderr)

    def test_check_graded_other_scorers_fail_and_human_needs_no_decision(self):
        # 不変条件: human:<名> は決定無しでも決定付きでも通る。operator:・cos:・接頭辞の無い名・名の無い human:/model:・大文字の Human: は、
        # 決定があっても落ちる。
        d, scoring = self.graded("g2")
        ok = self.decision("g2")
        self.assertEqual(score("check-graded", scoring).returncode, 0)
        self.assertEqual(score("check-graded", scoring, "--decision", ok).returncode, 0)
        blank = (d / "pilot-scoring.md").read_text()
        for scorer in ("operator:claude-code-ops-session", "cos:x", "claude-fable-5-1", "human:", "model:", "Human:rmaeda", ""):
            for extra in ((), ("--decision", ok)):
                with self.subTest(scorer=scorer, extra=extra):
                    path = d / "bad.md"
                    path.write_text(fill(blank, scorer=scorer))
                    r = score("check-graded", path, *extra)
                    self.assertEqual(r.returncode, 1, r.stderr)
                    self.assertIn("採点者が human:<名> でない", r.stderr)

    def test_check_graded_decision_reaches_check_scored_machine_axes_and_tally(self):
        # 不変条件: check-scored・machine-axes・tally も --decision を受け、決定の無い model の採点者は断る（file を変えない）。
        # tally は model の採点者の分に「model の wall・人の時間ではない」と出し、人の採点者の分には出さない。
        d, scoring = self.graded("g3", scorer=MODEL)
        ok = self.decision("g3")
        before = scoring.read_bytes()
        r = self.machine_axes(d, scoring)
        self.assertEqual(r.returncode, 1)
        self.assertIn("check-graded を通らない", r.stderr)
        self.assertEqual(scoring.read_bytes(), before, "断ったときは採点の file を変えない")
        r = self.machine_axes(d, scoring, decision=ok)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(score("check-scored", scoring, "--decision", ok).returncode, 0)
        r = score("check-scored", scoring)
        self.assertEqual(r.returncode, 1)
        self.assertIn("人の決定の記録（--decision）が無い", r.stderr)
        human = d / "human.md"
        human.write_text(fill((d / "pilot-scoring.md").read_text(), machine=True))
        r = score("tally", "--scoring", scoring, "--scoring", human, "--key", d / "pilot-key.json")
        self.assertEqual(r.returncode, 1, "決定の無い model の採点は開封しない")
        r = score("tally", "--scoring", scoring, "--scoring", human, "--key", d / "pilot-key.json", "--decision", ok,
                  "--out", d / "tally.json")
        self.assertEqual(r.returncode, 0, r.stderr)
        rows = [line for line in r.stdout.splitlines() if line.startswith("| replay |")]
        self.assertTrue(rows)
        for line in rows:
            if f"| {MODEL} |" in line:
                self.assertTrue(line.endswith("| 12.0（model の wall・人の時間ではない） |"), line)
            else:
                self.assertTrue(line.endswith("| 12.0 |"), line)
        self.assertIn(f"採点者 {MODEL}: human:rmaeda が 2026-11-27 に決めた model の採点。分は model の wall・人の時間ではない", r.stdout)
        res = json.loads((d / "tally.json").read_text())["scorers"]
        self.assertEqual((res[MODEL]["kind"], res[MODEL]["minutes_note"], res[MODEL]["decided_by"], res[MODEL]["decided_at"]),
                         ("model", "model の wall・人の時間ではない", "human:rmaeda", "2026-11-27"))
        self.assertEqual(res["human:rmaeda"]["kind"], "human")
        self.assertNotIn("minutes_note", res["human:rmaeda"])

    def test_check_graded_committed_decision_admits_the_pilot_scorer(self):
        # 不変条件: commit した子 8 の人の決定の記録は、採点者 model:claude-fable-5-1 を認める形で、決めたのは human:rmaeda（2026-11-27）。
        doc = json.loads(DECISION.read_text())
        self.assertEqual(pilot_score.decision_problems(doc), [])
        self.assertEqual((doc["scorer"], doc["decided_by"], doc["decided_at"]), (MODEL, "human:rmaeda", "2026-11-27"))
        self.assertEqual(doc["axes"], pilot_score.graded_axes())

    def test_check_graded_blind_material_states_the_scorer_rule(self):
        # 不変条件: blind が作る材料の「この文書について」は、採点者を人か人の決定の記録がある model と書き、機械の 2 行を空に残すと書く。
        d = self.blind("g5")
        about = (d / "pilot-scoring.md").read_text().split("## この文書について", 1)[1].split("\n## ", 1)[0]
        for want in ("`human:<名>`", "人の決定の記録がある model（`model:<id>`）", "pivot の時点とコストの行は空のまま残す",
                     "model の採点者のレビュー時間は model の wall で、人の時間ではない"):
            self.assertIn(want, about)
        self.assertNotIn("採点者は人（rubric の手順）", about)
        self.assertEqual(score("check-blank", d / "pilot-scoring.md").returncode, 0)

if __name__ == "__main__":
    unittest.main()
