#!/usr/bin/env python3
"""R1 の識別実験（計画 v2 の r1-bench）: no-progress の連続が Outcome の途中到達で 0 に戻るかを long-horizon の記録で確かめる。

人の決定 after-r1（b replan）の付記の実験。入力は commit 済みの long-horizon bench の記録
agent-docs/progress/2026-11-04-long-horizon/{a8b/replay,a9/stub,a9/llm}/facts/*.json（44 trial）と、同じ dir の arm-map（case・arm）。
LLM・network・本番 DB は使わない（stdlib だけ。llm_calls は 0）。記録と子 0 の fixture は読むだけ。

    python3 -I agent-docs/progress/2026-11-27-strategy-v1/r1_bench.py run      # r1-bench.json を書く
    python3 -I agent-docs/progress/2026-11-27-strategy-v1/r1_bench.py verify   # 再計算して byte 一致・k=3・欄を確かめる（exit 0/1）

規則は r1_rule.py の streak_after_reach と escalate を importlib で読んで使う（写さない）。

写像（計画で事前に固定した。結果を見て変えない）:
- 案件 = 親、root = 子。root が終端 status（done・failed・cancelled。long_horizon.rs の root_status_stays と同じ集合）に
  なった時点を 1 点とし、時刻の順（同じ時刻は logical_id の順）に並べる。
- outcome_reached: 必要な Outcome が 1 件以上あり、全部がその点の時刻までに到達していれば真。
- terminal_children: その点までに終端になった root の数（点の順番と同じ）。
- no_progress_streak: 到達した必要な Outcome の数が最後に増えた時刻より後に終端になった root の数
  （r1_rule.streak_after_reach。増えていなければ terminal_children）。
- escalate(k): r1_rule.escalate。判定は K = 3、K = 2・4 は感度で判定に使わない。
- 時刻を facts から取る形: root の statuses の要素が {"status", "at"} の object なら、最初の終端の要素の at を終端の時刻とする。
  Outcome ごとの到達は facts の outcomes（{"id", "required", "reached_at"} の列）から取る。
  celeris.long-horizon-facts/1 の statuses は時刻の無い文字列の列で、outcomes の欄は無い（achieved_at は必要な Outcome が
  全部到達した案件単位の時刻だけ）。時刻の無い trial は推測せず（invocations・created_at・achieved_at から補わない）、
  理由を付けて除外する。

判定（事前に固定）:
- 途中到達: 必要な Outcome の到達のうち、その時刻で必要な Outcome がまだ全部は到達していないもの。
- F1: 途中到達のある trial で、到達の直後（到達の時刻までに終端になった root と、その時刻までの到達で計算した値）の
  no_progress_streak が 0 にならない到達がある。
- F2: 必要な Outcome に全部到達した trial のうち、全部の到達の時刻より前の点で K = 3 の escalate が発火した trial が 2 件以上。
- F1 か F2 に当たれば falsified。ただし途中到達のある trial が 3 件未満なら inconclusive とし、falsified にしない。
- 誤検知（arm ごと）: 必要な Outcome に全部到達した trial で、到達より前に発火した点。
"""
from __future__ import annotations

import hashlib
import importlib.util
import json
import sys
from pathlib import Path

sys.dont_write_bytecode = True  # repo に __pycache__ を作らない

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
LH = REPO / "agent-docs/progress/2026-11-04-long-horizon"
SETS = ("a8b/replay", "a9/stub", "a9/llm")
R1_RULE = HERE / "r1_rule.py"
R1_RESULT = HERE / "r1-result.json"
RESULT = HERE / "r1-bench.json"
SCHEMA = "celeris.r1-bench-result/1"
FACTS_SCHEMA = "celeris.long-horizon-facts/1"

K = 3
SENSITIVITY = (2, 4)
KS = (2, 3, 4)
MIN_MIDWAY_TRIALS = 3
F2_MIN_TRIALS = 2
TERMINAL = ("done", "failed", "cancelled")
EXPECTED_TRIALS = 44

REASONS = {
    "no_roots": "roots が 0 件で、終端になった root が無い（点が作れない）",
    "no_terminal_root": "終端 status の root が無い（点が作れない）",
    "root_terminal_time_missing": "root の statuses が時刻の無い列で、終端になった時刻が facts に無い",
    "outcome_reach_time_missing": "Outcome ごとの到達の時刻（outcomes の reached_at）が facts に無い（achieved_at は案件単位の時刻だけ）",
    "no_required_outcome": "必要な Outcome が 0 件",
}
REQUIRED = {
    "top": ("schema", "rule", "llm_calls", "inputs", "trials", "counts", "arms", "f1", "f2", "inconclusive", "falsified"),
    "rule": ("k", "sensitivity", "mapping", "judgement"),
    "trial": ("set", "trial", "case", "arm", "seed", "facts_sha256", "included", "exclusion", "evidence", "points", "reach"),
}


