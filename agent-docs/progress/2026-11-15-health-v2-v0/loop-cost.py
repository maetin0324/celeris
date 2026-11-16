#!/usr/bin/env python3
"""健全性v2 V0 loop-cost: 検査の追加費用・遅延・誤検出と開発 loop の実績を本番 DB から読み取りで数える。

DB は sqlite3 の URI（`file:...?mode=ro`）でだけ開く。`mode=ro` の無い URI・素の path は拒む（書き込みなし）。

  集計: python3 loop-cost.py --db 'file:/local/celeris/data/db/celeris.sqlite3?mode=ro' --out loop-cost.json
  検査: python3 loop-cost.py --db 'file:/local/celeris/data/db/celeris.sqlite3?mode=ro' --verify loop-cost.json

`--verify` は JSON の窓（B0 の窓と、JSON に固定した後の窓の終端）で数え直し、`measures` が JSON と
一致し、(a)〜(d) の欄が揃っていれば exit 0。違えば差を stderr に出して exit 1。
判定の規則は同じ dir の loop-cost.md にある。git の差分（指摘の path が変わったか）は `--repo` の
object を読む（既定はこの script の repo）。
"""
import argparse
import datetime
import json
import os
import re
import sqlite3
import statistics
import subprocess
import sys

SCHEMA = "health-v2-v0/loop-cost/1"
PARENT = "01M6TYV9DTZESAJJMXRQVGZ56V"
B0_START = "2026-10-24"  # B0 の task-history-collect.py と同じ文字列比較
B0_END = "2026-11-07T12:00"
B0_MEASURED_AT = "2026-11-07T12:20:58Z"  # B0 の task-history.json の measured_at
B0_PUBLISHED = {"claude": {"runs": 517, "with_usage": 351, "cost_usd_n": 326, "cost_usd_sum": 165.73,
                           "cost_usd_median": 0.4521, "cost_usd_p90": 0.9155,
                           "tokens_median": 494692, "tokens_p90": 1347122},
                "gpt": {"runs": 34, "with_usage": 31, "tokens_median": 633586, "tokens_p90": 1728388}}
HEALTH_CMD = "celeris:repo-health/1"
TOKEN_KEYS = ("input_tokens", "output_tokens", "cache_read_tokens", "cache_creation_tokens")
HUMAN_EVENT_TYPES = {"decision_answered", "answered", "approval_decided", "plan_gate_decided",
                     "integration_answered"}
HUMAN_TRANSITION_REASONS = {"answer", "comment", "approve", "accept"}
DELIVERY_TYPES = {"integration_detected", "integration_merged", "integration_discarded", "integration_held",
                  "integration_reopened", "integration_not_applicable"}
KEEP_TYPES = {"transitioned", "worker_finished", "review_verdict", "required_checks_evaluated", "reviewer_passed",
              "work_unit_check_finished", "integration_check_finished", "design_review_requested",
              "design_reviewed", "health_review_input_built", "health_review_input_skipped",
              "health_findings_repeated", "repair_scheduled", "auto_replan_requested",
              "execution_planned"} | HUMAN_EVENT_TYPES | DELIVERY_TYPES
TYPE_RE = re.compile(r'"type":"([a-z_]+)"')
FP_PATHS_RE = re.compile(r"health:([a-z0-9._-]+:[0-9a-f]{16})（evidence: ([^）]*)）")
FAIL_LINE_RE = re.compile(r"FAIL (\S+) ([^\s:]+):")
PATH_RE = re.compile(r"(?<![\w/.-])((?:crates|scripts|web|docs|agent-docs|tests|config|xtask|tools|\.celeris)"
                     r"/[\w./@+-]*\w)")
# policy・例外の file（health の profile・検査器の設定、例外・allowlist、baseline の json/toml）。agent-docs/ は計測記録なので除く。
EXCEPTION_PATH_RE = re.compile(r"^(?!agent-docs/)(?:(?:scripts/dev/repo-health/profiles/|\.celeris/)|"
                               r"(?:.*/)?[^/]*(?:exception|allowlist)[^/]*$|(?:.*/)?[^/]*baseline[^/]*\.(?:json|toml)$)",
                               re.I)
