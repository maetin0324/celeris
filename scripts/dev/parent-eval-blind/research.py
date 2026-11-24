#!/usr/bin/env python3
"""対照 pilot の研究ケース（ADR 2026-11-24-strategy-pilot-ablation D2.2）: c-results.md から時点の slice と seed の spec を作る。

節の切り出し・slice・未来の語の検査・sha256 は子 0 の research_case.py（sections・slices・leaks・SENTINELS・SHA）を path で
import して使い、写さない（規約 2）。本文は実行時に --research-src から読み、repo に写さない（D9）。

    python3 -I research.py check-sha --src <c-results.md>      # exit 0 = research_case.SHA と一致、2 = 不一致

seed の規約（D2.2。cutoff 以前の節だけを使う）:
- 時点 p の子は 11.15 節から p 節までの各節（終端の子 1 件 = 1 節）。子の title は節の見出し、子の報告は節の本文、受け入れ条件は
  「節の測定の結果と解釈を記録する」の 1 件（review の記録は c-results.md に無いので、判定の event は置かない）。
- 案件の目的と承認済み Roadmap の Outcome は 11.15 節（全時点の cutoff 以前）だけから作る: 問い = 11.15 節の見出し、
  判定方法 = 11.15 節の見出しの後の最初の段落。
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


def build(text, point):
    """時点 p の slice・目的・Outcome・子。漏れがあれば Leak。"""
    if point not in rc.POINTS:
        raise ValueError(f"point {point} is not in research_case.POINTS")
    secs = rc.sections(text)
    sl = rc.slices(secs)[point]
    first = secs[rc.POINTS[0]]
    question, method = heading(first), first_paragraph(first)
    if not method:
        raise ValueError(f"{rc.POINTS[0]} 節に見出しの後の段落が無い（判定方法を作れない）")
    goal = (f"# 研究ケース（{rc.POINTS[0]} 節の時点の問い）\n\n目的：{question}。\n"
            f"判定方法：{method}\n")
    outcome = {"id": OUTCOME_ID, "title": question, "acceptance": [method]}
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


def files(src, point):
    """作業 tree に置く file（goal.md・slice の本文・slice.json）と seed の spec の `research`。"""
    b = build(pathlib.Path(src).read_text(encoding="utf-8"), point)
    tree = {"goal.md": b["goal"].encode(), SLICE_FILE: b["slice"].encode(),
            "slice.json": (json.dumps(b["meta"], ensure_ascii=False, indent=2) + "\n").encode()}
    spec = {"goal": b["goal"], "outcome": b["outcome"], "children": b["children"]}
    return tree, spec


def main(argv):
    if len(argv) == 3 and argv[0] == "check-sha" and argv[1] == "--src":
        got = sha256_of(argv[2])
        if got != rc.SHA:
            print(f"research: --research-src sha256 {got} != research_case.SHA {rc.SHA}（版が違う）", file=sys.stderr)
            return 2
        return 0
    print("usage: research.py check-sha --src <c-results.md>", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
