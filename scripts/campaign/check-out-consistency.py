#!/usr/bin/env python3
"""runner の out dir 1 つについて、checker・export・human-op report の成功/失敗/欠測の件数と分母を照合する。

    python3 scripts/campaign/check-out-consistency.py --out <out dir> [--json-out <file>]

生記録（campaign.json・trials.jsonl・grades.jsonl）から成功契約（docs/ops/eval-raw-record-definitions.md §7）を
独立に再計算し、各集計の値と突き合わせる。一致で exit 0、不一致で 1、入力の誤りで 2。

- checker: `--check-json <file>`（`celeris-campaign check` の出力。`error: {...}` の形も可）。無ければ
  `--campaign-bin`（既定 $CELERIS_CAMPAIGN_BIN か PATH の celeris-campaign）で `check --manifest <out>/campaign.json
  --out <out> --measurement-id <保存 lock の id>` を実行し、`--check-arg` を足す（launcher と同じ資産の引数）。
- export: `--export-dir`（既定 <out>/export）の table.json か table.incomplete.json。無ければ「未生成」とし、
  `--export-log` があれば拒否の理由を写す（checker が違反を出すと export は表を書かない）。
- human-op report: `--human-ops-report`（既定 <out>/../human-ops-report、無ければ <out>/human-ops-report）の report.json。
- 欠測（missing）一覧と、未導出の wait 区間（wait_intervals）を report の試行行から出す。
- 共有作業（human-ops.jsonl の scope=shared）は report の shared_overhead に 1 度だけ載り、arm の試行に入らないこと。

checker の値は「checker が拒否していない記録」での再計算。採点の結合（hash・lock）は checker の判断を使う:
checker が lock 不信用で grades を外したときは、全 grade を未結合として再計算する。
"""
import argparse
import json
import os
import shutil
import subprocess
import sys
from collections import Counter, defaultdict

SIDE_SCHEMA = "celeris.campaign-manifest-side-b1c1/1"
NOT_RAN_REASONS = {"budget_stop", "dependency_blocked"}
CLASS_TO_CHECKER = {
    "validated_success": "final_passed",
    "model_failure": "grader_failed",
    "grader_env_failure": "grader_env_failure",
    "grader_undetermined": "grader_undetermined",
    "control_only": "grader_control_only",
    "observation_failed": "grader_observation_failed",
    "ungraded": "ungraded",
    "executor_failed": "failed",
    "executor_error": "error",
    "executor_timeout": "timeout",
    "executor_cancelled": "cancelled",
    "executor_blocked": "blocked",
    "missing": "missing",
}
WAIT_LANES = ["human_wait", "celeris_wait", "internal_wait", "external_queue_wait", "external_run"]


class InputError(Exception):
    pass


def read_json(path):
    try:
        with open(path, encoding="utf-8") as f:
            return json.load(f)
    except (OSError, ValueError) as e:
        raise InputError(f"{path}: {e}") from e


def read_jsonl(path):
    if not os.path.isfile(path):
        return []
    rows = []
    with open(path, encoding="utf-8") as f:
        for i, line in enumerate(f, 1):
            if line.strip():
                try:
                    rows.append(json.loads(line))
                except ValueError as e:
                    raise InputError(f"{path}:{i}: {e}") from e
    return rows


def json_from_text(text):
    """`celeris-campaign check` の stdout か `error: {...}`（stderr）から JSON を取り出す。"""
    i = text.find("{")
    if i < 0:
        raise InputError("checker output has no JSON object")
    try:
        return json.JSONDecoder().raw_decode(text[i:])[0]
    except ValueError as e:
        raise InputError(f"checker output: {e}") from e


# ---- manifest の計画 cell（crates/campaign/src/manifest.rs の active_arms / planned_configs と同じ規則）
def active_arms(m):
    sets = m.get("experiment_sets") or {}
    if sets:
        names = m.get("selected_sets") or list(sets)
        arms = set()
        for n in names:
            if n not in sets:
                raise InputError(f"selected_sets: unknown set {n}")
            arms.update(sets[n].get("arms", []))
        return sorted(arms)
    if m.get("schema") == SIDE_SCHEMA:
        return ["B", "C"]
    return ["A", "B", "C"]


def planned_cells(m):
    arms = active_arms(m)
    subset = set((m.get("secondary") or {}).get("subset") or [])
    side = m.get("schema") == SIDE_SCHEMA
    b_all = "A" in arms or (side and not m.get("experiment_sets"))
    repeats = int(m.get("repeats", 1) or 1)
    cells = []
    for p in m.get("problems", []):
        for a in arms:
            if a == "A" or (a == "B" and (b_all or p in subset)) or (a == "C" and p in subset):
                for rep in range(1, repeats + 1):
                    cells.append((a, p, rep))
    return arms, sorted(cells)


