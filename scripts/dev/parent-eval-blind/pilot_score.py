#!/usr/bin/env python3
"""対照 pilot の盲検の採点材料（ADR 2026-11-24-strategy-pilot-ablation D5・D6・D7、子 0 の rubric.md）。

    python3 -I pilot_score.py blind --runs <dir> [--runs <dir> ...] --metrics <metrics.json> --out <pilot-scoring.md>
        --key-out <pilot-key.json> [--seed N] [--repo-copy <md>]
    python3 -I pilot_score.py check-blank <md>       # 節がそろい、採点欄が空で、方式の名が無い（repo 用の写しも受ける）
    python3 -I pilot_score.py check-human <md>       # 人の 4 軸が 0〜2 で根拠が空でない、採点者が human:<名>、レビュー時間が数、
                                                     # 機械の 2 軸（pivot の時点・コスト）の行は空か根拠が「機械:」で始まる
    python3 -I pilot_score.py machine-axes --scoring <md> --key <pilot-key.json> --metrics <metrics.json>
        --answer-key <answer_key.json> [--out <json>]
    python3 -I pilot_score.py check-scored <md>      # 6 軸が 0〜2、人の 4 軸の根拠が空でない、機械の 2 軸の根拠が「機械:」で始まる
    python3 -I pilot_score.py tally --scoring <md> [--scoring <md> ...] --key <pilot-key.json> [--out <json>]

blind: --runs（driver の出力。反復ごとの dir またはその親 dir。dir の名が反復の名）と --metrics（parent-eval-blind-compare.py --pilot の出力）から、
事例ごとに方式を乱数の id（8 文字）に替えた記録（時点ごとの判定・根拠・次の手・費用と、answer key に依らない指標）と、
rubric の 6 軸 × 0〜2 点と根拠の空欄・採点者・レビュー時間（分）の欄、rubric.md の採点の手順・軸と基準・合格線の節（そのまま）、
pilot-key.json の sha256 を書く。方式の名・run id・判断 id・harness の名・史実の参照の点・answer key の値は書かない。
時点の中の記録の順は乱数で並べる。--seed を渡すと同じ入力で同じ byte。--repo-copy は研究ケースの本文（理由・主張・版の文）を
除いた写し（repo に置く用。研究ケースの本文を repo に写さない。ADR D9）。
採点の分担（人の決定 2026-11-25、ADR D5・D7 の付記）: 人は 4 軸（execution success・epistemic progress・parent goal achievement・
unsupported claims）とレビュー時間（分）を付ける。pivot の時点とコストの 2 軸は人が付けず、開封の後に machine-axes が付ける。
machine-axes: check-human と pilot-key.json の sha256（採点の file の値）・metrics.json の sha256（key の値）・answer key の sha256
（metrics の値）を確かめてから key を開き、pivot の時点とコストの 2 行だけを点と「機械: <規則> <数値>」で埋める（ほかの byte を
変えない。同じ入力でもう一度流しても同じ byte）。--out に id ごとの規則と数値を書く。規則は反復ごとに当て、id の点は反復の最小。
- pivot の時点: 検知できた時点（replay は answer key の first_detectable、研究ケースは ADR D2.2 の反証の 11.20）から metrics の
  first_detection までの時点の差（replay は answer key の order の順、研究ケースは research_case.py の POINTS の順。符号付きで、
  前の時点の検知は負）が 1 以下で 2、2〜3 で 1、4 以上か検知無しで 0。wrong_pivots が 2 以上なら 0。invalid の記録は検知に数えない。
- コスト: unneeded_after_first_detectable に方式の llm_runs・usd を足し、史実（metrics の history: 137 run・264.2525 USD）に対する
  比の大きい方が 25 % 以下で 2、75 % 以下で 1、超えたら 0。研究ケースは史実の基準が無いので 0（根拠に「史実の基準が無い・未確認」）。
tally: 各採点の file の pilot-key.json の sha256 を確かめてから開封し、方式 × 軸 × 採点者の表（機械の 2 軸に「（機械）」）と
合格線の判定を出す。check-scored を通らない（機械の 2 軸が埋まっていない）file は開封しない。
exit: 0 = 成功（tally は合格線を満たさなくても 0）、1 = 検査の不合格・sha256 の不一致・記録が読めない、2 = 使い方の誤り。
"""
import argparse
import hashlib
import json
import pathlib
import random
import re
import secrets
import sys

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import driver  # noqa: E402
from pilot_runs import iteration_roots  # noqa: E402

COMPARE = HERE.parent / "parent-eval-blind-compare.py"

RUBRIC = HERE.parents[2] / "agent-docs/progress/2026-11-21-strategy-v1-0/rubric.md"
MARK = "<!-- pilot-scoring: celeris.pilot-scoring/1"
KEY_SCHEMA = "celeris.pilot-key/1"
RUBRIC_SECTIONS = ("採点の手順", "軸と基準", "合格線")
HEADINGS = ("## この文書について", "## 記録", "## id ごとの決定的な指標", "## 採点欄", "## 採点の後に開く",
            *(f"## {s}（rubric.md から）" for s in RUBRIC_SECTIONS))