EXAMPLES = 5


def fam(model):
    if model and model.startswith("claude"):
        return "claude"
    if model and model.startswith("gpt"):
        return "gpt"
    return "other"


def q(xs, p):
    """B0 と同じ分位（sorted(xs)[int(p*(n-1))]）。"""
    return sorted(xs)[int(p * (len(xs) - 1))] if xs else None


def med(xs):
    return statistics.median(xs) if xs else None


def r4(x):
    return round(x, 4) if isinstance(x, float) else x


def open_ro(uri):
    if not uri.startswith("file:") or "mode=ro" not in uri.split("?", 1)[-1]:
        sys.exit("loop-cost: --db は mode=ro の sqlite URI（file:...?mode=ro）だけを受け付ける")
    return sqlite3.connect(uri, uri=True)


def in_win(ts, win):
    return win[0] <= ts < win[1]


class Git:
    def __init__(self, repo):
        self.repo = repo
        self.cache = {}

    def changed(self, a, b, paths):
        """a..b で paths のどれかが変わったか。object が無ければ None。"""
        key = (a, b, tuple(paths))
        if key not in self.cache:
            shas = a.split("...") if b is None else [a, b]
            ok = all(subprocess.run(["git", "-C", self.repo, "cat-file", "-e", f"{s}^{{commit}}"],
                                    capture_output=True).returncode == 0 for s in shas)
            if not ok:
                self.cache[key] = None
            else:
                revs = [a] if b is None else [a, b]
                out = subprocess.run(["git", "-C", self.repo, "diff", "--name-only", *revs, "--", *paths],
                                     capture_output=True, text=True)
                self.cache[key] = None if out.returncode != 0 else sorted(x for x in out.stdout.split("\n") if x)
        return self.cache[key]

    def existing(self, shas, paths):
        """shas のどれかの tree にある path だけを返す（本文から拾った語の誤りを除く）。"""
        keep = []
        for p in paths:
            if any(subprocess.run(["git", "-C", self.repo, "cat-file", "-e", f"{s}:{p}"],
                                  capture_output=True).returncode == 0 for s in shas if s):
                keep.append(p)
        return keep

    def touched_exception(self, a, b):
        out = self.changed(f"{a}...{b}", None, [])
        if out is None:
            return None
        return sorted(p for p in out if EXCEPTION_PATH_RE.search(p))


def load(c, end):
    runs = {}
    for rid, tid, wu, role, status, model, usage, metrics, st, fin in c.execute(
            "select run_id,task_id,work_unit_id,role,status,model,usage_json,metrics_json,started_at,finished_at "
            "from runs where started_at>=?", (B0_START,)):
        runs[rid] = dict(task=tid, wu=wu, role=role, status=status, family=fam(model),
                         usage=json.loads(usage) if usage else None,
                         metrics=json.loads(metrics) if metrics else None, started=st, finished=fin)
    ev = []
    fp_paths = {}
    for tid, seq, ts, j in c.execute("select task_id,seq,ts,json from events where ts>=? and ts<? order by id",
                                     (B0_START, end)):
        m = TYPE_RE.search(j[:80])
        t = m.group(1) if m else None
        if t == "worker_progress":
            continue
        if "（evidence: " in j:
            for fp, paths in FP_PATHS_RE.findall(j):
                fp_paths.setdefault(fp, sorted({p.strip() for p in paths.split(",") if p.strip()}))
        if t not in KEEP_TYPES:
            continue
        d = json.loads(j)
        if t == "design_review_requested":
            d.pop("plan", None)
        ev.append((tid, seq, ts, t, d))
    return runs, ev, fp_paths


def reviewer_stats(rs):
    out = {}
    for r in rs:
        a = out.setdefault(r["family"], dict(runs=0, with_usage=0, costs=[], tokens=[]))
        a["runs"] += 1
        u = r["usage"]
        if u:
            a["with_usage"] += 1
            a["tokens"].append(sum((u.get(k) or 0) for k in TOKEN_KEYS))
            if u.get("cost_usd"):
                a["costs"].append(u["cost_usd"])
    for a in out.values():
        cs, ts = a.pop("costs"), a.pop("tokens")
        a.update(cost_usd_sum=round(sum(cs), 2), cost_usd_n=len(cs), cost_usd_median=r4(med(cs)),
                 cost_usd_p90=r4(q(cs, .9)), tokens_median=med(ts), tokens_p90=q(ts, .9))
    return dict(sorted(out.items()))


