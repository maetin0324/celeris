#!/usr/bin/env python3
"""Jev 副比較（B1/C1）の実 `celerisctl campaign export` 出力の検証器。

外部ネットワーク・LLM は使わない（標準ライブラリのみ）。入力は evidence dir:

  records.jsonl        celerisctl campaign export の records（usage/decision/run/outcome/intervention/trial）
  summary.json         同 export の CampaignSummary
  provenance.json      export の出自（source=celerisctl campaign export, campaign_id, manifest_hash,
                       generated_at, export_command, files_sha256, tasks{task_id:{parent}}）
  routing_events.jsonl routing_decided event の写し（1 行 {task_id, run_id?, event, allowed?, max_lane?, fixed_lane?}）

判定は crates/campaign/src/jev_smoke.rs の judge と crates/campaign/src/jev_stages.rs の段定義に合わせる。
`real_api_verified`（本体側の定数）は証拠にしない。欠測は欠測のまま数え、成功に数えない。

終了コード: 0 合格 / 1 検証不合格 / 2 live 証拠なし（dir が無い・空・必須 file なし）。
"""
import argparse
import datetime
import hashlib
import json
import os
import re
import sys

SCHEMA = "celeris.decision-log/1"
ROLES = ["cos", "planner", "worker", "reviewer", "jev"]
LANE_RANK = {"cheap": 0, "standard": 1, "frontier": 2}
ULID = re.compile(r"^[0-9A-HJKMNP-TV-Z]{26}$")
HEX64 = re.compile(r"^[0-9a-f]{64}$")
EXPORT_SOURCE = "celerisctl campaign export"
FILES = ["records.jsonl", "summary.json", "provenance.json", "routing_events.jsonl"]
ARM_MODE = {"B": "fixed", "C": "decision_assisted"}
TOL = 1e-6


class NoEvidence(Exception):
    pass