ID_CHARS = "0123456789defghjkmnpqrstvwxyz"
# 方式の名（方式 a・(b)・arm c、3 方式の呼び名、方式 a の status）。rubric の節（そのまま写したもの）の外にあれば check-blank が落ちる。
ARM_WORDS = re.compile(r"方式\s*[(（]?\s*[abcａｂｃ]\s*[)）]?(?![A-Za-z0-9])|\barms?[\s\"'=:_-]*[abc]\b|固定の?\s*Task\s*tree"
                       r"|途中の親の?\s*Evaluator|Evidence\s*[+＋]\s*複数|複数戦略|fixed_plan", re.I)
SHA_LINE = re.compile(r"pilot-key\.json の sha256: `([0-9a-f]{64})`")
SCORE_BLOCK = re.compile(r"<!-- score case=(\S+) id=(\S+) -->\n(.*?)<!-- /score -->", re.S)
RUBRIC_BLOCK = re.compile(r"<!-- rubric:(\S+) -->\n(.*?)<!-- /rubric -->\n", re.S)
REDACTED = "［伏せ字］"
RESEARCH_OMITTED = "（研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る）"
AXIS = {"execution": "execution success", "epistemic": "epistemic progress", "goal": "parent goal achievement",
        "unsupported": "unsupported claims", "pivot": "pivot の時点", "cost": "コスト"}
# 人が付けない 2 軸（人の決定 2026-11-25）。開封の後に machine-axes が付け、根拠を MACHINE で始める。
MACHINE_AXES = (AXIS["pivot"], AXIS["cost"])
MACHINE = "機械:"
# 埋め込んだ rubric の節のうち、一致を見る節。「採点の手順」は有無だけを見る（材料を作った後に人が手順 3 を変え、材料の作り直しを禁じた）。
RUBRIC_EXACT = ("軸と基準", "合格線")
# 研究ケースの検知できた時点（ADR D2.2: 11.20 で yield でも止まり、早すぎた結論が反証された）。
RESEARCH_FIRST_DETECTABLE = "11.20"


def fail(msg, code=1):
    print(f"pilot_score: FAIL: {msg}", file=sys.stderr)
    return code


def rubric_section(name):
    text = RUBRIC.read_text()
    body = text.split(f"\n## {name}", 1)[1].split("\n", 1)[1]
    return body.split("\n## ", 1)[0].strip("\n") + "\n"


def axes():
    """rubric.md の「軸と基準」の表の最初の列（6 軸）。"""
    rows = [line for line in rubric_section("軸と基準").splitlines() if line.startswith("|")][2:]
    return [row.strip("|").split("|")[0].strip() for row in rows]


def sha256_bytes(data):
    return hashlib.sha256(data).hexdigest()


def one_line(text, redact=True):
    text = " ".join(str(text or "").split())
    return ARM_WORDS.sub(REDACTED, text) if redact else text


# ---------------------------------------------------------------- blind
def load_runs(runs, metrics):
    """{(iteration, case, point, arm): (output, metrics の行, run.json の status)} と {(iteration, case): 版の段の出力}。"""
    records, stages = {}, {}
    for root in iteration_roots(runs):
        it = root.resolve().name
        if it not in metrics["iterations"]:
            raise ValueError(f"metrics.json has no iteration {it}（compare --pilot に同じ --runs を渡す）")
        rows = metrics["iterations"][it]["points"]
        for case, points in driver.CASES.items():
            stage = root / case / driver.STRATEGY_STAGE / driver.STRATEGY_FILE
            if stage.is_file():
                stages[(it, case)] = json.loads(stage.read_text())
            for point in points:
                for arm in driver.ARMS:
                    d = root / case / point / arm
                    if (d / "output.json").is_file():
                        if f"{case}/{point}/{arm}" not in rows:
                            raise ValueError(f"metrics.json has no {it}/{case}/{point}/{arm}")
                        run = json.loads((d / "run.json").read_text()) if (d / "run.json").is_file() else {}
                        records[(it, case, point, arm)] = (json.loads((d / "output.json").read_text()),
                                                           rows[f"{case}/{point}/{arm}"], run.get("status"))
    if not records:
        raise ValueError("no <case>/<point>/<arm>/output.json under --runs")
    return records, stages


def new_ids(rng, arms_by_case):
    used, key = set(), {}
    for case, arms in arms_by_case.items():
        key[case] = {}
        for arm in arms:
            while True:
                i = "".join(rng.choice(ID_CHARS) for _ in range(8))
                if i not in used:
                    break
            used.add(i)
            key[case][i] = arm
    return key


def strategy_lines(doc, omit):
    if omit:
        return [f"- 戦略の版 v1: {RESEARCH_OMITTED}"]
    out = ["- 戦略の版 v1（この記録の評価の作業 tree に置いた文書）:"]
    for c in doc.get("candidates") or []:
        exp = (c.get("experiment") or {}).get("content")
        out.append(f"  - 候補 `{one_line(c.get('id'))}`: {one_line(c.get('title'))}。仮説 {one_line(', '.join(c.get('hypotheses') or []))}。"
                   f"識別実験: {one_line(exp)}")
    return out