def measure_a(runs, wins, design_runs):
    res = {}
    for name, win in wins.items():
        rs = [r for r in runs.values() if r["role"] == "reviewer" and in_win(r["started"], win)]
        if name == "after":  # 後の窓は終端までに終わった run だけ（終わった後に usage が書かれる）
            rs = [r for r in rs if r["finished"] and r["finished"] < win[1]]
        drs = [r for rid, r in runs.items() if rid in design_runs and r in rs]
        res[name] = dict(window=f"{win[0]}..{win[1]}", all_reviewer=reviewer_stats(rs),
                         design_review_only=reviewer_stats(drs),
                         final_review_only=reviewer_stats([r for r in rs if r not in drs]))
    b0 = [r for r in runs.values() if r["role"] == "reviewer" and in_win(r["started"], wins["b0"])]
    late = [r for r in b0 if r["family"] == "claude" and (not r["finished"] or r["finished"] > B0_MEASURED_AT)]
    res["b0_recount_vs_published"] = dict(
        published=B0_PUBLISHED,
        claude_runs_unfinished_at_b0_measure=len(late),
        claude_runs_unfinished_at_b0_measure_now_with_cost=sum(
            1 for r in late if r["usage"] and r["usage"].get("cost_usd")),
        claude_runs_still_unfinished=sum(1 for r in late if not r["finished"]),
    )
    return res


def health_ev(t, d):
    """経路ごとの health の証跡の正本（events.rs の PhaseCheckResult の doc）を (site_kind, key, evidence) で返す。"""
    if t in ("work_unit_check_finished", "integration_check_finished") and d.get("health"):
        yield ("wu" if t.startswith("work_unit") else "integration", d["key"], d["health"], d)
    elif t == "required_checks_evaluated":
        for r in d["results"]:
            if r.get("health"):
                yield ("review", d["repo_id"], r["health"], r)


