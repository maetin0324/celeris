#!/usr/bin/env python3
"""対照 pilot の研究ケース（ADR 2026-11-24-strategy-pilot-ablation D2.2）: c-results.md から時点の slice と seed の spec を作る。

節の切り出し・slice・未来の語の検査・sha256 は子 0 の research_case.py（sections・slices・leaks・SENTINELS・SHA）を path で
import して使い、写さない（規約 2）。本文は実行時に --research-src から読み、repo に写さない（D9）。

    python3 -I research.py check-sha --src <c-results.md>      # exit 0 = research_case.SHA と一致、2 = 不一致
    python3 -I research.py check-goal --src <c-results.md> --points 11.16,11.17,11.20 [--written-before <前の版>]
        # exit 0 = 各時点の build が Leak 無しで、goal・Outcome に 11.15 節の見出しも最初の段落も無い。1 = どれかが落ちた。
        # 出力は節の番号と件数だけ（本文を出さない）。--written-before は出典の節が前の版と同じ文かも見る。

seed の規約（D2.2。cutoff 以前の節だけを使う）:
- 時点 p の子は 11.15 節から p 節までの各節（終端の子 1 件 = 1 節）。子の title は節の見出し、子の報告は節の本文、受け入れ条件は
  「節の測定の結果と解釈を記録する」の 1 件（review の記録は c-results.md に無いので、判定の event は置かない）。
- 案件の目的と承認済み Roadmap の Outcome は、task C の目的のうち 11.15 節より前に書かれた問いから作る（ADR D2.2 の付記 1）。
  出典は `GOAL_SOURCES` の節だけで、0 節（後で書かれた結論）と 11.15 節以降の節は使わない:
  問い（Outcome の title）= 11.2 節の「人の答え」の箇条のうち「止ま」を含む文、
  判定方法（Outcome の acceptance）= 11.5 節の「停止の割合」を含む箇条、背景 = 11.1 節の「人の決定」の箇条。
  出典の箇条か文が無ければ ValueError（代わりの節を探さない）。11.15 節の見出しと見出しの後の最初の段落は、問いにも判定方法にもしない。
- 時点 p の入力（slice・goal・子）に、research_case.leaks の語（初出が p より後の SENTINELS）と、p より後の節の見出しがあれば
  die（exit 1）。seed も run も起こさない（D4.3 の run の前の検査）。
"""
import hashlib
import importlib.util
import json
import pathlib
import sys

REPO = pathlib.Path(__file__).resolve().parents[3]
RESEARCH_CASE = REPO / "agent-docs/progress/2026-11-21-strategy-v1-0/research/research_case.py"
# pilot の時点（D2.2）と、方式 c の版 v1 を書く段の slice（最初の時点より前。D1.3）。
POINTS = ("11.16", "11.17", "11.20")
STRATEGY_POINT = "11.15"
SLICE_FILE = "results-slice.md"
OUTCOME_ID = "research-question"
# goal と Outcome の出典（ADR D2.2 の付記 1）: (役割, 節, 箇条の印, 文の印)。文の印が None なら箇条の全文を使う。
# どの節も 11.15 節より前に書かれ、後で書き換えられていない（付記 1 の根拠）。
GOAL_SOURCES = (
    ("question", "11.2", "人の答え", "止ま"),
    ("method", "11.5", "停止の割合", None),
    ("background", "11.1", "人の決定", None),
)


def load():
    spec = importlib.util.spec_from_file_location("research_case", RESEARCH_CASE)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


rc = load()


class Leak(Exception):
    pass


def sha256_of(path):
    return hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()


def heading(section):
    """節の見出しの行から `#` を除いた文（例: `11.15 run 26 ...`）。"""
    return section.splitlines()[0].lstrip("#").strip()


def first_paragraph(section):
    """見出しの後の最初の段落（空行で区切る）。"""
    lines, out = section.splitlines()[1:], []
    for line in lines:
        if line.strip():
            out.append(line.strip())
        elif out:
            break
    return " ".join(out)


def later_headings(secs, point):
    """時点 p より後の節（POINTS の後の時点と、文書で p の節より後に現れる番号の節）の見出し。"""
    keys = list(secs)
    after = keys[keys.index(point) + 1:]
    return [heading(secs[k]) for k in after]


def bullet(section, mark):
    """節の中で `mark` を含む最初の箇条（`- ` の行。続きの行は無い）の本文。無ければ None。"""
    for line in section.splitlines():
        if line.startswith("- ") and mark in line:
            return line[2:].strip()
    return None


def goal_parts(secs):
    """GOAL_SOURCES の節から問い・判定方法・背景を作る（ADR D2.2 の付記 1）。出典が無い・11.15 節以降の節なら ValueError。"""
    keys = list(secs)
    cutoff = keys.index(STRATEGY_POINT)
    parts = {}
    for role, key, mark, word in GOAL_SOURCES:
        if key not in secs or key == "0" or keys.index(key) >= cutoff:
            raise ValueError(f"{role}: {key} 節が無いか、11.15 節より前の節ではない")
        text = bullet(secs[key], mark)
        if text is None:
            raise ValueError(f"{role}: {key} 節に印の箇条が無い（goal と Outcome を作れない）")
        if word is not None:
            text = "".join(t + "。" for t in text.split("。") if word in t)
            if not text:
                raise ValueError(f"{role}: {key} 節の印の箇条に印の文が無い")
        parts[role] = text
    return parts