def record_lines(rid, it, output, row, run_status, stage, excluded, omit):
    record = output.get("record") or {}
    ev = record.get("evaluation") or {}
    verdict = output.get("verdict") or "—（親の途中評価の記録が無い）"
    lines = [f"##### 記録 `{rid}`（反復 {it}）", ""]
    if excluded:
        lines += ["- **この記録は採点から外す**（compare --pilot が漏れか修正前の入力を検出した。理由は採点の後に metrics.json で開く）"]
    if record.get("status") == "invalid":
        # 評価器が答えを出せなかった記録（例: turn の上限）。下の判定と次の手は gate の既定値なので、人が評価器の答えと読み違えない
        # ように示す（ADR D4.1 付記 2）。方式の名は出さない。
        why = "、".join(one_line(v) for v in record.get("violations") or []) or "違反の記録無し"
        lines.append(f"- 評価の記録の状態: invalid（run: {one_line(run_status) or '—'}、{why}）。判定と次の手は gate の既定値で、評価器の答えではない")
    lines += [f"- 判定（verdict）: {verdict}", f"- 次の手（next）: {output.get('next') or '—'}"]
    if ev.get("axes"):
        lines.append("- 評価の軸（axes）: " + "、".join(f"{k}={v}" for k, v in sorted(ev["axes"].items())))
    if ev.get("reason"):
        lines.append(f"- 理由: {RESEARCH_OMITTED if omit else one_line(ev['reason'])}")
    lines.append("- 主張と根拠:" if output.get("claims") else "- 主張と根拠: 無し")
    for c in output.get("claims") or []:
        refs = "、".join(one_line(" ".join(f"{k}={v}" for k, v in sorted(r.items()) if k != "kind"), False)
                        if isinstance(r, dict) else one_line(r) for r in c.get("refs") or []) or "参照無し"
        lines.append(f"  - [{one_line(c.get('kind'))}] {RESEARCH_OMITTED if omit else one_line(c.get('statement'))} — 根拠: {refs}")
    if stage is not None:
        lines += strategy_lines(stage, omit)
    claims = row["claims"]
    lines += [f"- 費用: LLM run {row['llm_runs']}・{row['usd']} USD",
              f"- 決定的な指標: 根拠の無い claim {claims['no_ref'] + claims['outside']}（参照無し {claims['no_ref']}・slice の外 "
              f"{claims['outside']}）、secret の形 {row['secrets']['input'] + row['secrets']['output']}、記録の mutation "
              f"{'有' if row['mutation']['mutated'] else '無'}、読んだ path {row['reads']}", ""]
    return lines


def metric_rows(metrics, key):
    lines = ["| 事例 | id | 反復 | 未達発見の時点 | 未達発見までの task 数 | 検知できた時点から検知までの史実の worker run・USD | "
             "検知の後の史実の worker run・USD | LLM run | USD | 根拠の無い claim | secret の形 | mutation |",
             "|---|---|---|---|---|---|---|---|---|---|---|---|"]

    def runs(v):
        return f"{v['worker_runs']}・{v['usd']}" if v else "—"
    for case, ids in key.items():
        for rid, arm in sorted(ids.items()):
            for it, data in sorted(metrics["iterations"].items()):
                s = (data["arms"].get(arm) or {}).get(case)
                if s:
                    lines.append(f"| {case} | `{rid}` | {it} | {s['first_detection'] or '無し'} | {'—' if s['tasks_to_detection'] is None else s['tasks_to_detection']} | "
                                 f"{runs(s.get('unneeded_after_first_detectable'))} | {runs(s.get('after_detection'))} | "
                                 f"{s['llm_runs']} | {s['usd']} | {s['unsupported_claims']} | {s['secrets']} | "
                                 f"{'有' if s['mutated'] else '無'} |")
    return lines


def score_block(case, rid):
    rows = [f"| {a} |  |  |" for a in axes()]
    return [f"### 事例 {case}・id `{rid}`", "", f"<!-- score case={case} id={rid} -->", "| 軸 | 点（0〜2） | 根拠 |",
            "|---|---|---|", *rows, "", "- 採点者: ", "- レビュー時間（分）: ", "<!-- /score -->", ""]