def measure_b(ev, runs, wins):
    res = {}
    for name, win in wins.items():
        sites = {"wu": [], "integration": [], "review": []}
        all_check_ms = {"wu": 0, "integration": 0}
        built, skipped, repeated = [], [], 0
        dreq, drev = {}, []
        for tid, seq, ts, t, d in ev:
            if not in_win(ts, win):
                continue
            if t in ("work_unit_check_finished", "integration_check_finished"):
                all_check_ms["wu" if t.startswith("work_unit") else "integration"] += d.get("duration_ms") or 0
            for kind, _key, h, raw in health_ev(t, d):
                sites[kind].append((h["verdict"], raw.get("duration_ms"), raw.get("timed_out", False)))
            if t == "health_review_input_built":
                built.append((d["input"].get("findings") or {}).get("verdict"))
            elif t == "health_review_input_skipped":
                skipped.append(d["reason"].split("（")[0][:60])
            elif t == "health_findings_repeated":
                repeated += 1
            elif t == "design_review_requested":
                dreq.setdefault((tid, d["plan_sha256"]), ts)
            elif t == "design_reviewed":
                drev.append((tid, ts, d))
        out = {}
        for kind, xs in sites.items():
            verdicts = {}
            for v, _, _ in xs:
                verdicts[v] = verdicts.get(v, 0) + 1
            secs = [ms / 1000 for _, ms, _ in xs if ms is not None]
            out[kind] = dict(runs=len(xs), verdicts=dict(sorted(verdicts.items())),
                             failed=sum(1 for v, _, _ in xs if v != "pass"),
                             timed_out=sum(1 for _, _, to in xs if to),
                             seconds_sum=round(sum(secs), 1) if kind != "review" else None,
                             seconds_median=r4(med(secs)) if kind != "review" else None,
                             seconds_p90=r4(q(secs, .9)) if kind != "review" else None)
            if kind in all_check_ms:
                out[kind]["all_checks_seconds_sum"] = round(all_check_ms[kind] / 1000, 1)
        bv = {}
        for v in built:
            bv[str(v)] = bv.get(str(v), 0) + 1
        sk = {}
        for s in skipped:
            sk[s] = sk.get(s, 0) + 1
        out["review_input"] = dict(built=len(built), built_verdicts=dict(sorted(bv.items())), skipped=len(skipped),
                                   skipped_reasons=dict(sorted(sk.items())), seconds=None)
        out["findings_repeated_events"] = repeated
        costs, tokens, walls, lat = [], [], [], []
        for tid, ts, d in drev:
            r = runs.get(d["review_run"])
            if r and r["usage"]:
                if r["usage"].get("cost_usd"):
                    costs.append(r["usage"]["cost_usd"])
                tokens.append(sum((r["usage"].get(k) or 0) for k in TOKEN_KEYS))
            if r and r["metrics"] and r["metrics"].get("wall_ms") is not None:
                walls.append(r["metrics"]["wall_ms"] / 1000)
            t0 = dreq.get((tid, d["plan_sha256"]))
            if t0:
                lat.append((parse_ts(ts) - parse_ts(t0)).total_seconds())
        out["design_review"] = dict(
            requested=len(dreq), reviewed=len(drev), passed=sum(1 for *_, d in drev if d["pass"]),
            failed=sum(1 for *_, d in drev if not d["pass"]),
            runs_with_cost=len(costs), cost_usd_sum=round(sum(costs), 2), cost_usd_median=r4(med(costs)),
            cost_usd_p90=r4(q(costs, .9)), tokens_median=med(tokens), tokens_p90=q(tokens, .9),
            wall_seconds_sum=round(sum(walls), 1), wall_seconds_median=r4(med(walls)),
            request_to_verdict_seconds_median=r4(med(lat)), request_to_verdict_seconds_p90=r4(q(lat, .9)))
        res[name] = out
    return res


def parse_ts(ts):
    s = ts.rstrip("Z")
    if "." in s:
        head, frac = s.split(".", 1)
        s = head + "." + frac[:6]
    return datetime.datetime.fromisoformat(s)


def human_between(ev_by_task, tasks, t0, t1):
    for tid in tasks:
        for _seq, ts, t, d in ev_by_task.get(tid, []):
            if t0 < ts <= t1 and (t in HUMAN_EVENT_TYPES or
                                  (t == "transitioned" and d.get("reason") in HUMAN_TRANSITION_REASONS)):
                return f"{tid} {ts[:19]} {t}{(':' + d['reason']) if t == 'transitioned' else ''}"
    return None


def classify(git, ev_by_task, tid, t0, t1, sha0, sha1, paths, policy=False):
    """loop-cost.md の「誤検出の判定規則」の 1〜6 を上から順に当てる。"""
    if t1 is None:
        return "not_passed", None
    if policy:
        return "policy_changed", None
    who = human_between(ev_by_task, [tid], t0, t1)
    if who:
        return "human_decision", who
    if sha0 and sha1:
        exc = git.touched_exception(sha0, sha1)
        if exc:
            return "exception", "exception files: " + ", ".join(exc[:3])
    if paths and sha0 and sha1:
        paths = git.existing([sha0, sha1], paths)
    if not paths:
        return "no_paths", None
    if not (sha0 and sha1):
        return "git_unavailable", None
    ch = git.changed(sha0, sha1, paths)
    if ch is None:
        return "git_unavailable", None
    return ("passed_without_path_change", None) if not ch else ("changed_then_passed", ", ".join(ch[:3]))