def ran(r):
    """record.rs の TrialRecord::ran。"""
    reason = r.get("reason")
    if reason in NOT_RAN_REASONS:
        return False
    if reason == "arm_budget":
        return bool(r.get("workspace"))
    return True


def grade_class(g):
    """grades.rs の bind（結合後の GradeOutcome）と success.rs の classify の executor passed 側。"""
    gr = g.get("grade")
    if not g.get("graded") or not isinstance(gr, dict):
        return None
    fp = gr.get("final_pass")
    if fp is True:
        c = "validated_success"
    elif gr.get("verdict") == "control_only":
        c = "control_only"
    else:
        c = {"model_failure": "model_failure", "env_failure": "grader_env_failure",
             "undetermined": "grader_undetermined"}.get(gr.get("failure_class"))
        if c is None:
            return None
    obs = g.get("observation") if isinstance(g.get("observation"), dict) else {}
    if obs.get("status") == "collect_failed" and fp is not True:
        classes = {i.get("class") for i in obs.get("issues") or []}
        c = "observation_failed" if classes - {"not_submitted"} else "model_failure"
    return c


def recompute(m, trials, grades, grades_bound, rejected_tags):
    arms, cells = planned_cells(m)
    recs = {}
    for r in trials:
        k = (r.get("config"), r.get("problem_id"), r.get("rep", 1), r.get("trial_no"))
        recs.setdefault(k, r)  # 再起動で重なった同じ試行は 1 件
    by_cell = defaultdict(list)
    for (a, p, rep, n), r in recs.items():
        by_cell[(a, p, rep)].append((n, r))
    gmap = {}
    for g in grades:
        t = g.get("trial") or {}
        k = (t.get("config"), t.get("problem_id"), t.get("rep", 1), t.get("trial_no"))
        tag = f"{k[0]}/{k[1]}/rep{k[2]}#{k[3]}"
        if grades_bound and tag not in rejected_tags:
            c = grade_class(g)
            if c is not None:
                gmap[k] = c
    per_arm = {a: Counter() for a in arms}
    per_cell = {}
    for a, p, rep in cells:
        rs = sorted(by_cell.get((a, p, rep), []), key=lambda x: x[0])
        t = per_arm[a]
        t["n"] += 1
        if not rs:
            cls, cell_ran, first = "missing", False, None
        else:
            n, first = rs[0]
            outcome = first.get("outcome")
            if outcome == "passed":
                cls = gmap.get((a, p, rep, n), "ungraded")
                t["passed"] += 1
            else:
                cls = f"executor_{outcome}"
            cell_ran = ran(first)
            reason = first.get("reason")
            if reason == "executor_error" or (not cell_ran and reason != "dependency_blocked"):
                t["unexecuted"] += 1
        t[CLASS_TO_CHECKER.get(cls, cls)] += 1
        t["class:" + cls] += 1
        # report の消費の欠測（cell 単位）: 未実行の cell は not_run、記録なし・ran 試行のどれかが値なしなら欠測。
        for key, field in (("tokens", "tokens"), ("cost", "cost_micro_usd")):
            if first is not None and not cell_ran:
                t[f"report_{key}_not_run"] += 1
            elif first is None or any(ran(r) and r.get(field) is None for _, r in rs) or not any(
                    ran(r) for _, r in rs):
                t[f"report_{key}_missing"] += 1
        per_cell[f"{a}/{p}/rep{rep}"] = {"class": cls, "ran": cell_ran, "recorded_trials": len(rs)}
    # checker の消費: 実行した全記録（再試行を含む）。
    for (a, _p, _rep, _n), r in recs.items():
        if a in per_arm and ran(r):
            t = per_arm[a]
            t["ran_trials"] += 1
            t["tokens_missing_trials"] += r.get("tokens") is None
            t["cost_missing_trials"] += r.get("cost_micro_usd") is None
    return arms, per_arm, per_cell


CHECKER_KEYS = ["n", "passed", "final_passed", "grader_failed", "grader_env_failure", "grader_undetermined",
                "grader_control_only", "grader_observation_failed", "ungraded", "failed", "error", "timeout",
                "cancelled", "blocked", "missing", "unexecuted", "ran_trials", "tokens_missing_trials",
                "cost_missing_trials"]
EXPORT_KEYS = ["n", "passed", "final_passed", "grader_failed", "grader_env_failure", "grader_undetermined",
               "grader_control_only", "ungraded", "failed", "error", "timeout", "cancelled", "blocked", "missing",
               "ran_trials", "tokens_missing_trials", "cost_missing_trials"]


