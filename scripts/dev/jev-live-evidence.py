#!/usr/bin/env python3
"""Jev 副比較（B1/C1）の実 `celerisctl campaign export` 出力を、独立読取元の原記録と照合して検証する。

信頼境界（本 script の契約）:
  - 信頼の根は運用者が入力とは独立に設定する CELERIS_JEV_SOURCE_DB（測定 daemon の SQLite）だけ。
    jev_source_reader が mode=ro + query_only・単一読取 transaction で開く。evidence dir の中の DB、
    CELERIS_DB / CELERIS_RUN_DB と同じ DB、未設定・到達不能は拒否（照合を省く switch は無い）。
  - evidence dir の records.jsonl / summary.json / provenance.json / routing_events.jsonl は「照合される側」。
    原記録から決定的に投影した export（jev_source_reconcile.build_export）・task 木・routing_decided と
    全件・全欄で照合し、重複（同一内容を含む）・欠落・余剰（孤立・別 campaign 混入）・改変・未知欄・順序違いを拒否。
  - provenance の files_sha256 / export_command / manifest_hash（CLI 引数 --manifest-hash 由来）は証明にしない。
    manifest hash は読取元の task_campaigns の値だけを使う。
  - 段階判定（jev_smoke::judge / jev_stages 相当）へは全件照合が通った後にだけ渡す。入力は原記録から実 ID で組む:
    routing_decided は event の run_id、Jev 消費は usage.run_id、応答版はその run の decision_log_runs の
    classifier_snapshot。task だけで結ぶ fallback・他 run の版や費用での補完はしない（run 未結合は未検証）。

入力 dir:
  records.jsonl / summary.json   `celerisctl campaign export` の出力そのもの
  provenance.json                {campaign_id, tasks{task_id:{parent}}, ...}（tasks は原記録と全件照合）
  routing_events.jsonl           1 行 {task_id, run_id, event}（原記録の routing_decided と全件照合）

終了コード: 0 合格 / 1 検証不合格 / 2 live 証拠なし（dir・必須 file なし）または信頼元なし・到達不能 /
3 読取元の schema 不明・必要欄なし・campaign 不在（live 不可）。
"""
import argparse
import importlib.util
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))


def _load(name, file):
    spec = importlib.util.spec_from_file_location(name, os.path.join(HERE, file))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


reader = _load("jev_source_reader", "jev_source_reader.py")
rec = _load("jev_source_reconcile", "jev_source_reconcile.py")

SCHEMA = "celeris.decision-log/1"
ROLES = ["cos", "planner", "worker", "reviewer", "jev"]
LANE_RANK = {"cheap": 0, "standard": 1, "frontier": 2}
ULID = re.compile(r"^[0-9A-HJKMNP-TV-Z]{26}$")
FILES = ["records.jsonl", "summary.json", "provenance.json", "routing_events.jsonl"]
PROVENANCE_KEYS = {"source", "campaign_id", "manifest_hash", "generated_at", "export_command", "files_sha256", "tasks"}
ARM_MODE = {"B": "fixed", "C": "decision_assisted"}
# 凍結 manifest（celeris.campaign-manifest-side-b1c1/1）の B1/C1 共通候補: Sonnet と Opus の 2 件（source/model/harness）。
# 読取元 DB は manifest の hash だけを持つので、本文の契約値は此処に固定し、全 B/C 試行をこれと照合する。
FROZEN_CANDIDATES = frozenset({
    ("anthropic", "claude-sonnet-5-5", "claude_code"),
    ("anthropic", "claude-opus-5-5", "claude_code"),
})
# 実効 pin のうち B/C で一致すべき欄（mode だけが違ってよい）と、固定値で要求する欄。
PIN_COMMON_KEYS = ("source", "lower_policy", "evidence_mode", "evidence_table_id", "online_update")


class NoEvidence(Exception):
    pass