def measure_c(ev, runs, fp_paths, wins, git):
    ev_by_task = {}
    for tid, seq, ts, t, d in ev:
        ev_by_task.setdefault(tid, []).append((seq, ts, t, d))
    # health: site ごとの証跡の列
    site_seq = {}
    for tid, seq, ts, t, d in ev:
        for kind, key, h, raw in health_ev(t, d):
            paths_review = sorted({p for _r, p in FAIL_LINE_RE.findall(raw.get("summary", ""))}) if kind == "review" \
                else []
            site_seq.setdefault((tid, kind, key), []).append((ts, h, paths_review))
    # reviewer: run → reviewed_sha、拒否の理由
    reviewed_sha, fail_reasons, outcome_text = {}, {}, {}
    for tid, seq, ts, t, d in ev:
        if t in ("required_checks_evaluated", "reviewer_passed", "health_review_input_built"):
            reviewed_sha.setdefault(d["review_run"], d.get("reviewed_sha"))
        elif t == "review_verdict" and not d.get("pass"):
            m = re.match(r"reviewer\(([0-9A-Z]{26})\)", d.get("reason", ""))
            if m:
                fail_reasons.setdefault(m.group(1), []).append(d["reason"])
        elif t == "worker_finished" and runs.get(d.get("run_id"), {}).get("role") == "reviewer":
            outcome_text[d["run_id"]] = d.get("outcome") or ""
    design_runs = {d["review_run"] for _, _, _, t, d in ev if t == "design_reviewed"}
    rev_runs = {}
    for rid, r in runs.items():
        if r["role"] == "reviewer" and rid not in design_runs:
            rev_runs.setdefault(r["task"], []).append((r["started"], rid))
    for v in rev_runs.values():
        v.sort()

    def review_run_at(tid, ts):
        xs = [rid for st, rid in rev_runs.get(tid, []) if st <= ts]
        return xs[-1] if xs else None

    res = {}
    for name, win in wins.items():
        cats, examples = {}, {}

        def add(group, cat, ex):
            g = cats.setdefault(group, {})
            g[cat] = g.get(cat, 0) + 1
            e = examples.setdefault(group, {}).setdefault(cat, [])
            if len(e) < EXAMPLES:
                e.append(ex)
        policy_stopped = 0
        for (tid, kind, key), xs in sorted(site_seq.items()):
            for i, (ts, h, paths_review) in enumerate(xs):
                if h["verdict"] == "pass" or not in_win(ts, win):
                    continue
                if h["verdict"] == "error":
                    add("health", "error_not_finding", dict(task=tid, site=f"{kind}:{key}", ts=ts[:19],
                                                            reason=(h.get("reason") or "")[:120]))
                    continue
                fps = h.get("blocking") or []
                policy = any(f.startswith("policy.changed:") for f in fps) or h["verdict"] == "policy_only"
                policy_stopped += policy
                nxt = next(((t2, h2) for t2, h2, _ in xs[i + 1:] if h2["verdict"] == "pass"), (None, None))
                paths = sorted({p for f in fps for p in fp_paths.get(f, [])} | set(paths_review))
                cat, why = classify(git, ev_by_task, tid, ts, nxt[0], h.get("head_sha"),
                                    (nxt[1] or {}).get("head_sha"), paths, policy)
                add("health", cat, dict(task=tid, site=f"{kind}:{key}", ts=ts[:19], blocking=fps[:3],
                                        paths=paths[:3], passed_at=(nxt[0] or "")[:19] or None, why=why))
        for tid, seq, ts, t, d in ev:
            if t == "transitioned" and d.get("reason") == "review_fail" and in_win(ts, win):
                rid = review_run_at(tid, ts)
                later = [(seq2, ts2) for seq2, ts2, t2, d2 in ev_by_task.get(tid, [])
                         if seq2 > seq and t2 == "transitioned" and d2.get("reason") == "review_pass"]
                ts_pass = later[0][1] if later else None
                rid_pass = review_run_at(tid, ts_pass) if ts_pass else None
                text = " ".join(fail_reasons.get(rid, [])) + " " + outcome_text.get(rid, "")
                paths = sorted({p.rstrip(".") for p in PATH_RE.findall(text)})
                cat, why = classify(git, ev_by_task, tid, ts, ts_pass,
                                    reviewed_sha.get(rid), reviewed_sha.get(rid_pass), paths)
                add("review", cat, dict(task=tid, ts=ts[:19], review_run=rid, paths=paths[:3],
                                        criteria_failed=len(fail_reasons.get(rid, [])),
                                        passed_at=(ts_pass or "")[:19] or None, why=why))
            if t == "design_reviewed" and not d["pass"] and in_win(ts, win):
                later = [ts2 for seq2, ts2, t2, d2 in ev_by_task.get(tid, [])
                         if seq2 > seq and t2 == "design_reviewed" and d2["pass"]]
                who = human_between(ev_by_task, [tid], ts, later[0]) if later else None
                cat = "not_passed" if not later else ("human_decision" if who else "passed_after_replan")
                add("design_review", cat, dict(task=tid, ts=ts[:19], findings=[f.get("kind") for f in d["findings"]][:3],
                                               paths=sorted({f.get("path") for f in d["findings"] if f.get("path")})[:3],
                                               passed_at=(later[0][:19] if later else None), why=who))
        res[name] = dict(counts={k: dict(sorted(v.items())) for k, v in sorted(cats.items())},
                         policy_changed_stops=policy_stopped,
                         examples={k: dict(sorted(v.items())) for k, v in sorted(examples.items())})
    return res