def render(records, stages, metrics, key, key_sha, metrics_path, rng_order, repo_copy):
    head = MARK + (" repo-copy -->" if repo_copy else " -->")
    lines = ["# 対照 pilot の盲検の採点材料（戦略層 v1 子 8）", "", head, "", "## この文書について", "",
             "- 方式の名を、事例ごとに乱数の id（8 文字）に替えた。対応表は pilot-key.json にあり、採点の後まで開かない。",
             f"- pilot-key.json の sha256: `{key_sha}`",
             "- 採点者は人（rubric の手順）。実装者・CoS・operator は採点しない。採点欄の点・根拠・採点者（`human:<名>`）・レビュー時間（分）を埋める。",
             "- 既知の限界: 伏せたのは名前だけで、記録の内容（親の途中評価の本文の有無・戦略の版の有無）から方式を推測できる。",
             f"- 記録の文の中の方式の名に当たる語は {REDACTED} に替えた。answer key の値と史実の参照の点は書いていない。"]
    if repo_copy:
        lines.append(f"- この写しは repo 用で、研究ケースの理由・主張・版の文を除いた（{RESEARCH_OMITTED}）。")
    lines.append("")
    for name in RUBRIC_SECTIONS[:2]:
        lines += [f"## {name}（rubric.md から）", "", f"<!-- rubric:{name} -->", rubric_section(name).rstrip("\n"),
                  "<!-- /rubric -->", ""]
    lines += ["## 記録", ""]
    excluded = {e.split(":", 1)[0] for e in metrics.get("excluded", [])}
    by_arm = {case: {arm: rid for rid, arm in ids.items()} for case, ids in key.items()}
    for case, points in driver.CASES.items():
        if not any(k[1] == case for k in records):
            continue
        lines += [f"### 事例 {case}", ""]
        for point in points:
            here = sorted(k for k in records if k[1] == case and k[2] == point)
            if not here:
                continue
            rng_order.shuffle(here)
            lines += [f"#### 時点 {point}", ""]
            for k in here:
                it, _, _, arm = k
                output, row, run_status = records[k]
                stage = stages.get((it, case)) if arm == "c" else None
                lines += record_lines(by_arm[case][arm], it, output, row, run_status, stage, f"{it}/{case}/{point}/{arm}" in excluded,
                                      repo_copy and case == "research")
    lines += ["## id ごとの決定的な指標", "",
              "検知できた時点（replay は E-mid）から検知までの史実の worker run は rubric のコストの軸の材料（史実 137 run・264.25 USD）。"
              "研究ケースの worker run は数えていない（未確認）。", "", *metric_rows(metrics, key), "", "## 採点欄", ""]
    for case, ids in key.items():
        for rid in sorted(ids):
            lines += score_block(case, rid)
    lines += [f"## {RUBRIC_SECTIONS[2]}（rubric.md から）", "", f"<!-- rubric:{RUBRIC_SECTIONS[2]} -->",
              rubric_section(RUBRIC_SECTIONS[2]).rstrip("\n"), "<!-- /rubric -->", "",
              "## 採点の後に開く", "", "採点を書き終えてから開く（rubric の手順 2）。",
              "- answer key: `agent-docs/progress/2026-11-21-strategy-v1-0/replay/answer_key.json`",
              "- 研究ケースの参照の表: `agent-docs/progress/2026-11-21-strategy-v1-0/research/research-case.md` の「時点ごとの採点の参照」",
              "- 方式の対応表: `pilot-key.json`（sha256 は冒頭）",
              f"- 指標の全体（answer key との一致・誤った pivot・見逃しを含む）: `{metrics_path}`", ""]
    return "\n".join(lines)


def cmd_blind(a):
    metrics = json.loads(pathlib.Path(a.metrics).read_text())
    records, stages = load_runs(a.runs, metrics)
    rng = random.Random(a.seed) if a.seed is not None else secrets.SystemRandom()
    arms = {c: [m for m in driver.ARMS if any(k[1] == c and k[3] == m for k in records)] for c in driver.CASES}
    key = new_ids(rng, {c: ms for c, ms in arms.items() if ms})
    key_doc = {"schema": KEY_SCHEMA, "seed": a.seed, "cases": key,
               "metrics_sha256": sha256_bytes(pathlib.Path(a.metrics).read_bytes()),
               "iterations": sorted({k[0] for k in records})}
    key_bytes = (json.dumps(key_doc, ensure_ascii=False, indent=2, sort_keys=True) + "\n").encode()
    key_sha = sha256_bytes(key_bytes)
    order_seed = rng.random()
    for path, repo_copy in ((a.out, False), (a.repo_copy, True)):
        if path:
            md = render(records, stages, metrics, key, key_sha, a.metrics, random.Random(order_seed), repo_copy)
            pathlib.Path(path).write_text(md)
            problems = blank_problems(md)
            if problems:
                return fail(f"{path} does not pass check-blank: {problems}")
    pathlib.Path(a.key_out).write_bytes(key_bytes)
    print(f"pilot_score: wrote {a.out} and {a.key_out} (sha256 {key_sha})")
    return 0


