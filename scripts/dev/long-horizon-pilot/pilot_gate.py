#!/usr/bin/env python3
"""縮小 pilot の測定の契約を判定する（ADR 2026-11-06-phase5-n1-pilot-measurement-contract）。

    python3 -I pilot_gate.py check-approval --approval <json> --cases C06,C08 --arms new [--max-wall-secs N]
    python3 -I pilot_gate.py judge --raw <pilot の out> [--approval <json>] --out <gate.json> [--pilot-facts <json>]

judge は raw（facts/・ops/・grade/grades.jsonl）を読み、欠測を成功にしない（fail closed）:
- verdict は pass・fail・unknown。測れていない項目が 1 つでもあれば unknown、測れて規則に反すれば fail。
- g7 は実 LLM（harness_label が config）のときだけ pass か fail。stub・config-stub は pending（cos_usage_null は書かない）。
- --pilot-facts は pc_rollout.py --pilot-facts に渡す形（g7・cos_runs・cos_usage_null・missing）を書く。
exit: 0 = pass、1 = fail、3 = unknown、2 = 使い方の誤り・承認の不備。
"""
import argparse
import datetime
import json
import pathlib
import re
import sys

SCHEMA = "celeris.long-horizon-pilot-gate/1"
CACHE_KEYS = ("cache_read_tokens", "cache_creation_tokens")
APPROVAL_LIMITS = {"max_usd", "max_wall_secs", "max_quota_pct"}
DECIDED = ("once", "standing", "denied")


def now_utc():
    return datetime.datetime.now(datetime.timezone.utc)


def parse_time(v):
    try:
        t = datetime.datetime.fromisoformat(str(v).replace("Z", "+00:00"))
    except ValueError:
        return None
    return t if t.tzinfo else None


def check_approval(ap, cases, arms, max_wall_secs=None, now=None):
    """人の承認の記録を確かめる。問題の一覧を返す（空なら有効）。"""
    now = now or now_utc()
    problems = []
    for key in ("approval_id", "approved_by", "approved_at", "expires_at", "account", "model", "harness", "scope",
                "limits"):
        if not ap.get(key):
            problems.append(f"approval: {key} is missing")
    if ap.get("approved_by") and not re.fullmatch(r"human:[^\s:]+", str(ap["approved_by"])):
        problems.append("approval must be by a human (approved_by human:<name>)")
    exp = parse_time(ap.get("expires_at")) if ap.get("expires_at") else None
    if ap.get("expires_at") and exp is None:
        problems.append("approval: expires_at is not an ISO 8601 time with a zone")
    elif exp is not None and exp <= now:
        problems.append(f"approval expired at {ap['expires_at']}")
    scope = ap.get("scope") or {}
    for name, want in (("cases", cases), ("arms", arms)):
        allowed = set(scope.get(name) or [])
        outside = [x for x in want if x not in allowed]
        if outside:
            problems.append(f"approval.scope.{name} does not cover {','.join(outside)}")
    if scope and scope.get("environment") != "non-production":
        problems.append("approval.scope.environment must be non-production")
    limits = ap.get("limits") or {}
    for key in sorted(set(limits) - APPROVAL_LIMITS):
        problems.append(f"approval.limits.{key} is not accepted (limits are max_usd, max_wall_secs, max_quota_pct)")
    for key in ("max_usd", "max_wall_secs"):
        v = limits.get(key)
        if isinstance(v, bool) or not isinstance(v, (int, float)) or v <= 0:
            problems.append(f"approval.limits.{key} must be a positive number")
    w = limits.get("max_wall_secs")
    if max_wall_secs is not None and isinstance(w, (int, float)) and max_wall_secs > w:
        problems.append(f"--max-wall-secs {max_wall_secs} exceeds approval.limits.max_wall_secs {w}")
    return problems


def load(path):
    return json.loads(pathlib.Path(path).read_text())


def num(v):
    return isinstance(v, (int, float)) and not isinstance(v, bool)