def run_checker(args, out, lock):
    binp = args.campaign_bin or os.environ.get("CELERIS_CAMPAIGN_BIN") or shutil.which("celeris-campaign")
    if not binp:
        raise InputError("no checker output: pass --check-json or --campaign-bin")
    cmd = [binp, "check", "--manifest", args.manifest or os.path.join(out, "campaign.json"), "--out", out]
    mid = (lock or {}).get("measurement_id")
    if mid and "--measurement-id" not in args.check_arg:
        cmd += ["--measurement-id", mid]
    cmd += args.check_arg
    p = subprocess.run(cmd, capture_output=True, text=True, check=False)
    return json_from_text(p.stdout + "\n" + p.stderr), cmd


def load_export(d, log):
    for stem in ("table", "table.incomplete"):
        p = os.path.join(d, stem + ".json")
        if os.path.isfile(p):
            return {"status": "produced", "path": p, "table": read_json(p)}
    st = {"status": "not_produced", "path": d}
    if log and os.path.isfile(log):
        with open(log, encoding="utf-8") as f:
            st["reason"] = f.read().strip()[:2000]
    return st


def compare(arms, raw, checker, export, report):
    table, mismatches = {}, []
    ctot = (checker.get("recomputed") or {}).get("totals") or {}
    rows = {}
    if export.get("table"):
        rows = {r.get("config"): r for r in export["table"].get("configs", [])}
    rarms = (report or {}).get("arms") or {}
    for a in arms:
        rw = raw[a]
        entry = {}

        def chk(key, src, got, want):
            ok = got == want
            entry.setdefault(key, {"raw": want})[src] = got
            if not ok:
                mismatches.append(f"{a}.{key}: {src}={got} raw={want}")

        for k in CHECKER_KEYS:
            if a in ctot:
                chk(k, "checker", ctot[a].get(k, 0), rw.get(k, 0))
        if a not in ctot:
            mismatches.append(f"{a}: checker has no totals")
        if a in rows:
            for k in EXPORT_KEYS:
                chk(k, "export", rows[a].get(k, 0), rw.get(k, 0))
            chk("failures", "export", rows[a].get("failures"), rw["n"] - rw["final_passed"])
        elif export.get("table"):
            mismatches.append(f"{a}: export table has no row")
        if report is not None:
            ra = rarms.get(a)
            if ra is None:
                mismatches.append(f"{a}: human-op report has no arm")
            else:
                chk("n", "report.trials", ra.get("trials", 0), rw["n"])
                chk("final_passed", "report.validated_success", ra.get("validated_success", 0), rw["final_passed"])
                chk("passed", "report.executor_passed", ra.get("executor_passed", 0), rw["passed"])
                classes = {k[6:]: v for k, v in rw.items() if k.startswith("class:") and v}
                chk("success_classes", "report", ra.get("success_classes", {}), classes)
                for key, field in (("tokens", "tokens"), ("cost", "cost_micro_usd")):
                    chk(f"report_{key}_missing", "report", ra.get(f"{field}_missing_trials", 0),
                        rw.get(f"report_{key}_missing", 0))
                    chk(f"report_{key}_not_run", "report", ra.get(f"{field}_not_run_trials", 0),
                        rw.get(f"report_{key}_not_run", 0))
                chk("not_run", "report", ra.get("not_run_trials", 0),
                    rw.get("report_tokens_not_run", 0))
        table[a] = entry
    return table, mismatches


def missing_and_waits(report, per_cell):
    missing, waits = [], []
    for c, v in per_cell.items():
        if v["class"] in ("missing", "ungraded", "grader_env_failure", "grader_undetermined", "observation_failed"):
            missing.append({"cell": c, "item": "success_class", "value": v["class"],
                            "note": "未確定（成功にも失敗にも数えない理由がある。分母には残る）"})
    for t in (report or {}).get("trials", []):
        cell = f"{t.get('arm')}/{t.get('case_id')}/rep{t.get('rep')}"
        for k, v in sorted(t.items()):
            if isinstance(v, dict) and v.get("source") == "missing" and k not in WAIT_LANES:
                missing.append({"cell": cell, "item": k, "reason": v.get("reason")})
        for lane in WAIT_LANES:
            v = t.get(lane)
            if isinstance(v, dict):
                derived = v.get("source") != "missing"
                waits.append({"cell": cell, "lane": lane, "derived": derived, "source": v.get("source"),
                              "reason": None if derived else v.get("reason")})
    return missing, waits