# ---------------------------------------------------------------- 検査
def parse_scores(md):
    """{(case, id): {"axes": {軸: (点, 根拠)}, "scorer": str, "minutes": str}}。"""
    out = {}
    for case, rid, body in SCORE_BLOCK.findall(md):
        rows = {}
        for line in body.splitlines():
            cells = [c.strip() for c in line.strip().strip("|").split("|")] if line.startswith("| ") else []
            if len(cells) == 3 and cells[0] not in ("軸",) and not cells[0].startswith("---"):
                rows[cells[0]] = (cells[1], cells[2])
        scorer = re.search(r"^- 採点者:[ \t]*(.*)$", body, re.M)
        minutes = re.search(r"^- レビュー時間（分）:[ \t]*(.*)$", body, re.M)
        out[(case, rid)] = {"axes": rows, "scorer": scorer.group(1).strip() if scorer else None,
                            "minutes": minutes.group(1).strip() if minutes else None}
    return out


def structure_problems(md):
    problems = [] if MARK in md else ["pilot-scoring の印が無い"]
    problems += [f"節 {h} が無い" for h in HEADINGS if f"\n{h}\n" not in md]
    if not SHA_LINE.search(md):
        problems.append("pilot-key.json の sha256 の行が無い")
    found = {name for name, _ in RUBRIC_BLOCK.findall(md)}
    problems += [f"rubric の節 {n} が無い" for n in RUBRIC_SECTIONS if n not in found]
    for name, body in RUBRIC_BLOCK.findall(md):
        if name not in RUBRIC_SECTIONS or (name in RUBRIC_EXACT and body != rubric_section(name)):
            problems.append(f"rubric の節 {name} が rubric.md と違う")
    scores = parse_scores(md)
    if not scores:
        problems.append("採点欄が無い")
    for (case, rid), s in scores.items():
        if sorted(s["axes"]) != sorted(axes()):
            problems.append(f"{case}/{rid}: 軸が rubric の 6 軸と違う")
        if s["scorer"] is None or s["minutes"] is None:
            problems.append(f"{case}/{rid}: 採点者かレビュー時間の欄が無い")
    return problems, scores


def blank_problems(md):
    problems, scores = structure_problems(md)
    for (case, rid), s in scores.items():
        if any(p or e for p, e in s["axes"].values()) or s["scorer"] or s["minutes"]:
            problems.append(f"{case}/{rid}: 採点欄が空でない")
    words = sorted(set(ARM_WORDS.findall(RUBRIC_BLOCK.sub("", md))))
    if words:
        problems.append(f"方式の名がある: {words}")
    return problems


def human_problems(md):
    """人の 4 軸・採点者・レビュー時間が埋まり、機械の 2 軸の行が空か機械の根拠であること（check-human）。"""
    problems, scores = structure_problems(md)
    for (case, rid), s in scores.items():
        for axis, (pt, why) in s["axes"].items():
            if axis in MACHINE_AXES:
                if (pt or why) and not (pt in ("0", "1", "2") and why.startswith(MACHINE)):
                    problems.append(f"{case}/{rid}/{axis}: 機械の軸が空でも「{MACHINE}」の根拠と 0〜2 の点でもない")
                continue
            if pt not in ("0", "1", "2"):
                problems.append(f"{case}/{rid}/{axis}: 点が 0〜2 でない（{pt!r}）")
            if not why:
                problems.append(f"{case}/{rid}/{axis}: 根拠が空")
            elif why.startswith(MACHINE):
                problems.append(f"{case}/{rid}/{axis}: 人の軸の根拠が「{MACHINE}」で始まる")
        if not re.fullmatch(r"human:\S+", s["scorer"] or ""):
            problems.append(f"{case}/{rid}: 採点者が human:<名> でない")
        if not re.fullmatch(r"\d+(\.\d+)?", s["minutes"] or ""):
            problems.append(f"{case}/{rid}: レビュー時間（分）が数でない")
    return problems


def scored_problems(md):
    """check-human に加えて、機械の 2 軸が 0〜2 の点と「機械:」の根拠で埋まっていること（check-scored）。"""
    problems = human_problems(md)
    for (case, rid), s in parse_scores(md).items():
        for axis in MACHINE_AXES:
            pt, why = s["axes"].get(axis, ("", ""))
            if not (pt in ("0", "1", "2") and why.startswith(MACHINE)):
                problems.append(f"{case}/{rid}/{axis}: 機械の軸が埋まっていない（0〜2 の点と「{MACHINE}」の根拠）")
    return problems


def cmd_check(path, rule):
    problems = rule(pathlib.Path(path).read_text())
    for p in problems:
        print(f"pilot_score: {p}", file=sys.stderr)
    return 1 if problems else 0


# ---------------------------------------------------------------- machine-axes
def compare_module():
    """検知の定義（DETECT_VERDICTS）は compare --pilot の first_detection と同じものを使う（写さない）。"""
    import importlib.util
    spec = importlib.util.spec_from_file_location("parent_eval_blind_compare", COMPARE)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def pivot_points(diff, wrong_pivots):
    """検知できた時点から検知までの時点の差（符号付き。None = 検知無し）と誤った pivot の数から、rubric の pivot の時点の点。"""
    if wrong_pivots >= 2 or diff is None or diff >= 4:
        return 0
    return 2 if diff <= 1 else 1


def cost_points(ratio):
    """史実に対する比（run と USD の大きい方）から、rubric のコストの点。"""
    return 2 if ratio <= 0.25 else 1 if ratio <= 0.75 else 0