def invocation_gaps(facts):
    """invocations を数え直す（facts の usage.missing だけを信じない。修正前の driver は CoS の null を見落とした）。

    CoS の行は `launch`（launched・not_launched・unrecorded）で分ける（ADR 2026-11-06-cos-run-launch-accounting-and-drain D5）。
    欄の無い旧 raw は unrecorded（起動済み）として読むので、旧 raw の判定は変わらない。not_launched は課金対象の起動ではない
    ので roles["cos"]・欠測に数えず、roles["cos_not_launched"] に数える。drain が待ちきれなかった行（drain_timeout）の usage
    欠測は cos_drain_timeout に数える。"""
    gaps = {"run_usage_null": 0, "cos_usage_null": 0, "cos_drain_timeout": 0, "cache_missing": 0, "usd_missing": 0}
    roles = {}
    for i in facts.get("invocations") or []:
        role = i.get("role") or "?"
        if role == "cos" and i.get("launch") == "not_launched":
            roles["cos_not_launched"] = roles.get("cos_not_launched", 0) + 1
            continue
        roles[role] = roles.get(role, 0) + 1
        u = i.get("usage")
        if not u:
            if role != "cos":
                gaps["run_usage_null"] += 1
            else:
                gaps["cos_drain_timeout" if i.get("drain_timeout") else "cos_usage_null"] += 1
            continue
        if not all(isinstance(u.get(k), int) and not isinstance(u.get(k), bool) for k in CACHE_KEYS):
            gaps["cache_missing"] += 1
        if not num(u.get("cost_usd")) or u["cost_usd"] < 0:
            gaps["usd_missing"] += 1
    return gaps, roles


def judge_trial(facts, ops, grade):
    """1 試行の検査。checks は (name, state, detail) の list。state は pass・fail・unknown・info。"""
    checks = []

    def add(name, state, detail):
        checks.append({"name": name, "state": state, "detail": detail})

    label = (ops or {}).get("harness_label") or (ops or {}).get("harness")
    real = label == "config"
    add("real_llm", "pass" if real else "unknown", f"harness_label={label}")
    usage = facts.get("usage") or {}
    gaps, roles = invocation_gaps(facts)
    reasons = [f"{k}:{n}" for k, n in gaps.items() if n]
    if usage.get("missing"):
        reasons.append(f"facts.missing={usage['missing']}")
    for key in ("tokens", "cache_tokens", "usd"):
        if not num(usage.get(key)):
            reasons.append(f"usage.{key}=null")
    add("usage_complete", "unknown" if reasons else "pass", ",".join(reasons) or "usage・cache・USD が全起動にある")
    cos = roles.get("cos", 0)
    add("cos_runs", "pass" if cos >= 1 else "fail", f"cos_runs={cos}")
    # 起動前に終わった CoS run は費用不明ではない。ただし CoS が返事をしなかったので、G7 は pass にしない。
    not_launched = roles.get("cos_not_launched", 0)
    add("cos_not_launched", "pass" if not_launched == 0 else "fail", f"cos_not_launched={not_launched}")
    drain = facts.get("drain")
    if not isinstance(drain, dict):
        add("drain", "info", "not recorded (old raw)")
    else:
        late = drain.get("timed_out")
        if not num(late):
            add("drain", "unknown", f"drain.timed_out={late}")
        elif late > 0 or drain.get("wall_limit_hit"):
            why = f"timed_out={late}" if late else "wall limit reached before the controller stopped"
            add("drain", "unknown", f"{why} deadline_at={drain.get('deadline_at')} waited_secs={drain.get('waited_secs')}")
        else:
            add("drain", "pass", f"waited_secs={drain.get('waited_secs')} deadline_at={drain.get('deadline_at')}")
    for role in ("worker", "reviewer"):
        n = roles.get(role, 0)
        add(f"{role}_runs", "pass" if n >= 1 else "unknown", f"{role}_runs={n}")
    # 追記専用は driver が試行中に見た行を終わりに照らす（D1）。固定値だった旧い facts（events_checked なし）は不明。
    ev = facts.get("events_checked")
    if facts.get("events_append_only") is False:
        add("events_append_only", "fail", json.dumps(ev))
    elif facts.get("events_append_only") is True and isinstance(ev, dict) and ev.get("seen", 0) > 0:
        add("events_append_only", "pass", json.dumps(ev))
    else:
        add("events_append_only", "unknown", f"not measured (events_checked={ev})")
    for key in ("cos_launch_rejected", "cos_ops_rejected"):
        if not num(facts.get(key)):
            add(key, "unknown", "not measured")
        elif key == "cos_launch_rejected":
            add(key, "pass" if facts[key] == 0 else "fail", str(facts[key]))
        else:
            add(key, "info", str(facts[key]))
    leaks = facts.get("leak_hits")
    if not isinstance(leaks, list):
        add("leak_hits", "unknown", "not measured")
    else:
        add("leak_hits", "pass" if not leaks else "fail", str(len(leaks)))
    if ops is None:
        add("approvals_decided", "unknown", "ops record is missing")
    else:
        stale = [a.get("id") for a in ops.get("approvals") or [] if a.get("decision") not in DECIDED]
        add("approvals_decided", "fail" if stale else "pass", f"expired_or_undecided={len(stale)}")
    if grade is None:
        add("oracle", "unknown", "no grade row")
        add("double_effect", "unknown", "no grade row")
    else:
        ag = grade.get("agreement") or {}
        verdict = (grade.get("grade") or {}).get("verdict")
        if verdict not in ("pass", "fail") or not isinstance(ag.get("controller_achieved"), bool):
            add("oracle", "unknown", f"oracle={verdict} controller_achieved={ag.get('controller_achieved')}")
        else:
            add("oracle", "info", f"oracle={verdict} controller_achieved={ag['controller_achieved']} "
                                  f"mismatch={ag.get('mismatch')}")
        de = (grade.get("metrics") or {}).get("double_effect")
        add("double_effect", "unknown" if not num(de) else ("pass" if de == 0 else "fail"), str(de))
    return checks, gaps, roles, real