def measure_d(c, ev, runs, end):
    children = [dict(id=i, title=t) for i, t in c.execute(
        "select id,title from tasks where parent_id=? and created_at<? order by created_at", (PARENT, end))]
    kids = {}
    for i, p in c.execute("select id,parent_id from tasks where parent_id is not null"):
        kids.setdefault(p, []).append(i)
    ev_by_task = {}
    for tid, seq, ts, t, d in ev:
        ev_by_task.setdefault(tid, []).append((seq, ts, t, d))
    out = []
    for ch in children:
        tree, stack = [], [ch["id"]]
        while stack:
            x = stack.pop()
            tree.append(x)
            stack.extend(kids.get(x, []))
        mark = ",".join("?" * len(tree))
        plans = c.execute(f"select count(*) from execution_plans where task_id in ({mark}) and created_at<?",
                          (*tree, end)).fetchone()[0]
        wus = c.execute(f"select count(*), sum(kind like '%repair%') from work_units where task_id in ({mark}) "
                        "and created_at<?", (*tree, end)).fetchone()
        own = ev_by_task.get(ch["id"], [])
        tr = [(ts, d) for _s, ts, t, d in own if t == "transitioned"]
        sub = [x for tid in tree for x in ev_by_task.get(tid, [])]
        status = tr[-1][1]["to"] if tr else None
        started = next((ts for ts, d in tr if d.get("to") == "running"), None)
        done = tr[-1][0] if status == "done" else None
        delivery = [(ts, t, d) for _s, ts, t, d in own if t in DELIVERY_TYPES]
        rr = [r for r in runs.values() if r["task"] in tree and r["started"] < end]
        usd = sum((r["usage"] or {}).get("cost_usd") or 0 for r in rr if r["family"] == "claude"
                  and r["finished"] and r["finished"] < end)
        out.append(dict(
            id=ch["id"], title=ch["title"], subtree_tasks=len(tree), status_at_end=status,
            plan_versions=plans, work_units=wus[0], repair_work_units=wus[1] or 0,
            repair_scheduled=sum(1 for *_, t, d in sub if t == "repair_scheduled"),
            replan_transitions=sum(1 for *_, t, d in sub if t == "transitioned"
                                   and d.get("reason") in ("replan", "plan_replan")),
            auto_replan_requested=sum(1 for *_, t, d in sub if t == "auto_replan_requested"),
            review_fail=sum(1 for *_, t, d in sub if t == "transitioned" and d.get("reason") == "review_fail"),
            design_review_fail=sum(1 for *_, t, d in sub if t == "design_reviewed" and not d["pass"]),
            runs={role: sum(1 for r in rr if r["role"] == role) for role in ("planner", "worker", "reviewer")},
            claude_usd=round(usd, 2),
            gpt_runs=sum(1 for r in rr if r["family"] == "gpt"),
            started_at=started[:19] if started else None, done_at=done[:19] if done else None,
            hours_to_done=round((parse_ts(done) - parse_ts(started)).total_seconds() / 3600, 2)
            if started and done else None,
            delivery=(delivery[-1][1][len("integration_"):] if delivery else None),
        ))
    tot = {k: sum((x[k] or 0) for x in out) for k in ("plan_versions", "work_units", "repair_work_units",
                                                      "repair_scheduled", "replan_transitions",
                                                      "auto_replan_requested", "review_fail", "design_review_fail",
                                                      "claude_usd", "gpt_runs")}
    tot["claude_usd"] = round(tot["claude_usd"], 2)
    return dict(parent=PARENT, children=out, totals=tot)


