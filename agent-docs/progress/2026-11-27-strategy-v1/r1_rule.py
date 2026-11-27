#!/usr/bin/env python3
"""R1 の最小の識別実験（人の 2026-11-27 の決定。親 01M7XQ9PS0AFKKMRJ7P5ZMKQQE の task のコメント）。

決定的な escalate の規則を、子 0 の replay の 12 slice（アーキv4）と研究ケースの 7 slice（BenchFS task C の 11.15〜11.21 節）に当て、
発火の時点と誤検知を数える。LLM・network・本番 DB は使わない（stdlib と git だけ。llm_calls は 0）。

    python3 -I agent-docs/progress/2026-11-27-strategy-v1/r1_rule.py run      # slice を tempfile の dir に作り、r1-result.json を書く
    python3 -I agent-docs/progress/2026-11-27-strategy-v1/r1_rule.py verify   # 再計算して r1-result.json と byte 一致・欄・k=3 を確かめる（exit 0/1）

規則（計画で事前に固定した。結果を見て変えない）:
- outcome_reached: Outcome が 1 件以上あり、全部が到達のときだけ真。到達の記録が無ければ偽（子 8 の goal_reached・D6 と同じ定義）。
  - replay: Outcome は goal.md から作る 1 件（子 8 の seed の outcome_spec と同じ数）。到達の記録は親 P の done への遷移か
    reviewer_passed の event（子 8 の seed が cutoff 以前に無いことを確かめた記録と同じ種類）。
  - 研究: slice（c-results.md の 11.15〜11.k 節）に Outcome とその到達の記録は無い。Outcome 0 件なので偽。
- terminal_children:
  - replay: slice.json の children_done の数（親 P は子ではないので除く）。
  - 研究: 11.15〜11.k 節のうち、実験の結果を報告した節の数。節の本文（見出しの行を含む）に PBS の job の番号（5 桁の数）と、
    測った数の比（`8/24`・`1 / 488` の形）の両方があれば報告した節とする。正規表現は節の本文だけから決め、sentinel と答えの表を使わない。
- no_progress_streak: 到達した Outcome の数が最後に増えてからの terminal_children。増えていなければ terminal_children。
- escalate(k): outcome_reached が偽 かつ terminal_children >= k かつ no_progress_streak >= k。判定は K = 3、K = 2・4 は感度（判定に使わない）。

規則の関数（measure・escalate）は slice の dir だけを読む。answer key（子 0 の replay/answer_key.json）と研究ケースの時点の表
（research/research-case.md）は、19 点の発火をすべて出した後の採点（score_*）だけが読む。

誤検知と反証（採点。人の決定の反証条件をそのまま使う）:
- replay: answer key の ideal_verdict が off_track の点を「検知してよい点」とし（first_detectable = E-mid から後）、それ以外の点の発火を誤検知とする。
- 研究: 時点の表で「合理的な次の手」に「改め」（結論を改める）が出る最初の行の時点を基準（11.20）とし、それより前の点の発火を誤検知とする。
- 反証（K = 3）: A-done・C-mid・C-done のうち 2 点以上で発火する、または A-mid〜F-done の 10 点で一度も発火しない。当たれば falsified。
"""
from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import re
import shutil
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
CHILD0 = REPO / "agent-docs/progress/2026-11-21-strategy-v1-0"
REPLAY_PY = CHILD0 / "replay/replay.py"
RESEARCH_PY = CHILD0 / "research/research_case.py"
ANSWER_KEY = CHILD0 / "replay/answer_key.json"
RESEARCH_TABLE = CHILD0 / "research/research-case.md"
RESULT = HERE / "r1-result.json"
SCHEMA = "celeris.r1-escalate-result/1"

K = 3
SENSITIVITY = (2, 4)
KS = (2, 3, 4)
PARENT = "P"
RESEARCH_KIND = "research-sections"
RESULTS_FILE = "results-slice.md"
# 研究の節が実験の結果を報告したか: PBS の job の番号と、測った数の比の両方。
JOB_RE = re.compile(r"(?<![\d.])\d{5}(?![\d.])")
RATIO_RE = re.compile(r"(?<![\d.])\d+\s*/\s*\d+(?![\d.])")
# 人の決定の反証条件（K = 3）。
FALSIFY_EARLY = ("A-done", "C-mid", "C-done")
FALSIFY_WINDOW = ("A-mid", "A-done", "C-mid", "C-done", "D-mid", "E-mid", "D-done", "E-done", "F-mid", "F-done")
REQUIRED = {
    "top": ("schema", "rule", "llm_calls", "inputs", "replay", "research", "falsification", "falsified"),
    "rule": ("k", "sensitivity"),
    "point": ("id", "order", "outcomes", "outcomes_reached", "outcome_reached", "terminal_children", "no_progress_streak", "escalate"),
    "case": ("points", "score"),
    "score": ("reference", "warranted", "by_k"),
    "by_k": ("fired", "first_fire", "lag_points", "false_positives", "false_positive_count", "missed"),
}