def load(name: str, path: Path):
    """r1_rule.py を path から読む（写さない）。"""
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


r1 = load("strategy_v1_r1_rule", R1_RULE)


# ---------------------------------------------------------------- facts → 時刻の列（時刻が無ければ除外の理由）
def status_name(s) -> str | None:
    if isinstance(s, str):
        return s
    if isinstance(s, dict):
        return s.get("status")
    return None


def terminal_time(root: dict) -> tuple[bool, int | None]:
    """(終端か, 終端の時刻)。最初の終端の要素に at が無ければ時刻は None。"""
    for s in root.get("statuses", []):
        if status_name(s) in TERMINAL:
            at = s.get("at") if isinstance(s, dict) else None
            return True, at if isinstance(at, int) and not isinstance(at, bool) else None
    return False, None


def status_shape(roots: list[dict]) -> str:
    shapes = set()
    for r in roots:
        for s in r.get("statuses", []):
            shapes.add("object" if isinstance(s, dict) else type(s).__name__)
    return "+".join(sorted(shapes)) if shapes else "none"


def extract(facts: dict) -> tuple[dict | None, list[str], dict]:
    """facts から時刻の列を取る。取れなければ (None, 除外の理由の code, 根拠)。推測で補わない。"""
    roots = facts.get("roots") or []
    term = [(r, *terminal_time(r)) for r in roots]
    terminal = [(r, at) for r, is_term, at in term if is_term]
    outcomes = facts.get("outcomes")
    evidence = {
        "roots": len(roots),
        "terminal_roots": len(terminal),
        "terminal_roots_with_time": sum(1 for _, at in terminal if at is not None),
        "statuses_shape": status_shape(roots),
        "has_outcomes_field": isinstance(outcomes, list),
        "achieved_at": facts.get("achieved_at"),
    }
    reasons = []
    if not roots:
        reasons.append("no_roots")
    elif not terminal:
        reasons.append("no_terminal_root")
    elif any(at is None for _, at in terminal):
        reasons.append("root_terminal_time_missing")
    valid_outcomes = isinstance(outcomes, list) and all(
        isinstance(o, dict) and isinstance(o.get("id"), str) and isinstance(o.get("required"), bool) and "reached_at" in o
        for o in outcomes
    )
    if not valid_outcomes:
        reasons.append("outcome_reach_time_missing")
    else:
        required = [o for o in outcomes if o["required"]]
        if not required:
            reasons.append("no_required_outcome")
    if reasons:
        return None, reasons, evidence
    required = sorted(o["id"] for o in outcomes if o["required"])
    reach = {o["id"]: o["reached_at"] for o in outcomes if o["required"]}
    roots_t = sorted(((at, r["logical_id"]) for r, at in terminal), key=lambda x: (x[0], x[1]))
    return {"roots": roots_t, "required": required, "reach": reach}, [], evidence


# ---------------------------------------------------------------- 点と判定（r1_rule の関数を使う）
def reached_by(reach: dict, t: int) -> list[int]:
    return sorted(v for v in reach.values() if v is not None and v <= t)


def points(tl: dict) -> list[dict]:
    pts = []
    roots = tl["roots"]
    need = len(tl["required"])
    for i, (t, lid) in enumerate(roots, start=1):
        done_ts = [rt for rt, _ in roots[:i]]
        reach_ts = reached_by(tl["reach"], t)
        m = {
            "order": i,
            "at": t,
            "root": lid,
            "outcomes": need,
            "outcomes_reached": len(reach_ts),
            "outcome_reached": need >= 1 and len(reach_ts) == need,
            "terminal_children": i,
            "no_progress_streak": r1.streak_after_reach(done_ts, reach_ts),
        }
        m["escalate"] = {str(k): r1.escalate(m, k) for k in KS}
        pts.append(m)
    return pts


def reach_summary(tl: dict) -> dict:
    need = len(tl["required"])
    times = sorted(v for v in tl["reach"].values() if v is not None)
    full = max(times) if len(times) == need and need >= 1 else None
    midway = []
    for r in sorted(set(times)):
        if len(reached_by(tl["reach"], r)) < need:
            done_ts = [rt for rt, _ in tl["roots"] if rt <= r]
            midway.append({"at": r, "streak_right_after": r1.streak_after_reach(done_ts, reached_by(tl["reach"], r))})
    return {"required": tl["required"], "reached_at": tl["reach"], "full_reach_at": full, "midway": midway}


def fired_before(pts: list[dict], full: int | None, k: int) -> list[int]:
    if full is None:
        return []
    return [p["order"] for p in pts if p["escalate"][str(k)] and p["at"] < full]