def fold(states):
    if "fail" in states:
        return "fail"
    if "unknown" in states:
        return "unknown"
    return "pass"


def judge(raw, approval=None, now=None):
    raw = pathlib.Path(raw)
    grades = {}
    gfile = raw / "grade" / "grades.jsonl"
    if gfile.is_file():
        for line in gfile.read_text().splitlines():
            if line.strip():
                g = json.loads(line)
                grades[g.get("trial")] = g
    trials = []
    tot = {"usd": 0.0, "wall_secs": 0, "cache_creation_tokens": 0, "cos_runs": 0, "cos_not_launched": 0}
    usd_known = wall_known = True
    gap_total = {}
    all_real = True
    files = sorted((raw / "facts").glob("*.json")) if (raw / "facts").is_dir() else []
    for f in files:
        facts = load(f)
        trial = f.stem
        opsf = raw / "ops" / f"{trial}.json"
        ops = load(opsf) if opsf.is_file() else None
        checks, gaps, roles, real = judge_trial(facts, ops, grades.get(trial))
        all_real = all_real and real
        for k, n in gaps.items():
            gap_total[k] = gap_total.get(k, 0) + n
        usage = facts.get("usage") or {}
        if num(usage.get("usd")) and not any(gaps.values()):
            tot["usd"] += usage["usd"]
        else:
            usd_known = False
        if num(usage.get("wall_secs")):
            tot["wall_secs"] += usage["wall_secs"]
        else:
            wall_known = False
        tot["cos_runs"] += roles.get("cos", 0)
        tot["cos_not_launched"] += roles.get("cos_not_launched", 0)
        tot["cache_creation_tokens"] += sum(int((i.get("usage") or {}).get("cache_creation_tokens") or 0)
                                            for i in facts.get("invocations") or [])
        trials.append({"trial": trial, "case": facts.get("case"), "arm": (ops or {}).get("arm"),
                       "verdict": fold([c["state"] for c in checks]), "checks": checks, "roles": roles})
    top = []

    def add(name, state, detail):
        top.append({"name": name, "state": state, "detail": detail})

    add("trials", "pass" if trials else "unknown", f"{len(trials)} trials")
    if approval is None:
        add("approval", "unknown", "承認待ち（承認の記録が無い）")
        add("cost_cap", "unknown", "承認の上限が無い")
        add("wall_cap", "unknown", "承認の上限が無い")
    else:
        lim = approval.get("limits") or {}
        add("approval", "pass", f"{approval.get('approval_id')} by {approval.get('approved_by')} "
                                f"expires {approval.get('expires_at')}")
        if not usd_known:
            add("cost_cap", "unknown", "USD が出ない試行がある（0 として足さない）")
        else:
            add("cost_cap", "pass" if tot["usd"] <= lim.get("max_usd", 0) else "fail",
                f"usd={tot['usd']:.6f} max_usd={lim.get('max_usd')}")
        if not wall_known:
            add("wall_cap", "unknown", "wall_secs が無い試行がある")
        else:
            add("wall_cap", "pass" if tot["wall_secs"] <= lim.get("max_wall_secs", 0) else "fail",
                f"wall_secs={tot['wall_secs']} max_wall_secs={lim.get('max_wall_secs')}")
        if "max_quota_pct" in lim:
            add("quota_cap", "unknown", "driver は quota を測らない（上限があるときは判定できない）")
    add("quota", "info", "not_measured（0 と書かない）")
    verdict = fold([c["state"] for c in top] + [t["verdict"] for t in trials])
    if not trials or not all_real:
        g7 = "pending"
    else:
        g7_states = [c["state"] for t in trials for c in t["checks"]
                     if c["name"] in ("usage_complete", "cos_runs", "cos_not_launched", "drain")]
        g7 = "pass" if tot["cos_runs"] >= 1 and all(s in ("pass", "info") for s in g7_states) else "fail"
    return {
        "schema": SCHEMA, "verdict": verdict, "g7": g7,
        # cos_usage_null は実 LLM の facts があるときだけ数を書く（stub では null）。
        "cos_usage_null": gap_total.get("cos_usage_null", 0) if g7 != "pending" else None,
        "cos_not_launched": tot["cos_not_launched"] if g7 != "pending" else None,
        "cos_drain_timeout": gap_total.get("cos_drain_timeout", 0) if g7 != "pending" else None,
        "real_llm": bool(trials) and all_real,
        "approval": None if approval is None else {k: approval.get(k) for k in (
            "approval_id", "approved_by", "approved_at", "expires_at", "scope", "limits", "account", "model",
            "harness")},
        "approval_state": "承認待ち" if approval is None else "approved",
        "totals": {**tot, "usd": round(tot["usd"], 6) if usd_known else None,
                   "wall_secs": tot["wall_secs"] if wall_known else None, "gaps": gap_total},
        "checks": top, "trials": trials,
    }