def load(name: str, path: Path):
    """子 0 の script を path から読む（写さない）。"""
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


replay = load("strategy_v1_0_replay", REPLAY_PY)
rc = load("strategy_v1_0_research_case", RESEARCH_PY)


# ---------------------------------------------------------------- 規則（slice の dir だけを読む）
def measure(slice_dir: Path) -> dict:
    meta = json.loads((slice_dir / "slice.json").read_text(encoding="utf-8"))
    if "children_done" in meta:
        return measure_replay(slice_dir, meta)
    if meta.get("kind") == RESEARCH_KIND:
        return measure_research(slice_dir, meta)
    raise ValueError(f"{slice_dir.name}: slice.json の形が分からない")


def streak_after_reach(done_ts: list[str], reach_ts: list[str]) -> int:
    """到達した Outcome の数が最後に増えた時刻より後に終端になった子の数。増えていなければ全数。"""
    if not reach_ts:
        return len(done_ts)
    last = max(reach_ts)
    return sum(1 for t in done_ts if t > last)


def measure_replay(slice_dir: Path, meta: dict) -> dict:
    events = [json.loads(line) for line in (slice_dir / "events.jsonl").read_text(encoding="utf-8").splitlines() if line.strip()]
    outcomes = 1 if (slice_dir / "goal.md").is_file() else 0
    reach = sorted(
        replay.norm(e["ts"])
        for e in events
        if e["task"] == PARENT
        and ((e["type"] == "transitioned" and e.get("to") == "done") or e["type"] == "reviewer_passed")
    )
    # Outcome は 1 件なので、到達の数が増えるのは最初の到達の記録の時だけ。
    reach_increase = reach[:1] if outcomes else []
    reached = len(reach_increase)
    children = [c for c in meta["children_done"] if c != PARENT]
    done_ts = []
    for c in children:
        ts = [replay.norm(e["ts"]) for e in events if e["task"] == c and e["type"] == "integration_merged"]
        if not ts:
            raise ValueError(f"{slice_dir.name}: {c} の integration_merged が events.jsonl に無い")
        done_ts.append(min(ts))
    return {
        "id": meta["id"],
        "cutoff": meta["cutoff"],
        "outcomes": outcomes,
        "outcomes_reached": reached,
        "outcome_reached": outcomes >= 1 and reached == outcomes,
        "children": children,
        "terminal_children": len(children),
        "no_progress_streak": streak_after_reach(done_ts, reach_increase),
    }


def reported(section: str) -> bool:
    return bool(JOB_RE.search(section)) and bool(RATIO_RE.search(section))


def measure_research(slice_dir: Path, meta: dict) -> dict:
    text = (slice_dir / RESULTS_FILE).read_text(encoding="utf-8")
    secs = rc.sections(text)
    hits = [k for k, body in secs.items() if reported(body)]
    return {
        "id": meta["point"],
        "outcomes": 0,
        "outcomes_reached": 0,
        "outcome_reached": False,
        "sections": list(secs),
        "sections_reported": hits,
        "terminal_children": len(hits),
        # 到達の記録が無いので、到達の数は一度も増えていない。
        "no_progress_streak": len(hits),
    }


def escalate(m: dict, k: int) -> bool:
    return (not m["outcome_reached"]) and m["terminal_children"] >= k and m["no_progress_streak"] >= k


def evaluate(slice_dir: Path) -> dict:
    m = measure(slice_dir)
    m["escalate"] = {str(k): escalate(m, k) for k in KS}
    return m


# ---------------------------------------------------------------- slice を作る（子 0 の build と sections）
def build_replay(root: Path) -> list[Path]:
    replay.cmd_build(str(root))
    got = replay.manifest_lines(root)
    want = (CHILD0 / "replay/manifest.sha256").read_text(encoding="utf-8").splitlines()
    if got != want:
        raise SystemExit("r1: replay の slice が子 0 の manifest.sha256 と一致しない")
    dirs = [p for p in root.iterdir() if p.is_dir()]
    return sorted(dirs, key=lambda p: json.loads((p / "slice.json").read_text(encoding="utf-8"))["cutoff"])