def compute(c, end, repo):
    wins = {"b0": (B0_START, B0_END), "after": (B0_END, end)}
    runs, ev, fp_paths = load(c, end)
    design_runs = {d["review_run"] for _, _, _, t, d in ev if t == "design_reviewed"}
    return {
        "a_reviewer_runs": measure_a(runs, wins, design_runs),
        "b_health_cost": measure_b(ev, runs, wins),
        "c_false_positives": measure_c(ev, runs, fp_paths, wins, Git(repo)),
        "d_dev_loop": measure_d(c, ev, runs, end),
    }


UNMEASURED = [
    "GPT 系（codex）の reviewer run の USD: runs.usage_json の cost_usd が 0 で記録される。tokens だけ数えた",
    "final review の材料（HealthReviewInputBuilt）と RequiredChecksEvaluated の health の秒: event に所要時間の欄が無い",
    "usage_json の無い reviewer run の費用",
    "設計 review の拒否が『指摘の path を変えずに通った』か: 計画は commit でないので git で比べられない",
]


def diff(a, b, path=""):
    if isinstance(a, dict) and isinstance(b, dict):
        for k in sorted(set(a) | set(b)):
            yield from diff(a.get(k), b.get(k), f"{path}.{k}")
    elif a != b:
        yield f"{path}: json={json.dumps(a, ensure_ascii=False)[:200]} recount={json.dumps(b, ensure_ascii=False)[:200]}"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--db", required=True)
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--out")
    g.add_argument("--verify")
    ap.add_argument("--end", help="後の窓の終端（既定は今。UTC の YYYY-MM-DDTHH:MM:SSZ）")
    ap.add_argument("--repo", default=os.path.dirname(os.path.abspath(__file__)))
    args = ap.parse_args()
    repo = subprocess.run(["git", "-C", args.repo, "rev-parse", "--show-toplevel"], capture_output=True,
                          text=True).stdout.strip() or args.repo
    c = open_ro(args.db)
    if args.verify:
        with open(args.verify, encoding="utf-8") as f:
            stored = json.load(f)
        need = ["a_reviewer_runs", "b_health_cost", "c_false_positives", "d_dev_loop"]
        missing = [k for k in need if k not in stored.get("measures", {})]
        end = stored.get("windows", {}).get("after", {}).get("end")
        if missing or not end:
            print(f"loop-cost verify: 欄が無い: {missing or ['windows.after.end']}", file=sys.stderr)
            return 1
        got = compute(c, end, repo)
        diffs = list(diff(stored["measures"], got))
        if diffs:
            print("loop-cost verify: 数え直しが JSON と違う:\n  " + "\n  ".join(diffs[:40]), file=sys.stderr)
            return 1
        print(f"loop-cost verify: ok（b0 {B0_START}..{B0_END}、after {B0_END}..{end}、(a)〜(d) 一致）")
        return 0
    # 既定の終端は 60 秒前（書き込み中の transaction の event を窓に入れない）。
    end = args.end or (datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(seconds=60)).strftime(
        "%Y-%m-%dT%H:%M:%SZ")
    measures = compute(c, end, repo)
    doc = dict(schema=SCHEMA,
               measured_at=datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
               source="read-only sqlite URI (mode=ro): runs, events, tasks, execution_plans, work_units",
               windows=dict(b0=dict(start=B0_START, end=B0_END), after=dict(start=B0_END, end=end)),
               measures=measures, unmeasured=UNMEASURED)
    with open(args.out, "w", encoding="utf-8") as f:
        json.dump(doc, f, ensure_ascii=False, indent=1)
        f.write("\n")
    print(f"loop-cost: wrote {args.out}（after の終端 {end}）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