def pilot_facts(gate):
    """pc_rollout.py --pilot-facts の入力。"""
    missing = [f"{k}:{n}" for k, n in gate["totals"]["gaps"].items() if n]
    if not gate["real_llm"]:
        missing.append("not_real_llm")
    return {"schema": SCHEMA, "g7": gate["g7"], "verdict": gate["verdict"], "cos_runs": gate["totals"]["cos_runs"],
            "cos_usage_null": gate["cos_usage_null"], "cos_not_launched": gate["cos_not_launched"],
            "cos_drain_timeout": gate["cos_drain_timeout"], "missing": missing,
            "approval_id": (gate["approval"] or {}).get("approval_id")}


def main(argv=None):
    p = argparse.ArgumentParser()
    sub = p.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("check-approval")
    c.add_argument("--approval", required=True)
    c.add_argument("--cases", required=True)
    c.add_argument("--arms", required=True)
    c.add_argument("--max-wall-secs", type=int)
    j = sub.add_parser("judge")
    j.add_argument("--raw", required=True)
    j.add_argument("--approval")
    j.add_argument("--out", required=True)
    j.add_argument("--pilot-facts")
    a = p.parse_args(argv)
    if a.cmd == "check-approval":
        try:
            ap = load(a.approval)
        except (OSError, ValueError) as e:
            print(json.dumps({"ok": False, "problems": [f"approval: {e}"]}))
            return 2
        problems = check_approval(ap, a.cases.split(","), a.arms.split(","), a.max_wall_secs)
        print(json.dumps({"ok": not problems, "problems": problems, "approval_id": ap.get("approval_id"),
                          "max_usd": (ap.get("limits") or {}).get("max_usd"), "expires_at": ap.get("expires_at"),
                          "max_wall_secs": (ap.get("limits") or {}).get("max_wall_secs")}, ensure_ascii=False))
        return 2 if problems else 0
    approval = load(a.approval) if a.approval else None
    gate = judge(a.raw, approval)
    pathlib.Path(a.out).write_text(json.dumps(gate, ensure_ascii=False, indent=1) + "\n")
    if a.pilot_facts:
        pathlib.Path(a.pilot_facts).write_text(json.dumps(pilot_facts(gate), ensure_ascii=False, indent=1) + "\n")
    print(f"pilot-gate: verdict={gate['verdict']} g7={gate['g7']} cos_usage_null={gate['cos_usage_null']} "
          f"approval={gate['approval_state']}")
    return {"pass": 0, "fail": 1}.get(gate["verdict"], 3)


if __name__ == "__main__":
    sys.exit(main())