def counted_detection(doc, case, arm, summary, detect_verdicts):
    """metrics の first_detection。その時点の記録が invalid なら検知に数えず、同じ定義で後の時点を見る。"""
    rows = {p: doc["points"][f"{case}/{p}/{arm}"] for p in summary["points"]}
    det = summary["first_detection"]
    if det is None or rows[det].get("status") != "invalid":
        return det
    later = summary["points"][summary["points"].index(det) + 1:]
    return next((p for p in later if rows[p].get("status") != "invalid" and rows[p]["verdict"] in detect_verdicts
                 and rows[p]["next"] != "continue"), None)


def machine_scores(metrics, cases, answer_key, iterations):
    """{(case, id): {"arm", 軸: {"points", "why", "iterations"}}}。規則は反復ごとに当て、id の点は反復の最小。"""
    detect_verdicts = compare_module().DETECT_VERDICTS
    hist = metrics["history"]
    if hist["first_detectable"] != answer_key["first_detectable"]["checkpoint"]:
        raise ValueError(f"metrics.json の history.first_detectable {hist['first_detectable']} が answer key と違う")
    order = {"replay": [p for p, _ in sorted(answer_key["checkpoints"].items(), key=lambda kv: kv[1]["order"])],
             "research": list(driver.research.rc.POINTS)}
    first = {"replay": hist["first_detectable"], "research": RESEARCH_FIRST_DETECTABLE}
    order_name = {"replay": "answer key の order の順", "research": "research_case.py の POINTS の順"}
    first_name = {"replay": "answer key の first_detectable", "research": "ADR D2.2 の反証"}
    # 誤った pivot は answer key の時点ごとの期待から数える。研究ケースは answer key が無く、metrics に wrong_pivots が無い。
    wrong_rule = {"replay": "wrong_pivots 2 以上で 0", "research": "wrong_pivots は数えない（answer key が無い）"}
    out = {}
    for case, ids in cases.items():
        for rid, arm in sorted(ids.items()):
            pivot, cost = {}, {}
            for it in iterations:
                doc = metrics["iterations"].get(it) or {}
                summary = (doc.get("arms", {}).get(arm) or {}).get(case)
                if summary is None:
                    raise ValueError(f"metrics.json has no {it} arm summary for {case}/{rid}")
                det = counted_detection(doc, case, arm, summary, detect_verdicts)
                diff = None if det is None else order[case].index(det) - order[case].index(first[case])
                wrong = summary.get("wrong_pivots", 0)
                pivot[it] = {"first_detection": summary["first_detection"], "counted_detection": det, "diff": diff,
                             "wrong_pivots": summary.get("wrong_pivots"), "points": pivot_points(diff, wrong)}
                if case == "replay":
                    u = summary["unneeded_after_first_detectable"]
                    runs, usd = u["worker_runs"] + summary["llm_runs"], round(u["usd"] + summary["usd"], 6)
                    ratio = max(runs / hist["worker_runs"], usd / hist["usd"])
                    cost[it] = {"history_runs": u["worker_runs"], "history_usd": u["usd"], "llm_runs": summary["llm_runs"],
                                "usd": summary["usd"], "ratio": round(ratio, 6), "points": cost_points(ratio)}
                else:
                    cost[it] = {"llm_runs": summary["llm_runs"], "usd": summary["usd"], "ratio": None, "points": 0}

            def detected(v):
                if v["counted_detection"] is None:
                    text = "検知無し"
                else:
                    text = f"検知 {v['counted_detection']}・差 {v['diff']}"
                if v["counted_detection"] != v["first_detection"]:
                    text += f"（invalid の記録の検知 {v['first_detection']} を数えない）"
                if v["wrong_pivots"] is not None:
                    text += f"・wrong_pivots {v['wrong_pivots']}"
                return text
            pivot_why = (f"{MACHINE} 検知できた時点 {first[case]}（{first_name[case]}）から検知までの時点の差（{order_name[case]}、符号付き）"
                         f"が 1 以下で 2・2〜3 で 1・4 以上か検知無しで 0、{wrong_rule[case]}、invalid の記録は検知に数えない、"
                         "反復の最小。" + "。".join(f"{it}: {detected(v)} → {v['points']}" for it, v in pivot.items()))
            if case == "replay":
                cost_why = (f"{MACHINE} (史実の検知できた時点からの worker run・USD ＋ LLM run・USD) ÷ 史実 {hist['worker_runs']} run・"
                            f"{hist['usd']} USD の大きい方が 25 % 以下で 2・75 % 以下で 1・超えたら 0、反復の最小。"
                            + "。".join(f"{it}: {v['history_runs']}＋{v['llm_runs']} run・{v['history_usd']}＋{v['usd']} USD → "
                                       f"{v['ratio'] * 100:.2f} % → {v['points']}" for it, v in cost.items()))
            else:
                cost_why = (f"{MACHINE} 研究ケースは史実の基準が無い・未確認のため 0。"
                            + "。".join(f"{it}: LLM run {v['llm_runs']}・{v['usd']} USD" for it, v in cost.items()))
            out[(case, rid)] = {
                "arm": arm,
                AXIS["pivot"]: {"points": min(v["points"] for v in pivot.values()), "why": pivot_why, "iterations": pivot},
                AXIS["cost"]: {"points": min(v["points"] for v in cost.values()), "why": cost_why, "iterations": cost}}
    return out