def build(text, point):
    """時点 p の slice・目的・Outcome・子。漏れがあれば Leak。"""
    if point not in rc.POINTS:
        raise ValueError(f"point {point} is not in research_case.POINTS")
    secs = rc.sections(text)
    sl = rc.slices(secs)[point]
    g = goal_parts(secs)
    src = "・".join(f"{key} 節" for _, key, _, _ in GOAL_SOURCES)
    goal = (f"# 研究ケース（task C の 11.15 節より前に書かれた問い。出典 {src}）\n\n目的：{g['question']}\n"
            f"判定方法：{g['method']}\n背景：{g['background']}\n")
    outcome = {"id": OUTCOME_ID, "title": g["question"], "acceptance": [g["method"]]}
    keys = rc.POINTS[: rc.POINTS.index(point) + 1]
    children = [{"key": k, "title": heading(secs[k]), "report": secs[k],
                 "criteria": [f"{k} 節の測定の結果と解釈を c-results.md に記録する"]} for k in keys]
    blob = "\n".join([sl, goal, json.dumps(outcome, ensure_ascii=False),
                      json.dumps(children, ensure_ascii=False)])
    found = rc.leaks(point, blob) + [f"{point}: later heading {h!r}" for h in later_headings(secs, point) if h in blob]
    if found:
        raise Leak("; ".join(found))
    meta = {"case": "research", "point": point, "sections": list(keys),
            "slice_sha256": hashlib.sha256(sl.encode()).hexdigest()}
    return {"slice": sl, "goal": goal, "outcome": outcome, "children": children, "meta": meta}


def cutoff_hits(secs, b):
    """goal と Outcome に入った 11.15 節の見出し（番号の有無とも）と最初の段落の数。"""
    first = secs[STRATEGY_POINT]
    head = heading(first)
    marks = [head, head.split(" ", 1)[1] if " " in head else head, first_paragraph(first)]
    text = b["goal"] + json.dumps(b["outcome"], ensure_ascii=False)
    return sum(1 for m in marks if m and m in text)


def check_goal(src, points, before=None):
    """各時点の build を流し、Leak と 11.15 節の混入を数える。出力は節の番号と件数だけ。全部 0 なら 0、他は 1。"""
    text = pathlib.Path(src).read_text(encoding="utf-8")
    secs = rc.sections(text)
    bad = 0
    for p in points:
        try:
            b = build(text, p)
        except Leak as e:
            print(f"{p}: leak {len(str(e).split('; '))} 件")
            bad += 1
            continue
        hits = cutoff_hits(secs, b)
        print(f"{p}: sections {b['meta']['sections'][0]}..{b['meta']['sections'][-1]}（{len(b['children'])} 節）、"
              f"leak 0 件、11.15 節の見出し・最初の段落 {hits} 件")
        bad += hits > 0
    keys = [key for _, key, _, _ in GOAL_SOURCES]
    print("goal の出典: " + "・".join(keys) + " 節（11.15 節より前）")
    if before is not None:
        old = rc.sections(pathlib.Path(before).read_text(encoding="utf-8"))
        same = [k for k in keys if old.get(k) == secs[k]]
        print(f"前の版と同じ文の出典: {len(same)} / {len(keys)} 節")
        bad += len(same) != len(keys)
    return 0 if bad == 0 else 1


def files(src, point):
    """作業 tree に置く file（goal.md・slice の本文・slice.json）と seed の spec の `research`。"""
    b = build(pathlib.Path(src).read_text(encoding="utf-8"), point)
    tree = {"goal.md": b["goal"].encode(), SLICE_FILE: b["slice"].encode(),
            "slice.json": (json.dumps(b["meta"], ensure_ascii=False, indent=2) + "\n").encode()}
    spec = {"goal": b["goal"], "outcome": b["outcome"], "children": b["children"]}
    return tree, spec


USAGE = ("usage: research.py check-sha --src <c-results.md>\n"
         "       research.py check-goal --src <c-results.md> --points 11.16,11.17,11.20 [--written-before <前の版>]")


def main(argv):
    if len(argv) == 3 and argv[0] == "check-sha" and argv[1] == "--src":
        got = sha256_of(argv[2])
        if got != rc.SHA:
            print(f"research: --research-src sha256 {got} != research_case.SHA {rc.SHA}（版が違う）", file=sys.stderr)
            return 2
        return 0
    if argv[:1] == ["check-goal"] and len(argv) in (5, 7):
        opts = dict(zip(argv[1::2], argv[2::2]))
        points = opts.get("--points", "").split(",")
        if set(opts) - {"--src", "--points", "--written-before"} or "--src" not in opts or not set(points) <= set(POINTS):
            print(USAGE, file=sys.stderr)
            return 2
        return check_goal(opts["--src"], points, opts.get("--written-before"))
    print(USAGE, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
