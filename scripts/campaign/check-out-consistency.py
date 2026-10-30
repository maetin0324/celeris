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
- 共有作業（human-ops.jsonl の case/arm/rep の identity が無い記録。human_ops_report.rs と同じ規則）は report の
  shared_overhead に 1 度だけ載り、arm の試行に入らないこと。
- final-accounting: `--final-accounting <file>`（`celeris-campaign final-accounting` の出力）があれば、arm ごとの
  cell 数・ran・not_run・tokens 欠測、cell ごとの ran と trial_tokens、final_cutoff（観測 token の和・欠測試行数）、
  manifest_hash、total の null（欠測を 0 で埋めない）、共有行（shared_unattributable）の数を生記録と照合する。
  共有行の生記録は decision-log export の records.jsonl（`--records`、既定 <out>/../dl/records.jsonl）の
  shared_overhead=true の usage 行を dedup_key で 1 度だけ数えたもの。共有行は total へ足さない（added_to_total=false）。

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
    # report の arm 表は human-op 記録の arm_label（A0・ops-r1-A など）で引かれる。試行行の arm（config）から label を引く。
    labels = {}
    for t in (report or {}).get("trials", []):
        lab = (t.get("arm_labels") or [None])[0]
        if t.get("arm") and lab:
            labels.setdefault(t["arm"], lab)
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
            ra = rarms.get(a, rarms.get(labels.get(a)))
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


def is_shared_op(r):
    """human_ops_report.rs: case_id・arm・repetition のどれかが無い記録は shared（arm へ配分しない）。"""
    return r.get("scope") == "shared" or any(r.get(k) is None for k in ("case_id", "arm", "repetition"))


def shared_check(out, report):
    recs = read_jsonl(os.path.join(out, "human-ops.jsonl"))
    shared = [r for r in recs if is_shared_op(r)]
    corrections = [r for r in recs if r.get("record_type") == "correction" or r.get("corrects")]
    over = (report or {}).get("shared_overhead") or []
    in_report = sum(int(s.get("records") or 0) for s in over)
    campaigns = [s.get("campaign_id") for s in over]
    by_campaign = Counter(r.get("campaign_id") for r in shared)
    res = {"shared_records": len(shared), "shared_overhead_records": in_report,
           "shared_ops": len({(r.get("campaign_id"), r.get("logical_op_id")) for r in shared}),
           "shared_overhead_campaigns": campaigns, "corrections": len(corrections)}
    problems = []
    if len(campaigns) != len(set(campaigns)):
        problems.append("shared_overhead lists a campaign more than once")
    if report is not None and not corrections and in_report != len(shared):
        problems.append(f"shared_overhead has {in_report} records, human-ops.jsonl has {len(shared)} shared")
    if report is not None and not corrections:
        for s in over:
            c, n = s.get("campaign_id"), int(s.get("records") or 0)
            if n != by_campaign.get(c, 0):
                problems.append(f"shared_overhead[{c}] has {n} records, human-ops.jsonl has {by_campaign.get(c, 0)}")
    res["counted_once"] = not problems
    res["note"] = ("共有作業の記録なし（欠測。0 ではない）" if not shared else
                   "shared scope の記録は arm へ配分せず shared_overhead に 1 度だけ載る")
    return res, problems


def shared_usage_rows(records_path, campaign_id, manifest_hash):
    """final_accounting.rs と同じ絞り込み（schema・campaign_id・manifest_hash）で、共有の usage 行を dedup_key で数える。"""
    if not records_path or not os.path.isfile(records_path):
        return None
    lines, keys, cos = 0, set(), set()
    for r in read_jsonl(records_path):
        if r.get("record_type") != "usage" or r.get("schema") != "celeris.decision-log/1":
            continue
        if r.get("campaign_id") != campaign_id or (r.get("manifest_hash") not in (None, manifest_hash)):
            continue
        if r.get("shared_overhead") is True:
            lines += 1
            keys.add(r.get("dedup_key") or "")
            if r.get("role") == "cos":
                cos.add(r.get("dedup_key") or "")
    return {"lines": lines, "rows": len(keys), "cos_rows": len(cos)}