def fill_machine(md, scores):
    """採点欄の機械の 2 軸の行だけを点と根拠で置き換える（ほかの byte を変えない）。"""
    def block(m):
        res = scores[(m.group(1), m.group(2))]
        lines = []
        for line in m.group(3).split("\n"):
            cells = [c.strip() for c in line.strip().strip("|").split("|")] if line.startswith("| ") else []
            if len(cells) == 3 and cells[0] in MACHINE_AXES:
                line = f"| {cells[0]} | {res[cells[0]]['points']} | {res[cells[0]]['why']} |"
            lines.append(line)
        return m.group(0)[:m.start(3) - m.start(0)] + "\n".join(lines) + m.group(0)[m.end(3) - m.start(0):]
    return SCORE_BLOCK.sub(block, md)


def cmd_machine_axes(a):
    path = pathlib.Path(a.scoring)
    md = path.read_text()
    problems = human_problems(md)
    if problems:
        return fail(f"{path}: check-human を通らない: {problems}")
    key_bytes = pathlib.Path(a.key).read_bytes()
    key_sha = sha256_bytes(key_bytes)
    m = SHA_LINE.search(md)
    if not m or m.group(1) != key_sha:
        return fail(f"{path}: pilot-key.json の sha256 {key_sha} が採点の file の値 {m.group(1) if m else None} と一致しない")
    key = json.loads(key_bytes)
    metrics_bytes = pathlib.Path(a.metrics).read_bytes()
    if sha256_bytes(metrics_bytes) != key.get("metrics_sha256"):
        return fail(f"{a.metrics}: sha256 が pilot-key.json の metrics_sha256 と一致しない（材料を作った metrics.json を渡す）")
    metrics = json.loads(metrics_bytes)
    answer_key_bytes = pathlib.Path(a.answer_key).read_bytes()
    if sha256_bytes(answer_key_bytes) != metrics.get("answer_key_sha256"):
        return fail(f"{a.answer_key}: sha256 が metrics.json の answer_key_sha256 と一致しない")
    ids = {(case, rid) for case, rid, _ in SCORE_BLOCK.findall(md)}
    keyed = {(case, rid) for case, rids in key["cases"].items() for rid in rids}
    if ids != keyed:
        return fail(f"{path}: 採点欄の id が pilot-key.json と違う（欄だけ {sorted(ids - keyed)}、key だけ {sorted(keyed - ids)}）")
    scores = machine_scores(metrics, key["cases"], json.loads(answer_key_bytes), key["iterations"])
    new = fill_machine(md, scores)
    problems = scored_problems(new)
    if problems:
        return fail(f"{path}: 機械の 2 軸を埋めた後も check-scored を通らない: {problems}")
    if new != md:
        path.write_text(new)
    if a.out:
        doc = {"key_sha256": key_sha, "metrics_sha256": key["metrics_sha256"], "answer_key_sha256": metrics["answer_key_sha256"],
               "history": metrics["history"], "first_detectable": {"replay": metrics["history"]["first_detectable"],
                                                                   "research": RESEARCH_FIRST_DETECTABLE},
               "iterations": key["iterations"], "machine_axes": list(MACHINE_AXES),
               "ids": {f"{case}/{rid}": {"case": case, "id": rid, **v} for (case, rid), v in sorted(scores.items())}}
        pathlib.Path(a.out).write_text(json.dumps(doc, ensure_ascii=False, indent=2, sort_keys=True) + "\n")
    for (case, rid), v in sorted(scores.items()):
        print(f"pilot_score: {case}/{rid}: " + "、".join(f"{ax} {v[ax]['points']}" for ax in MACHINE_AXES))
    return 0


# ---------------------------------------------------------------- tally
def judge(totals, points, cases):
    """rubric.md の合格線（採点者ごと）。totals[arm] = 両事例の合計点、points[(case, arm)] = {軸: 点}。"""
    out = {}
    disq = {arm: any(p.get(AXIS["execution"]) == 2 and p.get(AXIS["goal"]) == 0
                     for (c, a), p in points.items() if a == arm) for arm in driver.ARMS}
    for arm in ("b", "c"):
        conds = {case: (points.get((case, arm), {}).get(AXIS["pivot"]) == 2
                        and (points.get((case, arm), {}).get(AXIS["unsupported"]) or 0) >= 1
                        and points.get((case, arm), {}).get(AXIS["goal"]) not in (0, None)) for case in cases}
        both = all(conds.get(c, False) for c in driver.CASES)
        diff = totals.get(arm, 0) - totals.get("a", 0)
        out[arm] = {"conditions_by_case": conds, "both_cases": both, "total_minus_a": diff,
                    "disqualified": disq[arm], "pass": both and diff >= 3 and not disq[arm]}
    out["a"] = {"disqualified": disq["a"]}
    return out