def judge_trial(facts: dict) -> dict:
    tl, reasons, evidence = extract(facts)
    out = {
        "included": tl is not None,
        "exclusion": [{"code": c, "reason": REASONS[c]} for c in reasons],
        "evidence": evidence,
        "points": [],
        "reach": None,
        "fired": {},
        "fired_before_full_reach": {},
        "f1_violations": [],
    }
    if tl is None:
        return out
    pts = points(tl)
    rs = reach_summary(tl)
    out["points"] = pts
    out["reach"] = rs
    out["fired"] = {str(k): [p["order"] for p in pts if p["escalate"][str(k)]] for k in KS}
    out["fired_before_full_reach"] = {str(k): fired_before(pts, rs["full_reach_at"], k) for k in KS}
    out["f1_violations"] = [m for m in rs["midway"] if m["streak_right_after"] != 0]
    return out


def judge(trials: list[dict]) -> dict:
    inc = [t for t in trials if t["included"]]
    midway = [t for t in inc if t["reach"]["midway"]]
    reached = [t for t in inc if t["reach"]["full_reach_at"] is not None]
    f1v = [{"set": t["set"], "trial": t["trial"], "violations": t["f1_violations"]} for t in midway if t["f1_violations"]]
    f2_by_k = {}
    for k in KS:
        hit = [{"set": t["set"], "trial": t["trial"], "points": t["fired_before_full_reach"][str(k)]} for t in reached if t["fired_before_full_reach"][str(k)]]
        f2_by_k[str(k)] = {"trials_fired_before_full_reach": hit, "count": len(hit), "hit": len(hit) >= F2_MIN_TRIALS}
    inconclusive = len(midway) < MIN_MIDWAY_TRIALS
    f1_hit = bool(f1v)
    f2_hit = f2_by_k[str(K)]["hit"]
    by_reason: dict[str, int] = {}
    for t in trials:
        for e in t["exclusion"]:
            by_reason[e["code"]] = by_reason.get(e["code"], 0) + 1
    return {
        "counts": {
            "trials": len(trials),
            "included": len(inc),
            "excluded": len(trials) - len(inc),
            "excluded_by_reason": dict(sorted(by_reason.items())),
            "midway_trials": len(midway),
            "full_reach_trials": len(reached),
        },
        "f1": {"midway_trials": [f"{t['set']}/{t['trial']}" for t in midway], "violations": f1v, "hit": f1_hit},
        "f2": {"k": K, "min_trials": F2_MIN_TRIALS, "by_k": f2_by_k, "hit": f2_hit},
        "inconclusive": inconclusive,
        "inconclusive_reason": (
            f"途中到達のある trial が {len(midway)} 件で {MIN_MIDWAY_TRIALS} 件未満" if inconclusive else None
        ),
        "falsified": (not inconclusive) and (f1_hit or f2_hit),
    }


def arms(trials: list[dict]) -> dict:
    out: dict[str, dict] = {}
    for t in trials:
        a = out.setdefault(t["arm"], {"trials": 0, "included": 0, "excluded": 0, "full_reach_trials": 0, "by_k": {}})
        a["trials"] += 1
        a["included" if t["included"] else "excluded"] += 1
        if t["included"] and t["reach"]["full_reach_at"] is not None:
            a["full_reach_trials"] += 1
        for k in KS:
            bk = a["by_k"].setdefault(str(k), {"false_positive_trials": 0, "false_positive_points": 0})
            fp = t["fired_before_full_reach"].get(str(k), [])
            bk["false_positive_trials"] += 1 if fp else 0
            bk["false_positive_points"] += len(fp)
    return dict(sorted(out.items()))


# ---------------------------------------------------------------- 入力と実行
def read_trials() -> list[dict]:
    rows = []
    for s in SETS:
        for f in sorted((LH / s / "facts").glob("*.json")):
            data = f.read_bytes()
            facts = json.loads(data)
            if facts.get("schema") != FACTS_SCHEMA:
                raise SystemExit(f"r1-bench: {f.relative_to(REPO)} の schema が {FACTS_SCHEMA} でない")
            am = json.loads((LH / s / "arm-map" / f.name).read_text(encoding="utf-8"))
            # a9 の arm-map は trial の欄を持たない（file 名が trial）。持つときだけ照合する。
            if am.get("trial", f.stem) != f.stem or am.get("case") != facts.get("case") or am.get("seed") != facts.get("seed"):
                raise SystemExit(f"r1-bench: {s}/arm-map/{f.name} の trial・case・seed が facts と違う")
            row = {
                "set": s,
                "trial": f.stem,
                "case": facts["case"],
                "arm": am["arm"],
                "seed": facts["seed"],
                "dataset": facts.get("dataset"),
                "facts_sha256": hashlib.sha256(data).hexdigest(),
            }
            row.update(judge_trial(facts))
            rows.append(row)
    return rows