def build_research(root: Path) -> list[Path]:
    data = Path(rc.SRC).read_bytes()
    if hashlib.sha256(data).hexdigest() != rc.SHA:
        raise SystemExit(f"r1: {rc.SRC} の sha256 が research_case.SHA と違う（版が違う）")
    secs = rc.sections(data.decode("utf-8"))
    out = []
    for p, text in rc.slices(secs).items():
        if rc.leaks(p, text):
            raise SystemExit(f"r1: 研究の slice {p} に未来の語がある")
        d = root / p
        d.mkdir(parents=True)
        (d / RESULTS_FILE).write_text(text, encoding="utf-8")
        meta = {"kind": RESEARCH_KIND, "point": p, "slice_sha256": hashlib.sha256(text.encode()).hexdigest()}
        (d / "slice.json").write_text(json.dumps(meta, ensure_ascii=False, sort_keys=True) + "\n", encoding="utf-8")
        out.append(d)
    return out


def fire_all(dirs: list[Path]) -> list[dict]:
    pts = []
    for i, d in enumerate(dirs, start=1):
        m = evaluate(d)
        m["order"] = i
        pts.append(m)
    return pts


# ---------------------------------------------------------------- 採点（発火をすべて出した後にだけ読む）
def research_reference() -> tuple[str, list[str]]:
    """時点の表の行（時点, 次の手）から、次の手に「改め」が出る最初の行の時点を返す。"""
    rows = []
    for line in RESEARCH_TABLE.read_text(encoding="utf-8").splitlines():
        m = re.match(r"^\| (\d+\.\d+) の後 \|(.*)\|\s*$", line)
        if m:
            rows.append((m.group(1), m.group(2).split("|")[-1].strip()))
    for p, nxt in rows:
        if "改め" in nxt:
            return p, [r[0] for r in rows]
    raise SystemExit("r1: research-case.md の時点の表に結論を改める行が無い")


def by_k(points: list[dict], warranted: set[str], ref_order: int) -> dict:
    out = {}
    for k in KS:
        fired = [p["id"] for p in points if p["escalate"][str(k)]]
        first = next((p for p in points if p["escalate"][str(k)]), None)
        fp = [i for i in fired if i not in warranted]
        out[str(k)] = {
            "fired": fired,
            "first_fire": first["id"] if first else None,
            "lag_points": (first["order"] - ref_order) if first else None,
            "false_positives": fp,
            "false_positive_count": len(fp),
            "missed": [p["id"] for p in points if p["id"] in warranted and not p["escalate"][str(k)]],
        }
    return out


def score_replay(points: list[dict]) -> dict:
    key = json.loads(ANSWER_KEY.read_text(encoding="utf-8"))
    cps = key["checkpoints"]
    if [p["id"] for p in points] != sorted(cps, key=lambda c: cps[c]["order"]):
        raise SystemExit("r1: slice の時刻の順と answer key の order が違う")
    ref = key["first_detectable"]["checkpoint"]
    warranted = [c for c in sorted(cps, key=lambda c: cps[c]["order"]) if cps[c]["ideal_verdict"] == "off_track"]
    ref_order = next(p["order"] for p in points if p["id"] == ref)
    return {
        "reference": ref,
        "reference_basis": "answer_key.json の first_detectable",
        "warranted": warranted,
        "warranted_basis": "answer_key.json の ideal_verdict が off_track の点",
        "by_k": by_k(points, set(warranted), ref_order),
    }


def score_research(points: list[dict]) -> dict:
    ref, rows = research_reference()
    ref_order = next(p["order"] for p in points if p["id"] == ref)
    warranted = [p["id"] for p in points if p["order"] >= ref_order]
    return {
        "reference": ref,
        "reference_basis": "research-case.md の時点の表で、合理的な次の手に「改め」が出る最初の行",
        "table_rows": rows,
        "warranted": warranted,
        "warranted_basis": "基準の時点とその後の点",
        "by_k": by_k(points, set(warranted), ref_order),
    }


def falsification(replay_points: list[dict]) -> dict:
    fired = {p["id"] for p in replay_points if p["escalate"][str(K)]}
    early = [i for i in FALSIFY_EARLY if i in fired]
    window = [i for i in FALSIFY_WINDOW if i in fired]
    return {
        "k": K,
        "early_points": list(FALSIFY_EARLY),
        "early_fired": early,
        "early_2plus": len(early) >= 2,
        "window": list(FALSIFY_WINDOW),
        "window_fired": window,
        "no_fire_in_window": not window,
    }