def cmd_tally(a):
    key_bytes = pathlib.Path(a.key).read_bytes()
    key_sha = sha256_bytes(key_bytes)
    key = json.loads(key_bytes)["cases"]
    per_scorer = {}
    for path in a.scoring:
        md = pathlib.Path(path).read_text()
        m = SHA_LINE.search(md)
        if not m or m.group(1) != key_sha:
            return fail(f"{path}: pilot-key.json の sha256 {key_sha} が採点の file の値 {m.group(1) if m else None} と一致しない")
        problems = scored_problems(md)
        if problems:
            return fail(f"{path}: 採点が埋まっていない: {problems}")
        for (case, rid), s in parse_scores(md).items():
            arm = (key.get(case) or {}).get(rid)
            if arm is None:
                return fail(f"{path}: id {case}/{rid} is not in the key")
            pts = {axis: int(p) for axis, (p, _) in s["axes"].items()}
            per_scorer.setdefault(s["scorer"], {})[(case, arm)] = {"points": pts, "minutes": float(s["minutes"])}
    result = {"key_sha256": key_sha, "machine_axes": list(MACHINE_AXES), "scorers": {}}
    for scorer, rows in sorted(per_scorer.items()):
        points = {k: v["points"] for k, v in rows.items()}
        totals = {}
        for (case, arm), pts in points.items():
            totals[arm] = totals.get(arm, 0) + sum(pts.values())
        cases = sorted({c for c, _ in points})
        result["scorers"][scorer] = {
            "points": {f"{c}/{arm}": pts for (c, arm), pts in sorted(points.items())},
            "minutes": {f"{c}/{arm}": v["minutes"] for (c, arm), v in sorted(rows.items())},
            "totals": totals, "judgement": judge(totals, points, cases)}
    names = sorted(result["scorers"])
    if len(names) >= 2:
        s1, s2 = (result["scorers"][n] for n in names[:2])
        result["axis_gaps_over_1"] = sorted(f"{k}/{ax}" for k, pts in s1["points"].items()
                                            for ax, p in pts.items() if abs(p - s2["points"].get(k, {}).get(ax, p)) > 1)
        result["rescore"] = any(abs(s1["totals"].get(arm, 0) - s2["totals"].get(arm, 0)) > 4 for arm in driver.ARMS)
    print("| 事例 | 方式 | 採点者 | " + " | ".join(f"{ax}（機械）" if ax in MACHINE_AXES else ax for ax in axes()) + " | 計 | 分 |")
    print("|---|---|---|" + "---|" * (len(axes()) + 2))
    for scorer, s in result["scorers"].items():
        for k, pts in s["points"].items():
            case, arm = k.split("/")
            print(f"| {case} | {arm} | {scorer} | " + " | ".join(str(pts.get(ax, "")) for ax in axes())
                  + f" | {sum(pts.values())} | {s['minutes'][k]} |")
        for arm in ("b", "c"):
            j = s["judgement"][arm]
            print(f"合格線 {scorer} 方式 {arm}: {'満たす' if j['pass'] else '満たさない'}（両事例の条件 {j['both_cases']}、"
                  f"a との差 {j['total_minus_a']}、失格 {j['disqualified']}）")
    if a.out:
        pathlib.Path(a.out).write_text(json.dumps(result, ensure_ascii=False, indent=2, sort_keys=True) + "\n")
    return 0


def main(argv):
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    b = sub.add_parser("blind")
    b.add_argument("--runs", action="append", required=True)
    b.add_argument("--metrics", required=True)
    b.add_argument("--out", required=True)
    b.add_argument("--key-out", required=True)
    b.add_argument("--seed", type=int)
    b.add_argument("--repo-copy")
    for name in ("check-blank", "check-human", "check-scored"):
        sub.add_parser(name).add_argument("md")
    m = sub.add_parser("machine-axes")
    m.add_argument("--scoring", required=True)
    m.add_argument("--key", required=True)
    m.add_argument("--metrics", required=True)
    m.add_argument("--answer-key", required=True)
    m.add_argument("--out")
    t = sub.add_parser("tally")
    t.add_argument("--scoring", action="append", required=True)
    t.add_argument("--key", required=True)
    t.add_argument("--out")
    a = ap.parse_args(argv)
    try:
        if a.cmd == "blind":
            return cmd_blind(a)
        if a.cmd == "check-blank":
            return cmd_check(a.md, blank_problems)
        if a.cmd == "check-human":
            return cmd_check(a.md, human_problems)
        if a.cmd == "check-scored":
            return cmd_check(a.md, scored_problems)
        if a.cmd == "machine-axes":
            return cmd_machine_axes(a)
        return cmd_tally(a)
    except (OSError, ValueError, KeyError) as e:
        return fail(f"{e.__class__.__name__}: {e}")


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