def read_json(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def read_jsonl(path):
    out = []
    with open(path, encoding="utf-8") as f:
        for n, line in enumerate(f, 1):
            line = line.strip()
            if line:
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


def sha256_file(path):
    return hashlib.sha256(open(path, "rb").read()).hexdigest()


def lane_assist(ev):
    for ptr in (("record", "lane_assist"), ("lane_assist",)):
        cur = ev
        for k in ptr:
            cur = cur.get(k) if isinstance(cur, dict) else None
        if isinstance(cur, dict):
            return cur
    return None


def resolved_of(ev):
    cur = ev
    for k in ("record", "features", "decision", "routing_resolved"):
        cur = cur.get(k) if isinstance(cur, dict) else None
    return cur if isinstance(cur, dict) else None


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


def is_timestamp(ts):
    try:
        datetime.datetime.fromisoformat(ts.replace("Z", "+00:00"))
        return True
    except (ValueError, AttributeError):
        return False


def models_of(trial):
    return {c.get("model_id") for c in (trial.get("proposed_candidates") or []) if c.get("model_id")}


def verify(evidence_dir, data, require_live):
    errs = []
    fail = lambda code, msg: errs.append(f"[{code}] {msg}")
    records, summary, prov, events = data["records"], data["summary"], data["prov"], data["events"]
    campaign = summary.get("campaign_id")
    mhash = summary.get("manifest_hash")

    # --- 出自（provenance）: 手組み fixture を live にしない ---
    if prov.get("source") != EXPORT_SOURCE:
        fail("provenance", f"provenance.source が {EXPORT_SOURCE!r} ではない: {prov.get('source')!r}（手組み fixture は live 証拠にならない）")
    if prov.get("fixture") or prov.get("synthetic") or prov.get("hand_made"):
        fail("provenance", "provenance が fixture/synthetic/hand_made を宣言している")
    if not campaign or prov.get("campaign_id") != campaign:
        fail("provenance", f"campaign_id が summary と provenance で一致しない: {campaign!r} / {prov.get('campaign_id')!r}")
    if not (isinstance(mhash, str) and HEX64.match(mhash)) or prov.get("manifest_hash") != mhash:
        fail("provenance", f"manifest_hash が 64 hex でない、または summary と provenance で不一致: {mhash!r} / {prov.get('manifest_hash')!r}")
    if not is_timestamp(prov.get("generated_at")):
        fail("provenance", f"generated_at が時刻として読めない: {prov.get('generated_at')!r}")
    cmd = prov.get("export_command") or ""
    if EXPORT_SOURCE not in cmd or (campaign and campaign not in cmd):
        fail("provenance", f"export_command に実コマンドと campaign_id が無い: {cmd!r}")
    for fn in ("records.jsonl", "summary.json"):
        want = (prov.get("files_sha256") or {}).get(fn)
        if want != sha256_file(os.path.join(evidence_dir, fn)):
            fail("provenance", f"files_sha256[{fn}] が実 file と一致しない（export 後に改変、または記録なし）")
    if summary.get("schema") != SCHEMA:
        fail("schema", f"summary.schema が {SCHEMA} ではない: {summary.get('schema')!r}")
    tasks_map = prov.get("tasks") if isinstance(prov.get("tasks"), dict) else {}
    if not tasks_map:
        fail("provenance", "provenance.tasks（task 木: task_id -> {parent}）が無い")

    # --- fake 成功の定数 ---
    # real_api_verified は証拠にしない: classifier_status・費用・版だけを見る（下の judge）。
    # --- 混入 ---
    by_type = {}
    for r in records:
        if r.get("schema") != SCHEMA:
            fail("schema", f"record schema 不一致: {r.get('record_type')} {r.get('dedup_key')}")
        if r.get("campaign_id") != campaign:
            fail("mixed-campaign", f"別 campaign の行が混入: {r.get('dedup_key')} campaign_id={r.get('campaign_id')!r}（期待 {campaign!r}）")
        by_type.setdefault(r.get("record_type"), []).append(r)
    trials = by_type.get("trial", [])
    usages = by_type.get("usage", [])
    if not trials:
        fail("no-trials", "trial record が 1 件も無い（実行の証拠なし）")
    if summary.get("n_runs") != len(trials):
        fail("count", f"summary.n_runs={summary.get('n_runs')} と trial 数 {len(trials)} が不一致")
    run_ids = {t.get("run_id") for t in trials}
    task_ids = {t.get("task_id") for t in trials}
    for t in trials:
        if t.get("manifest_hash") != mhash:
            fail("mixed-manifest", f"trial {t.get('run_id')} の manifest_hash が export と違う（別 B1/C1 条件の混入）")
        arm = t.get("arm")
        if arm not in ARM_MODE:
            fail("mixed-arm", f"trial {t.get('run_id')} の arm が B/C ではない: {arm!r}（主比較の A 等の混入）")
        elif t.get("routing_mode") != ARM_MODE[arm]:
            fail("mixed-arm", f"trial {t.get('run_id')} arm={arm} だが routing_mode={t.get('routing_mode')!r}（期待 {ARM_MODE[arm]}）: B1 と C1 の混入")
    for u in usages:
        if u.get("run_id") not in run_ids and u.get("task_id") not in task_ids:
            fail("mixed-usage", f"どの trial にも属さない消費行: {u.get('dedup_key')}")
        if u.get("run_id") and u["run_id"] not in run_ids:
            fail("mixed-usage", f"消費行 {u.get('dedup_key')} の run_id が trial に無い")

    # --- task 木・実 task_id 連結 ---
    for tid in task_ids:
        if tid not in tasks_map:
            fail("task-link", f"trial の task_id {tid} が provenance.tasks に無い")
        if require_live and not (isinstance(tid, str) and ULID.match(tid)):
            fail("not-live", f"task_id {tid!r} が実 task の ULID 形式ではない（手組み fixture）")
    depth = {}

    def depth_of(tid, seen=()):
        if tid in depth:
            return depth[tid]
        parent = (tasks_map.get(tid) or {}).get("parent")
        if tid in seen:
            fail("task-link", f"task 木に循環: {tid}")
            return 0
        d = 0 if parent is None else 1 + depth_of(parent, seen + (tid,))
        if parent is not None and parent not in tasks_map:
            fail("task-link", f"task {tid} の親 {parent} が provenance.tasks に無い")
        depth[tid] = d
        return d

    def root_of(tid):
        seen = set()
        while (tasks_map.get(tid) or {}).get("parent") and tid not in seen:
            seen.add(tid)
            tid = tasks_map[tid]["parent"]
        return tid

    for tid in tasks_map:
        depth_of(tid)

    # --- routing event の突き合わせ（pin / lane_assist / exec model） ---
    ev_by_key = {}
    for e in events:
        ev_by_key[(e.get("task_id"), e.get("run_id"))] = e
    joined = []
    for t in trials:
        tid, rid, arm = t.get("task_id"), t.get("run_id"), t.get("arm")
        e = ev_by_key.get((tid, rid)) or ev_by_key.get((tid, None))
        if e is None:
            fail("no-event", f"trial {rid}（task {tid}）に対応する routing_decided event が routing_events に無い")
            continue
        ev = e.get("event") or {}
        la = lane_assist(ev)
        run_usages = [u for u in usages if u.get("role") == "jev" and (u.get("run_id") == rid or (u.get("run_id") is None and u.get("task_id") == tid))]
        costs = [u.get("list_cost_usd") for u in run_usages]
        cash = [u.get("cash_cost_usd") for u in run_usages]
        cost_vals = [c if c is not None else k for c, k in zip(cash, costs)]
        jev_cost = None if (not run_usages or any(c is None for c in cost_vals)) else sum(cost_vals)
        version = t.get("classifier_snapshot") or t.get("provider_model_version")
        verdict = judge(e, bool(run_usages), jev_cost, version)
        resolved = resolved_of(ev)
        joined.append({
            "trial": t, "arm": arm, "task_id": tid, "run_id": rid, "la": la, "verdict": verdict,
            "resolved": resolved, "jev_calls": len(run_usages), "jev_cost": jev_cost,
            "proposed_model": ((t.get("proposed_first") or {}).get("model_id")),
            "exec_model": t.get("exec_model"), "fallback_reason": t.get("fallback_reason"),
            "budget_stopped": False, "result": t.get("grader_verdict") or t.get("run_end"),
            "is_retry": bool(t.get("is_retry")), "ev_line": e,
        })

    cand_models = {"B": set(), "C": set()}
    for j in joined:
        t, arm = j["trial"], j["arm"]
        label = f"trial {j['run_id']}（arm {arm}, task {j['task_id']}）"
        if arm in cand_models:
            cand_models[arm] |= models_of(t)
        # 同じ arm の root と pin が揃うか（子・孫・retry）
        # exec model
        if not j["exec_model"]:
            fail("exec-model", f"{label}: exec_model が欠測")
        else:
            ms = models_of(t)
            if ms and j["exec_model"] not in ms and not j["fallback_reason"]:
                fail("exec-model", f"{label}: exec_model={j['exec_model']} が提案候補 {sorted(ms)} に無い（fallback 理由もなし）")
        r = j["resolved"]
        if r is None:
            fail("pin", f"{label}: routing_resolved（task 単位の pin）が event に無い")
        else:
            if r.get("mode") != ARM_MODE.get(arm):
                fail("pin", f"{label}: routing_resolved.mode={r.get('mode')!r} が arm の方策 {ARM_MODE.get(arm)!r} と違う")
            if r.get("evidence_mode") != "none" or r.get("evidence_table_id"):
                fail("pin", f"{label}: 根拠表を使っている（evidence_mode={r.get('evidence_mode')!r}）。B1/C1 は none")
            if r.get("source") != "task":
                fail("pin", f"{label}: routing_resolved.source={r.get('source')!r}（task 欄の pin ではない）")
        la = j["la"]
        if arm == "C":
            if la is None:
                fail("lane-assist", f"{label}: C1 に lane_assist が無い")
            elif la.get("overridden_by") == "explicit":
                fail("explicit-lane", f"{label}: 明示 lane が Jev の提案に勝った（C1 の明示 lane pin は不可）")
            if la is not None and "real_api_verified" in la and la["real_api_verified"] and j["verdict"]["kind"] != "jev_ok":
                fail("fake-success", f"{label}: real_api_verified=true だが classifier_status が jev_ok ではない（定数は証拠にしない）")
        elif arm == "B":
            if j["verdict"]["kind"] in ("jev_ok", "fallback") or j["jev_calls"] > 0:
                fail("mixed-arm", f"{label}: B1（fixed）なのに Jev の分類/消費がある")
        if arm == "C":
            for v in j["verdict"]["violations"]:
                fail("judge", f"{label}: {v}")
    # 子/孫/retry の pin が root と同じ
    def pin_of(j):
        r = j["resolved"] or {}
        return (r.get("mode"), r.get("lower_policy"), r.get("evidence_mode"))

    roots = {}
    for j in joined:
        if depth.get(j["task_id"]) == 0:
            roots.setdefault(j["task_id"], j)
    for j in joined:
        rt = root_of(j["task_id"])
        rj = roots.get(rt)
        if rj is None:
            fail("pin", f"task {j['task_id']} の root {rt} の trial/event が無く pin を比べられない")
        elif j["arm"] != rj["arm"]:
            fail("mixed-arm", f"task {j['task_id']} の arm {j['arm']} が root {rt} の arm {rj['arm']} と違う")
        elif pin_of(j) != pin_of(rj):
            fail("pin", f"task {j['task_id']} の pin {pin_of(j)} が root {rt} の pin {pin_of(rj)} と不一致（子/孫/retry で継承されていない）")
    # retry の親 run
    for j in joined:
        t = j["trial"]
        if j["is_retry"]:
            pr = next((x for x in records if x.get("record_type") == "run" and x.get("run_id") == j["run_id"]), {}).get("parent_run_id")
            if not pr or pr not in run_ids:
                fail("retry", f"retry {j['run_id']} の parent_run_id {pr!r} が trial に無い")

    # C1 候補集合
    if cand_models["C"]:
        has = lambda sub: any(sub in m for m in cand_models["C"])
        if not (has("sonnet") and has("opus")):
            fail("single-model", f"C1 の候補集合が Sonnet と Opus の両方を含まない（単一モデル pin）: {sorted(cand_models['C'])}")

    # --- 費用の突き合わせ（全役割・retry） ---
    by_role = summary.get("by_role") or {}
    for role in ROLES:
        if role not in by_role:
            fail("cost", f"summary.by_role に {role} が無い")
            continue
        rows = [u for u in usages if u.get("role") == role and not u.get("shared_overhead")]
        s = by_role[role]
        if s.get("n_rows") != len(rows):
            fail("cost", f"role {role}: summary n_rows={s.get('n_rows')} と record の {len(rows)} が不一致")
        for field in ("list_cost_usd", "cash_cost_usd"):
            vals = [u.get(field) for u in rows if u.get(field) is not None]
            got = (s.get(field) or {}).get("sum")
            if (len(vals) == 0) != (got is None) or (vals and abs(sum(vals) - got) > TOL):
                fail("cost", f"role {role}: {field} の和 summary={got} と record={sum(vals) if vals else None} が不一致")
            if (s.get(field) or {}).get("n_observed") != len(vals):
                fail("cost", f"role {role}: {field} の観測数が record と不一致")
    retry_rows = [u for u in usages if u.get("is_retry") and not u.get("shared_overhead")]
    sr = (summary.get("by_retry") or {}).get("retry") or {}
    if sr.get("n_rows") != len(retry_rows):
        fail("cost", f"retry 消費: summary n_rows={sr.get('n_rows')} と record の {len(retry_rows)} が不一致")
    rv = [u.get("list_cost_usd") for u in retry_rows if u.get("list_cost_usd") is not None]
    got_rv = (sr.get("list_cost_usd") or {}).get("sum")
    if rv and (got_rv is None or abs(sum(rv) - got_rv) > TOL):
        fail("cost", "retry 費用の和が summary と record で不一致")

    # --- live に必要な範囲 ---
    stages = {"total": new_stages(), "by_arm": {}}
    for j in joined:
        add_stage(stages["total"], j)
        add_stage(stages["by_arm"].setdefault(j["arm"] or "?", new_stages()), j)
    cj = [j for j in joined if j["arm"] == "C"]
    if require_live:
        if not cj:
            fail("coverage", "C1 の trial が無い")
        if not any(j["verdict"]["live_proven"] for j in cj):
            fail("auth", "jev_ok・費用・応答版が揃った C1 観測が 1 件も無い（認証成功/実経路は未確認。fallback・欠測は成功に数えない）")
        for need, pred in (
            ("root", lambda j: depth.get(j["task_id"]) == 0),
            ("子", lambda j: depth.get(j["task_id"]) == 1),
            ("孫", lambda j: depth.get(j["task_id"]) == 2),
            ("retry", lambda j: j["is_retry"]),
        ):
            if not any(pred(j) for j in cj):
                fail("coverage", f"C1 に {need} の trial が無い（pin/lane_assist/exec model を確かめられない）")
        if not any(r.get("role") == "jev" for r in usages):
            fail("coverage", "Jev の消費行が 1 件も無い")
    report = {
        "campaign_id": campaign, "manifest_hash": mhash,
        "generated_at": prov.get("generated_at"), "trials": len(trials),
        "stages": stages, "candidate_models": {k: sorted(v) for k, v in cand_models.items()},
        "missing": {
            "jev_cost_missing": stages["total"]["jev_cost_missing"],
            "version_missing": stages["total"]["version_missing"],
            "model_unknown": stages["total"]["model_unknown"],
            "result_missing": stages["total"]["result_missing"],
            "incomplete_fields": summary.get("incomplete"),
        },
        "suppressed": stages["total"]["suppressed"],
        "costs": {r: (by_role.get(r) or {}).get("list_cost_usd") for r in ROLES},
        "retry_cost": sr.get("list_cost_usd"),
    }
    return errs, report


def main(argv=None):
    ap = argparse.ArgumentParser(description="Jev B1/C1 の実 campaign export 検証器")
    ap.add_argument("--evidence-dir", required=True)
    ap.add_argument("--require-live", action="store_true",
                    help="provenance・実 task_id・C1 の認証成功・root/子/孫/retry の被覆を必須にする")
    ap.add_argument("--quiet", action="store_true", help="段階別 report を stdout に出さない")
    a = ap.parse_args(argv)
    try:
        data = load(a.evidence_dir)
        errs, report = verify(a.evidence_dir, data, a.require_live)
    except NoEvidence as e:
        print(str(e), file=sys.stderr)
        return 2
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
    print("OK: live 証拠の検証に合格" if a.require_live else "OK: 検証に合格（--require-live なし）", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