# ---------------------------------------------------------------- 実行
def compute() -> dict:
    os.chdir(REPO)  # 子 0 の replay.py は git を作業 dir で呼ぶ
    tmp = Path(tempfile.mkdtemp(prefix="r1-", dir=os.environ.get("TMPDIR")))
    try:
        rdirs = build_replay(tmp / "replay")
        sdirs = build_research(tmp / "research")
        # 規則: 19 点の発火を先にすべて出す（answer key と時点の表はまだ読まない）。
        rpts = fire_all(rdirs)
        spts = fire_all(sdirs)
        replay_manifest = (CHILD0 / "replay/manifest.sha256").read_bytes()
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    # 採点: 発火の後にだけ answer key と時点の表を読む。
    fz = falsification(rpts)
    return {
        "schema": SCHEMA,
        "rule": {
            "k": K,
            "sensitivity": list(SENSITIVITY),
            "escalate": "not outcome_reached and terminal_children >= k and no_progress_streak >= k",
            "research_report_regex": {"job": JOB_RE.pattern, "ratio": RATIO_RE.pattern},
        },
        "llm_calls": 0,
        "inputs": {
            "replay_manifest_sha256": hashlib.sha256(replay_manifest).hexdigest(),
            "replay_slices": len(rpts),
            "research_src_sha256": rc.SHA,
            "research_slices": len(spts),
        },
        "replay": {"points": rpts, "score": score_replay(rpts)},
        "research": {"points": spts, "score": score_research(spts)},
        "falsification": fz,
        "falsified": fz["early_2plus"] or fz["no_fire_in_window"],
    }


def render(result: dict) -> bytes:
    return (json.dumps(result, ensure_ascii=False, sort_keys=True, indent=1) + "\n").encode("utf-8")


def missing_fields(doc: dict) -> list[str]:
    bad = [f"top.{f}" for f in REQUIRED["top"] if f not in doc]
    bad += [f"rule.{f}" for f in REQUIRED["rule"] if f not in doc.get("rule", {})]
    for case, n in (("replay", 12), ("research", 7)):
        c = doc.get(case, {})
        bad += [f"{case}.{f}" for f in REQUIRED["case"] if f not in c]
        pts = c.get("points", [])
        if len(pts) != n:
            bad.append(f"{case}.points: {len(pts)} 点（{n} 点のはず）")
        for p in pts:
            bad += [f"{case}.{p.get('id')}.{f}" for f in REQUIRED["point"] if f not in p]
            if str(K) not in p.get("escalate", {}):
                bad.append(f"{case}.{p.get('id')}.escalate.{K}")
        s = c.get("score", {})
        bad += [f"{case}.score.{f}" for f in REQUIRED["score"] if f not in s]
        for k in KS:
            bk = s.get("by_k", {}).get(str(k))
            if bk is None:
                bad.append(f"{case}.score.by_k.{k}")
                continue
            bad += [f"{case}.score.by_k.{k}.{f}" for f in REQUIRED["by_k"] if f not in bk]
    if not isinstance(doc.get("falsified"), bool):
        bad.append("falsified: bool でない")
    return bad


def cmd_run() -> int:
    RESULT.write_bytes(render(compute()))
    print(f"r1: wrote {RESULT.relative_to(REPO)}")
    return 0


def cmd_verify() -> int:
    ok = True
    if not RESULT.is_file():
        print(f"FAIL {RESULT.relative_to(REPO)} が無い")
        return 1
    want = RESULT.read_bytes()
    doc = json.loads(want)
    bad = missing_fields(doc)
    for b in bad:
        print(f"FAIL 欄が無い: {b}")
    ok &= not bad
    rule = doc.get("rule", {})
    if rule.get("k") != K or rule.get("sensitivity") != list(SENSITIVITY):
        print(f"FAIL rule の k={rule.get('k')} sensitivity={rule.get('sensitivity')}（k=3・[2, 4] のはず）")
        ok = False
    if doc.get("llm_calls") != 0:
        print("FAIL llm_calls が 0 でない")
        ok = False
    got = render(compute())
    if got != want:
        print("FAIL 再計算が r1-result.json と byte で一致しない")
        ok = False
    else:
        print(f"ok   再計算が r1-result.json と byte で一致（{len(got)} byte）")
    print(f"r1 verify: {'ok' if ok else 'FAILED'}（falsified={doc.get('falsified')}）")
    return 0 if ok else 1


def main(argv: list[str]) -> int:
    if argv[1:] == ["run"]:
        return cmd_run()
    if argv[1:] == ["verify"]:
        return cmd_verify()
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