def compute() -> dict:
    trials = read_trials()
    if len(trials) != EXPECTED_TRIALS:
        raise SystemExit(f"r1-bench: facts が {len(trials)} 件（{EXPECTED_TRIALS} 件のはず）")
    digest = hashlib.sha256("".join(f"{t['set']}/{t['trial']} {t['facts_sha256']}\n" for t in trials).encode()).hexdigest()
    j = judge(trials)
    return {
        "schema": SCHEMA,
        "rule": {
            "k": K,
            "sensitivity": list(SENSITIVITY),
            "escalate": "r1_rule.escalate: not outcome_reached and terminal_children >= k and no_progress_streak >= k",
            "streak": "r1_rule.streak_after_reach（到達した必要な Outcome の数が最後に増えた時刻より後に終端になった root の数）",
            "r1_rule_sha256": hashlib.sha256(R1_RULE.read_bytes()).hexdigest(),
            "mapping": {
                "project": "親",
                "root": "子",
                "point": "root が終端 status（" + "・".join(TERMINAL) + "）になった時点（時刻の順、同じ時刻は logical_id の順）",
                "terminal_time": "statuses の最初の終端の要素（{status, at} の object）の at",
                "reach_time": "outcomes の {id, required, reached_at}",
                "no_inference": "時刻が facts に無い trial は invocations・created_at・achieved_at から補わず除外する",
            },
            "judgement": {
                "f1": "途中到達のある trial で、到達の直後の no_progress_streak が 0 にならない到達がある",
                "f2": f"必要な Outcome に全部到達した trial のうち、到達より前に K = {K} で発火した trial が {F2_MIN_TRIALS} 件以上",
                "inconclusive": f"途中到達のある trial が {MIN_MIDWAY_TRIALS} 件未満なら inconclusive、falsified にしない",
            },
        },
        "llm_calls": 0,
        "inputs": {
            "sets": list(SETS),
            "facts_schema": FACTS_SCHEMA,
            "trials": len(trials),
            "facts_digest_sha256": digest,
            "r1_result_sha256": hashlib.sha256(R1_RESULT.read_bytes()).hexdigest(),
        },
        "trials": trials,
        "arms": arms(trials),
        **j,
    }


def render(result: dict) -> bytes:
    return (json.dumps(result, ensure_ascii=False, sort_keys=True, indent=1) + "\n").encode("utf-8")


def problems(doc: dict) -> list[str]:
    bad = [f"top.{f}" for f in REQUIRED["top"] if f not in doc]
    rule = doc.get("rule", {})
    bad += [f"rule.{f}" for f in REQUIRED["rule"] if f not in rule]
    if rule.get("k") != K or rule.get("sensitivity") != list(SENSITIVITY):
        bad.append(f"rule の k={rule.get('k')} sensitivity={rule.get('sensitivity')}（k=3・[2, 4] のはず）")
    if doc.get("llm_calls") != 0:
        bad.append("llm_calls が 0 でない")
    for f in ("falsified", "inconclusive"):
        if not isinstance(doc.get(f), bool):
            bad.append(f"{f}: bool でない")
    trials = doc.get("trials", [])
    if len(trials) != EXPECTED_TRIALS:
        bad.append(f"trials: {len(trials)} 件（{EXPECTED_TRIALS} 件のはず）")
    for t in trials:
        name = f"{t.get('set')}/{t.get('trial')}"
        bad += [f"{name}.{f}" for f in REQUIRED["trial"] if f not in t]
        if t.get("included") is False and not all(e.get("code") and e.get("reason") for e in t.get("exclusion") or [{}]):
            bad.append(f"{name}: 除外したのに理由が無い")
        if t.get("included") is True:
            for p in t.get("points", []):
                if str(K) not in p.get("escalate", {}):
                    bad.append(f"{name}.{p.get('order')}.escalate.{K}")
    return bad


def cmd_run() -> int:
    RESULT.write_bytes(render(compute()))
    print(f"r1-bench: wrote {RESULT.relative_to(REPO)}")
    return 0


def cmd_verify() -> int:
    if not RESULT.is_file():
        print(f"FAIL {RESULT.relative_to(REPO)} が無い")
        return 1
    want = RESULT.read_bytes()
    doc = json.loads(want)
    bad = problems(doc)
    for b in bad:
        print(f"FAIL {b}")
    ok = not bad
    got = render(compute())
    if got != want:
        print("FAIL 再計算が r1-bench.json と byte で一致しない")
        ok = False
    else:
        print(f"ok   再計算が r1-bench.json と byte で一致（{len(got)} byte）")
    c = doc.get("counts", {})
    print(
        f"r1-bench verify: {'ok' if ok else 'FAILED'}（trials={c.get('trials')} included={c.get('included')} "
        f"midway={c.get('midway_trials')} inconclusive={doc.get('inconclusive')} falsified={doc.get('falsified')}）"
    )
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