def shared_check(out, report):
    recs = read_jsonl(os.path.join(out, "human-ops.jsonl"))
    shared = [r for r in recs if r.get("scope") == "shared"]
    corrections = [r for r in recs if r.get("record_type") == "correction" or r.get("corrects")]
    over = (report or {}).get("shared_overhead") or []
    in_report = sum(int(s.get("records") or 0) for s in over)
    campaigns = [s.get("campaign_id") for s in over]
    res = {"shared_records": len(shared), "shared_overhead_records": in_report,
           "shared_overhead_campaigns": campaigns, "corrections": len(corrections)}
    problems = []
    if len(campaigns) != len(set(campaigns)):
        problems.append("shared_overhead lists a campaign more than once")
    if report is not None and not corrections and in_report != len(shared):
        problems.append(f"shared_overhead has {in_report} records, human-ops.jsonl has {len(shared)} shared")
    res["counted_once"] = not problems
    res["note"] = ("共有作業の記録なし（欠測。0 ではない）" if not shared else
                   "shared scope の記録は arm へ配分せず shared_overhead に 1 度だけ載る")
    return res, problems


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--out", required=True)
    ap.add_argument("--manifest")
    ap.add_argument("--check-json")
    ap.add_argument("--campaign-bin")
    ap.add_argument("--check-arg", action="append", default=[])
    ap.add_argument("--export-dir")
    ap.add_argument("--export-log")
    ap.add_argument("--human-ops-report")
    ap.add_argument("--json-out")
    args = ap.parse_args(argv)
    out = os.path.abspath(args.out)
    try:
        if not os.path.isfile(os.path.join(out, "campaign.json")):
            raise InputError(f"{out}: campaign.json is missing (not a runner out dir)")
        m = read_json(args.manifest or os.path.join(out, "campaign.json"))
        lock = read_json(os.path.join(out, "lock.json")) if os.path.isfile(os.path.join(out, "lock.json")) else None
        cmd = None
        if args.check_json:
            with open(args.check_json, encoding="utf-8") as f:
                checker = json_from_text(f.read())
        else:
            checker, cmd = run_checker(args, out, lock)
        if not checker.get("recomputed"):
            raise InputError(f"checker gave no recomputed totals: {checker.get('violations')}")
        violations = checker.get("violations") or []
        grades_bound = not any("grades.jsonl is not bound" in v for v in violations)
        rejected = {v.split(":", 1)[0] for v in violations if "grade" in v and "#" in v.split(":", 1)[0]}
        trials = read_jsonl(os.path.join(out, "trials.jsonl"))
        grades = read_jsonl(os.path.join(out, "grades.jsonl"))
        arms, raw, per_cell = recompute(m, trials, grades, grades_bound, rejected)
        export = load_export(args.export_dir or os.path.join(out, "export"), args.export_log)
        rdir = args.human_ops_report
        if not rdir:
            for c in (os.path.join(os.path.dirname(out), "human-ops-report"), os.path.join(out, "human-ops-report")):
                if os.path.isfile(os.path.join(c, "report.json")):
                    rdir = c
                    break
        report = read_json(os.path.join(rdir, "report.json")) if rdir else None
    except InputError as e:
        print(f"check-out-consistency: {e}", file=sys.stderr)
        return 2
    table, mismatches = compare(arms, raw, checker, export, report)
    if report is None:
        mismatches.append("human-op report.json not found (pass --human-ops-report)")
    missing, waits = missing_and_waits(report, per_cell)
    shared, shared_problems = shared_check(out, report)
    mismatches += shared_problems
    result = {
        "schema": "celeris.out-consistency/1",
        "out": out,
        "consistent": not mismatches,
        "mismatches": mismatches,
        "denominator": "planned cells of the manifest (active arm problems x repeats); consumption missing counts "
                       "use executed trials (checker: every ran record; report: per cell, not-run cells separate)",
        "sources": {
            "checker": {"cmd": cmd, "check_json": args.check_json, "ok": checker.get("ok"),
                        "complete": checker.get("complete"), "violations": violations,
                        "incomplete_reasons": checker.get("incomplete_reasons"), "grades_bound": grades_bound},
            "export": {k: v for k, v in export.items() if k != "table"},
            "human_op_report": rdir,
        },
        "counts": table,
        "cells": per_cell,
        "missing": missing,
        "wait_intervals": waits,
        "wait_intervals_underived": sorted({w["lane"] for w in waits if not w["derived"]}),
        "shared": shared,
    }
    text = json.dumps(result, ensure_ascii=False, indent=2, sort_keys=True)
    if args.json_out:
        with open(args.json_out, "w", encoding="utf-8") as f:
            f.write(text + "\n")
    print(text)
    return 0 if result["consistent"] else 1


if __name__ == "__main__":
    sys.exit(main())