def read_json(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def read_jsonl(path):
    out = []
    with open(path, encoding="utf-8") as f:
        for n, line in enumerate(f, 1):
            if not line.strip():
                raise ValueError(f"{os.path.basename(path)}:{n}: 空行（export は空行を出さない）")
            try:
                out.append(json.loads(line))
            except ValueError as e:
                raise ValueError(f"{os.path.basename(path)}:{n}: JSON として読めない: {e}")
    return out


def load(evidence_dir):
    if not os.path.isdir(evidence_dir) or not os.listdir(evidence_dir):
        raise NoEvidence(f"live 証拠なし: {evidence_dir} が無い、または空")
    missing = [f for f in FILES if not os.path.isfile(os.path.join(evidence_dir, f))]
    if missing:
        raise NoEvidence(f"live 証拠なし: {evidence_dir} に必須 file が無い: {', '.join(missing)}")
    p = lambda n: os.path.join(evidence_dir, n)
    return {
        "records": read_jsonl(p("records.jsonl")),
        "summary": read_json(p("summary.json")),
        "prov": read_json(p("provenance.json")),
        "events": read_jsonl(p("routing_events.jsonl")),
    }


def _decision_of(ev):
    cur = ev
    for k in ("record", "features", "decision"):
        cur = cur.get(k) if isinstance(cur, dict) else None
    return cur if isinstance(cur, dict) else None


def lane_assist(ev):
    """routing_decided の record.features.decision.lane_assist（RouteDecision の実の位置）。"""
    la = (_decision_of(ev) or {}).get("lane_assist")
    return la if isinstance(la, dict) else None


def resolved_of(ev):
    r = (_decision_of(ev) or {}).get("routing_resolved")
    return r if isinstance(r, dict) else None


def classifier(la):
    st = (la or {}).get("classifier_status")
    if not isinstance(st, dict):
        return ("missing", None)
    s = st.get("status")
    if s == "jev_ok":
        return ("jev_ok", None)
    if s == "fell_back_to_fixed":
        return ("fallback", st.get("reason") or "unknown")
    if s == "not_attempted":
        return ("not_attempted", None)
    return ("missing", None)


def attribution_wrong(la, kind, fixed_lane):
    """jev_smoke::attribution の Wrong 判定だけ（理由 str か None）。"""
    base, fin, basis = la.get("base_lane"), la.get("final_lane"), la.get("basis")
    if not (base and fin and basis):
        return "base_lane/final_lane/basis が欠けている"
    b, f = LANE_RANK.get(base), LANE_RANK.get(fin)
    if b is None or f is None:
        return f"未知の lane: {base} / {fin}"
    if basis == "classification_raise" and kind != "jev_ok":
        return "classification_raise だが Jev の分類は成功していない"
    if f < b:
        return "final_lane が base_lane より低い"
    if f == b:
        return "lane が動いていないのに classification_raise" if basis == "classification_raise" else None
    x = LANE_RANK.get(fixed_lane) if fixed_lane else None
    if basis == "classification_raise":
        return "fixed 規則だけで final_lane に届くのに classification_raise" if x is not None and x >= f else None
    if x is not None and x < f:
        return f"fixed 規則だけでは final_lane に届かないのに basis={basis}"
    return None


def judge(ev_line, usage_present, cost, version):
    """jev_smoke::judge 相当。live_proven は jev_ok・費用・版・許可内・帰属矛盾なし。"""
    ev = ev_line.get("event") or {}
    la = lane_assist(ev)
    kind, reason = classifier(la)
    viol = []
    in_limits = True
    wrong = None
    if la is not None:
        fin = la.get("final_lane") or ""
        allowed = ev_line.get("allowed") or []
        max_lane = ev_line.get("max_lane")
        permitted = (not allowed or fin in allowed) and (
            max_lane is None
            or (LANE_RANK.get(fin) is not None and LANE_RANK.get(max_lane) is not None
                and LANE_RANK[fin] <= LANE_RANK[max_lane])
        )
        in_limits = permitted and fin in LANE_RANK
        if not in_limits:
            viol.append(f"final_lane {fin} が許可範囲の外")
        wrong = attribution_wrong(la, kind, ev_line.get("fixed_lane"))
        if wrong:
            viol.append(f"basis の帰属: {wrong}")
    else:
        viol.append("lane_assist 欄が無い")
    version_ok = bool(version)
    cost_ok = usage_present and cost is not None
    if kind == "jev_ok":
        if not usage_present:
            viol.append("Jev の消費行が無い")
        elif cost is None:
            viol.append("Jev の費用が欠測")
        if not version_ok:
            viol.append("Jev の応答版（model snapshot）が欠測")
    proven = kind == "jev_ok" and cost_ok and version_ok and in_limits and wrong is None
    return {"kind": kind, "reason": reason, "live_proven": proven, "violations": viol,
            "cost_ok": cost_ok, "version_ok": version_ok}


def new_stages():
    return {
        "trials": 0, "classification_attempted": 0, "not_attempted": 0, "missing": 0,
        "jev_ok": 0, "jev_fallback": 0, "jev_calls": 0, "live_proven": 0,
        "jev_cost_missing": 0, "jev_cost_usd": 0.0, "version_missing": 0,
        "lane_proposal_changed": 0, "lane_proposal_absent": 0, "final_lane_changed": 0,
        "final_lane_counts": {}, "lane_out_of_limits": 0,
        "model_changed": 0, "model_unknown": 0, "lane_changed_and_model_changed": 0,
        "suppressed": {"explicit": 0, "ceiling": 0, "availability": 0, "classifier_fallback": 0,
                       "classifier_fallback_reasons": {}, "not_attempted": 0, "run_fallback": 0,
                       "budget": 0},
        "results": {}, "result_missing": 0, "retries": 0,
    }


def add_stage(s, t):
    """jev_stages::add 相当。t は 1 試行の結合結果。"""
    v, la = t["verdict"], t["la"]
    s["trials"] += 1
    s["jev_calls"] += t["jev_calls"]
    k = v["kind"]
    if k == "jev_ok":
        s["classification_attempted"] += 1
        s["jev_ok"] += 1
        if not v["cost_ok"]:
            s["jev_cost_missing"] += 1
        if not v["version_ok"]:
            s["version_missing"] += 1
    elif k == "fallback":
        s["classification_attempted"] += 1
        s["jev_fallback"] += 1
        s["suppressed"]["classifier_fallback"] += 1
        r = s["suppressed"]["classifier_fallback_reasons"]
        r[v["reason"]] = r.get(v["reason"], 0) + 1
    elif k == "not_attempted":
        s["not_attempted"] += 1
        s["suppressed"]["not_attempted"] += 1
    else:
        s["missing"] += 1
    if t["jev_cost"] is not None:
        s["jev_cost_usd"] += t["jev_cost"]
    if v["live_proven"]:
        s["live_proven"] += 1
    g = (la or {}).get
    if la is not None:
        if v["violations"] and any("許可範囲" in x for x in v["violations"]):
            s["lane_out_of_limits"] += 1
        p, b = g("proposed_lane"), g("base_lane")
        if p is None:
            s["lane_proposal_absent"] += 1
        elif p != b:
            s["lane_proposal_changed"] += 1
        if g("final_lane"):
            c = s["final_lane_counts"]
            c[g("final_lane")] = c.get(g("final_lane"), 0) + 1
    lane_changed = bool(la) and g("base_lane") and g("final_lane") and g("base_lane") != g("final_lane")
    if lane_changed:
        s["final_lane_changed"] += 1
    ob = g("overridden_by")
    if ob in ("explicit", "ceiling", "availability"):
        s["suppressed"][ob] += 1
    pm, em = t["proposed_model"], t["exec_model"]
    if pm and em:
        if pm != em:
            s["model_changed"] += 1
            if lane_changed:
                s["lane_changed_and_model_changed"] += 1
    else:
        s["model_unknown"] += 1
    if t["fallback_reason"]:
        s["suppressed"]["run_fallback"] += 1
    if t["budget_stopped"]:
        s["suppressed"]["budget"] += 1
    if t["result"]:
        s["results"][t["result"]] = s["results"].get(t["result"], 0) + 1
    else:
        s["result_missing"] += 1
    if t["is_retry"]:
        s["retries"] += 1


def models_of(trial):
    return {c.get("model_id") for c in (trial.get("proposed_candidates") or []) if c.get("model_id")}


def _numbered(lines):
    """routing_events の行に (task_id, run_id, event 全文, 出現回数) の鍵を付ける。同一行の重複も別鍵になる。"""
    seen, out = {}, []
    for r in lines:
        if not isinstance(r, dict) or not set(r) <= {"task_id", "run_id", "event"}:
            out.append({"_key": None})
            continue
        c = rec.canon([r.get("task_id"), r.get("run_id"), r.get("event")])
        seen[c] = seen.get(c, 0) + 1
        out.append({"_key": f"{c}#{seen[c]}"})
    return out


def reconcile(data, proj):
    """evidence 全体を原記録の投影と照合する。(code, msg) の列（空なら全件一致）。"""
    errs = list(proj["problems"])
    c = proj["campaign_id"]
    errs += rec.reconcile_lines("records.jsonl", proj["records"], data["records"], rec.record_key, campaign=c)
    errs += [(code if code != "altered" else "summary", m)
             for code, m in rec.reconcile_lines("summary.json", [proj["summary"]], [data["summary"]], lambda _: "summary")]
    prov = data["prov"] if isinstance(data["prov"], dict) else {}
    unknown = sorted(set(prov) - PROVENANCE_KEYS)
    if unknown:
        errs.append(("provenance", f"provenance.json に未知欄: {unknown}（fixture/synthetic 等の自己申告は受け付けない）"))
    if prov.get("campaign_id") != c:
        errs.append(("provenance", f"provenance.campaign_id {prov.get('campaign_id')!r} が読取元の campaign {c!r} と違う"))
    if "manifest_hash" in prov and prov["manifest_hash"] != proj["manifest_hash"]:
        errs.append(("provenance", "provenance.manifest_hash が読取元の task_campaigns.manifest_hash と違う"))
    want_tasks = [{"task_id": k, "parent": v["parent"]} for k, v in sorted(proj["tasks"].items())]
    got_tasks = prov.get("tasks")
    if not isinstance(got_tasks, dict):
        errs.append(("task-link", "provenance.tasks（task 木）が無い"))
    else:
        got = [dict({"task_id": k}, **v) if isinstance(v, dict) else {"task_id": k, "_": v} for k, v in sorted(got_tasks.items())]
        errs += [("task-link", m) for _, m in rec.reconcile_lines("provenance.tasks", want_tasks, got, lambda r: r.get("task_id"))]
    errs += [("routing-events", m) for _, m in rec.reconcile_lines(
        "routing_events.jsonl", _numbered(proj["routing_events"]), _numbered(data["events"]), lambda r: r.get("_key"))]
    return errs


def bind(src, proj):
    """原記録から run ごとの段階入力を実 ID で組む。(joined, errs, unverified, unbound)。"""
    errs, unverified = [], []
    run_map, usage_map = proj["run_map"], proj["usage_map"]
    tree = {t: v["parent"] for t, v in proj["tasks"].items()}
    runs_tbl = {}
    for r in src["runs"]:
        runs_tbl.setdefault(r["run_id"], []).append(r)
    ev_by_run, ev_used = {}, {}
    for e in src["routing_decided"]:
        rid = (e["json"] or {}).get("run_id")
        if not rid:
            errs.append(("event-link", f"routing_decided event id={e['id']}（task {e['task_id']}）に run_id が無い（run へ結べない）"))
            continue
        ev_by_run.setdefault(rid, []).append(e)
        if rid not in run_map:
            errs.append(("run-not-exported", f"原記録の run {rid}（task {e['task_id']}）に routing_decided があるが export の run に無い（欠落）"))
    unbound = [u for u in usage_map.values() if u["role"] == "jev" and not u["shared_overhead"] and u["run_id"] is None]
    for u in unbound:
        unverified.append(f"Jev 消費行 {u['dedup_key']} は run_id が無く task {u['task_id']} にしか結べない（task 単位の fallback はしない: どの run の費用・認証の証拠にもしない）")
    joined = []
    for rid in sorted(run_map):
        r = run_map[rid]
        p = r["p"]
        tid = p["task_id"]
        label = f"run {rid}（task {tid}）"
        rows = runs_tbl.get(rid, [])
        if len(rows) != 1 or rows[0]["task_id"] != tid:
            errs.append(("run-link", f"{label}: runs 表に同じ run_id・task_id の実行記録が 1 件ない（{[x['task_id'] for x in rows]}）"))
        if tid not in tree:
            errs.append(("task-link", f"{label}: task が campaign 木に無い"))
        evs = [e for e in ev_by_run.get(rid, [])]
        e = None
        if len(evs) != 1:
            errs.append(("no-event" if not evs else "duplicate-event",
                         f"{label}: run_id で結ばれた routing_decided が {len(evs)} 件（1 件ちょうどが要る。task 単位の fallback はしない）"))
        else:
            e = evs[0]
            if e["task_id"] != tid:
                errs.append(("event-link", f"{label}: routing_decided の task {e['task_id']} が run の task と違う"))
            ev_used.setdefault(e["id"], []).append(rid)
        parent = p["parent_run_id"]
        if parent is not None:
            pr = run_map.get(parent)
            if pr is None:
                errs.append(("retry", f"{label}: retry の親 run {parent} が campaign の run に無い"))
            else:
                if pr["p"]["task_id"] != tid:
                    errs.append(("retry", f"{label}: retry の親 run {parent} の task {pr['p']['task_id']} が違う"))
                a, b = p["attempt_no"], pr["p"]["attempt_no"]
                if a is None or b is None or a != b + 1:
                    errs.append(("retry", f"{label}: attempt_no {a} が親 {parent} の attempt_no {b} + 1 でない"))
        seen, cur = set(), rid
        while cur is not None and cur in run_map and cur not in seen:
            seen.add(cur)
            cur = run_map[cur]["p"]["parent_run_id"]
        if cur is not None and cur in seen:
            errs.append(("retry", f"{label}: retry の親の連鎖が循環"))
        jev_rows = [u for u in usage_map.values() if u["role"] == "jev" and u["run_id"] == rid and not u["shared_overhead"]]
        ev = (e or {}).get("json") or {}
        la = lane_assist(ev)
        kind, _ = classifier(la)
        for u in jev_rows:
            parts = u["source_dedup_key"].split(":")
            status = "ok" if "ok" in parts else ("fallback" if "fallback" in parts else None)
            if kind == "jev_ok" and status != "ok":
                errs.append(("auth", f"{label}: event は jev_ok だが同じ run の Jev 呼出記録 {u['dedup_key']} は成功を示さない"))
            if kind == "fallback" and status == "ok":
                errs.append(("auth", f"{label}: event は fallback だが同じ run の Jev 呼出記録 {u['dedup_key']} は成功"))
        vals = [u["cash_cost_usd"] if u["cash_cost_usd"] is not None else u["list_cost_usd"] for u in jev_rows]
        jev_cost = None if (not jev_rows or any(v is None for v in vals)) else sum(vals)
        cls = p["classifier"] or {}
        version = cls.get("snapshot") if cls.get("kind") == "jev" else None
        ev_line = {"event": ev}
        verdict = judge(ev_line, bool(jev_rows), jev_cost, version)
        trial = next(t for t in proj["records"] if t["record_type"] == "trial" and t["run_id"] == rid)
        joined.append({
            "trial": trial, "arm": p["arm"], "task_id": tid, "run_id": rid, "la": la, "verdict": verdict,
            "resolved": resolved_of(ev), "jev_calls": len(jev_rows), "jev_cost": jev_cost,
            "proposed_model": (p["proposed_first"] or {}).get("model_id"), "exec_model": r["e"]["exec_model"],
            "fallback_reason": r["e"]["fallback_reason"], "budget_stopped": False,
            "result": r["o"]["campaign_verdict"] or r["o"]["run_end"], "is_retry": parent is not None,
            "has_event": e is not None,
        })
    for eid, rids in ev_used.items():
        if len(rids) > 1:
            errs.append(("duplicate-event", f"routing_decided id={eid} が複数 run {rids} に使われている"))
    return joined, errs, unverified, unbound


def verify(evidence_dir, data, require_live, environ=None):
    """原記録と全件照合し、一致した場合だけ段階判定へ渡す。(errs, report)。SourceError は呼び出し側へ。"""
    campaign = data["summary"].get("campaign_id") if isinstance(data["summary"], dict) else None
    if not isinstance(campaign, str) or not campaign:
        return ["[schema] summary.campaign_id が無い（どの campaign の原記録と照合するか決まらない）"], {}
    src = reader.read_campaign(campaign, environ=environ, evidence_dir=evidence_dir)
    try:
        proj = rec.project(src)
    except rec.ProjectionError as e:
        return [f"[source] 原記録から export を投影できない（live 不可）: {e}"], {"campaign_id": campaign}
    errs = [f"[{c}] {m}" for c, m in reconcile(data, proj)]
    report = {"campaign_id": campaign, "manifest_hash_source": proj["manifest_hash"],
              "source": src["source"], "reconciled": not errs}
    if errs:
        # 全件照合が通らなければ段階判定へ渡さない（照合されていない値で集計しない）。
        return errs, report
    fail = lambda code, msg: errs.append(f"[{code}] {msg}")
    joined, berrs, unverified, unbound = bind(src, proj)
    for c, m in berrs:
        fail(c, m)
    depth = {}
    tree = {t: v["parent"] for t, v in proj["tasks"].items()}

    def depth_of(t):
        d, seen = 0, set()
        while tree.get(t) is not None and t not in seen:
            seen.add(t)
            t = tree[t]
            d += 1
        return d

    for t in tree:
        depth[t] = depth_of(t)
    if not joined:
        fail("no-trials", "原記録に campaign の run が 1 件も無い（実行の証拠なし）")
    cand_models = {"B": set(), "C": set()}
    for j in joined:
        t, arm = j["trial"], j["arm"]
        label = f"trial {j['run_id']}（arm {arm}, task {j['task_id']}）"
        if t.get("manifest_hash") != proj["manifest_hash"]:
            fail("mixed-manifest", f"{label}: manifest_hash {t.get('manifest_hash')!r} が読取元の campaign の hash と違う")
        if arm not in ARM_MODE:
            fail("mixed-arm", f"{label}: arm が B/C ではない: {arm!r}（主比較の A 等の混入）")
            continue
        if t.get("routing_mode") != ARM_MODE[arm]:
            fail("mixed-arm", f"{label}: routing_mode={t.get('routing_mode')!r}（期待 {ARM_MODE[arm]}）: B1 と C1 の混入")
        cand_models[arm] |= models_of(t)
        cands = t.get("proposed_candidates")
        if not isinstance(cands, list) or not cands:
            fail("candidates", f"{label}: proposed_candidates が空または欠測（凍結候補を確かめられない）")
        else:
            keys = []
            for c in cands:
                keys.append((c.get("source_id"), c.get("model_id"), c.get("harness_id")) if isinstance(c, dict) else None)
            if len(set(keys)) != len(keys) or set(keys) != FROZEN_CANDIDATES:
                fail("candidates", f"{label}: 候補 {keys} が凍結 manifest の Sonnet/Opus 共通候補 {sorted(FROZEN_CANDIDATES)} と"
                     "一致しない（source_id/model_id/harness_id・重複・未知 ID・単一モデル pin を含む）")
        pf = t.get("proposed_first")
        if isinstance(pf, dict) and (pf.get("source_id"), pf.get("model_id"), pf.get("harness_id")) not in FROZEN_CANDIDATES:
            fail("candidates", f"{label}: proposed_first {pf} が凍結候補に無い")
        if require_live and not ULID.match(j["task_id"] or ""):
            fail("not-live", f"{label}: task_id が実 task の ULID 形式ではない")
        if not j["exec_model"]:
            fail("exec-model", f"{label}: exec_model が欠測")
        else:
            ms = models_of(t)
            if ms and j["exec_model"] not in ms and not j["fallback_reason"]:
                fail("exec-model", f"{label}: exec_model={j['exec_model']} が提案候補 {sorted(ms)} に無い（fallback 理由もなし）")
        if not j["has_event"]:
            continue
        r = j["resolved"]
        if r is None:
            fail("pin", f"{label}: routing_resolved（task 単位の pin）が event に無い")
        else:
            if r.get("mode") != ARM_MODE[arm]:
                fail("pin", f"{label}: routing_resolved.mode={r.get('mode')!r} が arm の方策 {ARM_MODE[arm]!r} と違う")
            if r.get("evidence_mode") != "none" or r.get("evidence_table_id"):
                fail("pin", f"{label}: 根拠表を使っている（evidence_mode={r.get('evidence_mode')!r}）。B1/C1 は none")
            if r.get("online_update") is not False:
                fail("pin", f"{label}: online_update={r.get('online_update')!r}（B1/C1 は false 固定）")
            if r.get("source") != "task":
                fail("pin", f"{label}: routing_resolved.source={r.get('source')!r}（task 欄の pin ではない）")
        la = j["la"]
        if arm == "C":
            if la is None:
                fail("lane-assist", f"{label}: C1 に lane_assist が無い")
            elif la.get("overridden_by") == "explicit":
                fail("explicit-lane", f"{label}: 明示 lane が Jev の提案に勝った（C1 の明示 lane pin は不可）")
            if la is not None and la.get("real_api_verified") and j["verdict"]["kind"] != "jev_ok":
                fail("fake-success", f"{label}: real_api_verified=true だが classifier_status が jev_ok ではない（定数は証拠にしない）")
            for v in j["verdict"]["violations"]:
                fail("judge", f"{label}: {v}")
        elif j["verdict"]["kind"] in ("jev_ok", "fallback") or j["jev_calls"] > 0:
            fail("mixed-arm", f"{label}: B1（fixed）なのに Jev の分類/消費がある")
    for u in unbound:
        p = next((rr["p"] for rr in proj["run_map"].values() if rr["p"]["task_id"] == u["task_id"]), None)
        if p is not None and p["arm"] == "B":
            fail("mixed-arm", f"B1 の task {u['task_id']} に Jev 消費行 {u['dedup_key']} がある")

    def pin_of(j):
        r = j["resolved"] or {}
        return tuple(r.get(k) for k in ("mode",) + PIN_COMMON_KEYS)

    def common_of(j):
        r = j["resolved"] or {}
        return tuple(r.get(k) for k in PIN_COMMON_KEYS)

    common = {}
    for j in joined:
        if j["has_event"] and j["arm"] in ARM_MODE:
            common.setdefault(common_of(j), []).append(j["run_id"])
    if len(common) > 1:
        fail("condition-mismatch", f"B/C 全試行の共通条件（{PIN_COMMON_KEYS}）が mode 以外で割れている: "
             + "; ".join(f"{k}: {v}" for k, v in sorted(common.items(), key=str)))

    roots = {}
    for j in joined:
        if depth.get(j["task_id"]) == 0 and not j["is_retry"]:
            roots.setdefault(j["task_id"], j)
    for j in joined:
        rt = j["task_id"]
        while tree.get(rt) is not None:
            rt = tree[rt]
        rj = roots.get(rt)
        if rj is None:
            fail("pin", f"task {j['task_id']} の root {rt} の初回 run が無く pin を比べられない")
        elif j["arm"] != rj["arm"]:
            fail("mixed-arm", f"task {j['task_id']} の arm {j['arm']} が root {rt} の arm {rj['arm']} と違う")
        elif pin_of(j) != pin_of(rj):
            fail("pin", f"task {j['task_id']}（run {j['run_id']}）の pin {pin_of(j)} が root {rt} の pin {pin_of(rj)} と不一致")
    stages = {"total": new_stages(), "by_arm": {}}
    for j in joined:
        add_stage(stages["total"], j)
        add_stage(stages["by_arm"].setdefault(j["arm"] or "?", new_stages()), j)
    cj = [j for j in joined if j["arm"] == "C"]
    if require_live:
        if not cj:
            fail("coverage", "C1 の run が無い")
        if not any(j["verdict"]["live_proven"] for j in cj):
            fail("auth", "同じ run の原記録で jev_ok・Jev 呼出記録（run_id 結合）・費用・応答版（classifier_snapshot）が"
                 "揃った C1 観測が 1 件も無い（認証成功/実経路は未検証。fallback・欠測・task 単位の消費は成功に数えない）")
        for need, pred in (
            ("root", lambda j: depth.get(j["task_id"]) == 0),
            ("子", lambda j: depth.get(j["task_id"]) == 1),
            ("孫", lambda j: depth.get(j["task_id"]) == 2),
            ("retry", lambda j: j["is_retry"]),
        ):
            if not any(pred(j) for j in cj):
                fail("coverage", f"C1 に {need} の run が無い（pin/lane_assist/exec model を確かめられない）")
    s = proj["summary"]
    report.update({
        "trials": len(joined), "stages": stages,
        "candidate_models": {k: sorted(v) for k, v in cand_models.items()},
        "missing": {
            "jev_cost_missing": stages["total"]["jev_cost_missing"],
            "version_missing": stages["total"]["version_missing"],
            "model_unknown": stages["total"]["model_unknown"],
            "result_missing": stages["total"]["result_missing"],
            "incomplete_fields": s["incomplete"],
        },
        "unverified": unverified + [
            "許可 lane・天井（LaneLimits）は原記録に無い: 許可範囲の判定は lane_assist の記録内の整合だけ",
            "budget による抑止は原記録の run 行に無い: suppressed.budget は常に 0（未計上）",
        ],
        "suppressed": stages["total"]["suppressed"],
        "costs": {r: s["by_role"][r]["list_cost_usd"] for r in ROLES},
        "costs_cash": {r: s["by_role"][r]["cash_cost_usd"] for r in ROLES},
        "retry_cost": s["by_retry"]["retry"]["list_cost_usd"],
        "shared_overhead": s["shared_overhead"]["list_cost_usd"],
        "results": stages["total"]["results"],
    })
    return errs, report


def main(argv=None):
    ap = argparse.ArgumentParser(description="Jev B1/C1 の実 campaign export を独立読取元の原記録と照合する検証器")
    ap.add_argument("--evidence-dir", required=True)
    ap.add_argument("--require-live", action="store_true",
                    help="実 task_id・C1 の認証成功（同じ run の原記録で）・root/子/孫/retry の被覆を必須にする")
    ap.add_argument("--quiet", action="store_true", help="段階別 report を stdout に出さない")
    a = ap.parse_args(argv)
    try:
        data = load(a.evidence_dir)
        errs, report = verify(a.evidence_dir, data, a.require_live)
    except NoEvidence as e:
        print(str(e), file=sys.stderr)
        return 2
    except reader.SourceError as e:
        print(f"原記録の読取元が使えない（live 不可）: {e}", file=sys.stderr)
        return e.exit_code
    except (ValueError, OSError) as e:
        print(f"検証不合格: 入力を読めない: {e}", file=sys.stderr)
        return 1
    if not a.quiet:
        print(json.dumps(report, ensure_ascii=False, indent=2, sort_keys=True))
    if errs:
        for e in errs:
            print(f"FAIL {e}", file=sys.stderr)
        print(f"検証不合格: {len(errs)} 件", file=sys.stderr)
        return 1
    print("OK: 原記録と全件一致し live 証拠の検証に合格" if a.require_live
          else "OK: 原記録と全件一致（--require-live なし）", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