def final_accounting_check(fa, arms, trials, lock, records_path):
    """final-accounting を生記録（trials.jsonl・records.jsonl）と照合する。(照合表, mismatches) を返す。"""
    mism, table = [], {}
    recs = {}
    for r in trials:
        recs.setdefault((r.get("config"), r.get("problem_id"), r.get("rep", 1), r.get("trial_no")), r)
    raw_cells = {}
    observed, missing = 0, 0
    for (a, p, rep, _n), r in sorted(recs.items(), key=lambda x: (str(x[0][0]), str(x[0][1]), x[0][2],
                                                                    x[0][3] or 0)):
        c = raw_cells.setdefault(f"{a}/{p}/rep{rep}", {"arm": a, "ran": False, "trial_tokens": None, "known": False})
        if not ran(r):
            continue
        t = r.get("tokens")
        c["trial_tokens"] = t if not c["known"] else (c["trial_tokens"] + t if None not in (
            c["trial_tokens"], t) else None)
        c["ran"] = c["known"] = True
        if t is None:
            missing += 1
        else:
            observed += t
    fcells = {}
    for c in (fa.get("trials") or {}).get("cells") or []:
        k = f"{c.get('arm')}/{c.get('case_id')}/rep{c.get('repetition')}"
        if k in fcells:
            mism.append(f"final_accounting lists cell {k} more than once")
        fcells[k] = c
    if set(fcells) != set(raw_cells):
        mism.append(f"final_accounting cells {sorted(fcells)} != recorded cells {sorted(raw_cells)}")
    for k in sorted(set(fcells) & set(raw_cells)):
        for f in ("ran", "trial_tokens"):
            if fcells[k].get(f) != raw_cells[k][f]:
                mism.append(f"{k}.{f}: final_accounting={fcells[k].get(f)} raw={raw_cells[k][f]}")
    for a in arms:
        mine = [c for c in fcells.values() if c.get("arm") == a]
        raw = [c for c in raw_cells.values() if c["arm"] == a]
        ran_trials = [r for (x, *_), r in recs.items() if x == a and ran(r)]
        entry = {}
        for key, got, want in (
                ("n", len(mine), len(raw)),
                ("ran", sum(bool(c.get("ran")) for c in mine), sum(c["ran"] for c in raw)),
                ("not_run", sum(not c.get("ran") for c in mine), sum(not c["ran"] for c in raw)),
                ("tokens_missing_cells", sum(bool(c.get("ran")) and c.get("trial_tokens") is None for c in mine),
                 sum(c["ran"] and c["trial_tokens"] is None for c in raw)),
                ("tokens_missing_trials_raw", None, sum(r.get("tokens") is None for r in ran_trials))):
            entry[key] = {"raw": want}
            if got is not None:
                entry[key]["final_accounting"] = got
                if got != want:
                    mism.append(f"{a}.{key}: final_accounting={got} raw={want}")
        table[a] = entry
    budget = fa.get("budget") or {}
    fc = budget.get("final_cutoff") or {}
    total = fa.get("total") or {}
    totals = {"observed_trial_tokens": {"raw": observed, "final_accounting": fc.get("observed_trial_tokens")},
              "missing_trials": {"raw": missing, "final_accounting": fc.get("missing_trials")},
              "total_tokens_complete": {"raw": missing == 0,
                                        "final_accounting": (total.get("complete") or {}).get("tokens")}}
    if lock and lock.get("campaign_sha256"):
        totals["manifest_hash"] = {"raw": lock["campaign_sha256"], "final_accounting": fa.get("manifest_hash")}
    if lock and lock.get("measurement_id"):
        totals["campaign_id"] = {"raw": lock["measurement_id"], "final_accounting": fa.get("campaign_id")}
    for k, v in totals.items():
        if v["raw"] != v["final_accounting"]:
            mism.append(f"total.{k}: final_accounting={v['final_accounting']} raw={v['raw']}")
    if missing and total.get("tokens") is not None:
        mism.append(f"total.tokens: final_accounting={total.get('tokens')} although {missing} trial(s) are missing "
                    "(0-filled?)")
    # 共有行: total に足さず、records.jsonl の共有 usage 行を 1 度だけ数える。
    su = fa.get("shared_unattributable") or {}
    fa_rows = (su.get("rows") or {}).get("n_rows")
    fa_cos = ((su.get("cos") or {}).get("rows") or {}).get("n_rows")
    raw_sh = shared_usage_rows(records_path, fa.get("campaign_id"), fa.get("manifest_hash"))
    shared = {"added_to_total": su.get("added_to_total"), "rows": {"final_accounting": fa_rows},
              "cos_rows": {"final_accounting": fa_cos}, "records": records_path}
    if su.get("added_to_total") is not False:
        mism.append(f"shared_unattributable.added_to_total: final_accounting={su.get('added_to_total')} want false")
    if raw_sh is None:
        mism.append("final_accounting shared rows: records.jsonl not found (pass --records)")
    else:
        shared["rows"]["raw"], shared["cos_rows"]["raw"] = raw_sh["rows"], raw_sh["cos_rows"]
        shared["record_lines"] = raw_sh["lines"]
        if fa_rows != raw_sh["rows"]:
            mism.append(f"shared_unattributable.rows: final_accounting={fa_rows} raw={raw_sh['rows']} "
                        f"(unique dedup_key of {raw_sh['lines']} shared usage line(s))")
        if fa_cos != raw_sh["cos_rows"]:
            mism.append(f"shared_unattributable.cos.rows: final_accounting={fa_cos} raw={raw_sh['cos_rows']}")
    shared["counted_once"] = not any(m.startswith("shared_unattributable") for m in mism)
    return {"per_arm": table, "totals": totals, "shared": shared, "total": total}, mism


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
    ap.add_argument("--final-accounting")
    ap.add_argument("--records")
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
        fa = read_json(args.final_accounting) if args.final_accounting else None
        records = args.records
        if fa is not None and not records:
            for c in (os.path.join(os.path.dirname(out), "dl", "records.jsonl"), os.path.join(out, "dl", "records.jsonl")):
                if os.path.isfile(c):
                    records = c
                    break
    except InputError as e:
        print(f"check-out-consistency: {e}", file=sys.stderr)
        return 2
    table, mismatches = compare(arms, raw, checker, export, report)
    if report is None:
        mismatches.append("human-op report.json not found (pass --human-ops-report)")
    missing, waits = missing_and_waits(report, per_cell)
    shared, shared_problems = shared_check(out, report)
    mismatches += shared_problems
    fa_src = None
    if fa is not None:
        if fa.get("schema") != "celeris.campaign-final-accounting/1":
            mismatches.append(f"final_accounting schema {fa.get('schema')!r}")
        fa_cmp, fa_mism = final_accounting_check(fa, arms, trials, lock, records)
        mismatches += fa_mism
        fa_src = dict(fa_cmp, path=os.path.abspath(args.final_accounting), status="produced")
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
            "final_accounting": fa_src,
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
