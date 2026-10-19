#!/usr/bin/env python3
"""Jev 副比較: 独立読取元（jev_source_reader の抽出）から export を決定的に投影し、evidence と全件・全欄照合する。

信頼の根は jev_source_reader が開く運用者設定の読取元だけ。evidence の records / summary / provenance /
routing_events は「照合される側」で、どれも期待集合の元にしない。

- 投影は crates/task-core/src/decision_log_export.rs の build_campaign_export と、
  crates/task-core/src/store/decision_log.rs（read_run / decision_log_usage_list / metering）、
  crates/celerisctl/src/commands/campaign.rs の秘密除去（scrub）を移したもの。Rust 側を変えたらここも直す。
- manifest hash は読取元の task_campaigns の値だけを使う（export の CLI 引数 --manifest-hash 由来の
  summary.manifest_hash は照合対象で、証明にしない）。
- 照合は (record_type, dedup_key) 単位で、重複（同一内容を含む）・欠落・余剰（孤立・別 campaign 混入）・
  改変（未知欄・欠けた欄・値の違い）・順序違いを全て不合格にする。
- run への結び付けは実 ID だけで行う: routing_decided は event の run_id、消費は usage.run_id。task だけで
  結ぶ fallback はしない（task 単位の Jev 消費行は run 未結合として数え、どの run の証拠にもしない）。

標準ライブラリのみ。判定（段階集計）はしない。呼び出し側（jev-live-evidence.py）が使う。
"""
import datetime
import json
import re

SCHEMA = "celeris.decision-log/1"
ROLES = ["cos", "planner", "worker", "reviewer", "jev"]
INTERVENTION_KINDS = ["research_judgment", "manual_handoff", "recovery", "unclassified"]
RUN_ENDS = {"completed", "yielded", "budget_exhausted", "question", "failed", "harness_error", "cancelled",
            "waiting", "timeout"}
WU_VERDICTS = {"passed", "failed", "unverified"}
CAMPAIGN_VERDICTS = {"pass", "fail", "unscored"}
STAT_FIELDS = ["input_tokens", "output_tokens", "cache_read_tokens", "cache_creation_tokens", "list_cost_usd",
               "cash_cost_usd", "wall_ms", "cluster_wait_ms", "cluster_run_ms", "cluster_node_hours",
               "cluster_queue_ms"]
FLOAT_FIELDS = {"list_cost_usd", "cash_cost_usd", "cluster_node_hours"}
U32_MAX = 0xFFFFFFFF


class ProjectionError(Exception):
    """読取元の行から export を作れない（store が export に失敗する入力、または必要欄なし）。live 不可。"""


# ---------------------------------------------------------------- 秘密除去（celerisctl campaign.rs と同じ）
SECRET_PREFIXES = ["sk-", "ghp_", "gho_", "github_pat_", "xoxb-", "xoxp-", "AKIA", "eyJ"]
SECRET_KEYS = ["api_key", "apikey", "token", "secret", "password", "authorization"]
REDACTED = "[redacted]"


def _ascii_lower(s):
    return "".join(c.lower() if "A" <= c <= "Z" else c for c in s)


def scrub_str(s):
    out = []
    bearer = False
    for word in s.split(" "):
        lower = _ascii_lower(word)
        assign = None
        cut = [i for i in (word.find("="), word.find(":")) if i >= 0]
        if cut:
            k, v = word[:min(cut)], word[min(cut) + 1:]
            if v and any(_ascii_lower(k).endswith(sk) for sk in SECRET_KEYS):
                assign = k
        if bearer:
            out.append(REDACTED)
            bearer = False
        elif assign is not None:
            out.append(f"{assign}{'=' if '=' in word else ':'}{REDACTED}")
        elif len(word.encode("utf-8")) >= 16 and any(word.startswith(p) for p in SECRET_PREFIXES):
            out.append(REDACTED)
        else:
            out.append(word)
            bearer = lower == "bearer"
    return " ".join(out)


def scrub_value(v):
    if isinstance(v, str):
        return scrub_str(v)
    if isinstance(v, list):
        return [scrub_value(x) for x in v]
    if isinstance(v, dict):
        return {k: scrub_value(x) for k, x in v.items()}
    return v


# ---------------------------------------------------------------- 行 → Rust の型（store/decision_log.rs）
def _u32(v):
    return v if isinstance(v, int) and not isinstance(v, bool) and 0 <= v <= U32_MAX else None


def _u64(v):
    return v if isinstance(v, int) and not isinstance(v, bool) and v >= 0 else None


def _f(v):
    return None if v is None else float(v)


def _parse_json(text, what):
    if text is None:
        return None
    try:
        return json.loads(text)
    except ValueError as e:
        raise ProjectionError(f"{what} が JSON として読めない（store の export が失敗する）: {e}")


def _cand(c, what):
    if not isinstance(c, dict) or not all(isinstance(c.get(k), str) for k in ("source_id", "model_id", "harness_id")):
        raise ProjectionError(f"{what}: 候補が CandidateRef の形でない: {c!r}")
    return {"source_id": c["source_id"], "model_id": c["model_id"], "harness_id": c["harness_id"]}


def run_from_row(r):
    """decision_log_runs の 1 行 → (proposal, execution, outcome)。read_run と同じ規則。"""
    rid = r["run_id"]
    classifier = None
    if r.get("classifier_kind") is not None:
        classifier = {"kind": r["classifier_kind"], "model": r.get("classifier_model"),
                      "snapshot": r.get("classifier_snapshot"), "confidence": _f(r.get("classifier_confidence")),
                      "probs": _parse_json(r.get("classifier_probs_json"), f"run {rid} classifier_probs_json"),
                      "fallback": r.get("classifier_fallback")}
    first = None
    if r.get("proposed_first_source_id") is not None and r.get("proposed_first_model") is not None:
        first = {"source_id": r["proposed_first_source_id"], "model_id": r["proposed_first_model"],
                 "harness_id": r.get("proposed_first_harness") or ""}
    cands = _parse_json(r.get("proposed_candidates_json"), f"run {rid} proposed_candidates_json") or []
    excl = _parse_json(r.get("exclusions_json"), f"run {rid} exclusions_json") or []
    if not isinstance(cands, list) or not isinstance(excl, list):
        raise ProjectionError(f"run {rid}: proposed_candidates/exclusions が配列でない")
    exclusions = []
    for x in excl:
        if not isinstance(x, dict) or not isinstance(x.get("reason"), str):
            raise ProjectionError(f"run {rid}: exclusion の形が違う: {x!r}")
        exclusions.append({"candidate": _cand(x.get("candidate"), f"run {rid} exclusion"), "reason": x["reason"]})
    p = {
        "run_id": rid, "decision_id": r.get("decision_id"), "campaign_id": r.get("campaign_id"),
        "arm": r.get("arm"), "case_id": r.get("case_id"), "repetition": _u32(r.get("repetition")),
        "manifest_hash": r.get("manifest_hash"), "routing_setting": r.get("routing_setting"),
        "project_id": r.get("project_id"), "task_id": r.get("task_id"), "work_unit_id": r.get("work_unit_id"),
        "parent_run_id": r.get("parent_run_id"), "attempt_no": _u32(r.get("attempt_no")),
        "request_class": r.get("request_class"), "routing_mode": r.get("routing_mode"), "classifier": classifier,
        "propensity": _f(r.get("propensity")), "proposed_lane": r.get("proposed_lane"),
        "decided_lane": r.get("decided_lane"),
        "proposed_candidates": [_cand(c, f"run {rid} proposed_candidates") for c in cands],
        "proposed_first": first, "exclusions": exclusions,
        "policy_version": r.get("policy_version"), "prompt_version": r.get("prompt_version"),
        "question_schema_version": r.get("question_schema_version"),
        "provider_model_version": r.get("provider_model_version"), "features_hash": r.get("features_hash"),
        "options_hash": r.get("options_hash"),
    }
    e = {"exec_source_id": r.get("exec_source_id"), "exec_model": r.get("exec_model"),
         "exec_account": r.get("exec_account"), "exec_harness": r.get("exec_harness"),
         "exec_adapter": r.get("exec_adapter"), "fallback_reason": r.get("fallback_reason"),
         "attempts": _parse_json(r.get("attempts_json"), f"run {rid} attempts_json")}
    ret = r.get("run_retryable")
    o = {"run_end": r.get("run_end") if r.get("run_end") in RUN_ENDS else None, "run_end_raw": r.get("run_end_raw"),
         "run_retryable": None if ret is None else ret != 0,
         "wu_verdict": r.get("wu_verdict") if r.get("wu_verdict") in WU_VERDICTS else None,
         "wu_failure_class": r.get("wu_failure_class"),
         "campaign_verdict": r.get("campaign_verdict") if r.get("campaign_verdict") in CAMPAIGN_VERDICTS else None,
         "wall_ms": _u64(r.get("wall_ms"))}
    return {"p": p, "e": e, "o": o}


def usage_from_row(r):
    """decision_log_usage の 1 行 → usage（鍵は dedup_key から作り直す。decision_log_usage_list と同じ）。"""
    dedup = r["dedup_key"]
    if r.get("role") not in ROLES:
        raise ProjectionError(f"消費行 {dedup}: 未知の role {r.get('role')!r}（store の export が失敗する）")
    if dedup.startswith("run:"):
        run = r.get("run_id") or ""
        rest = dedup[len("run:"):]
        src = rest[len(run) + 1:] if rest.startswith(f"{run}:") else rest
        key = {"kind": "run", "run_id": run, "usage_source": src}
        k = f"run:{run}:{src}"
    elif dedup.startswith("decision:"):
        last = dedup.rsplit(":", 1)[-1]
        if not re.fullmatch(r"\+?[0-9]+", last) or int(last) > U32_MAX:
            raise ProjectionError(f"消費行の dedup_key が読めない: {dedup}")
        key = {"kind": "call", "decision_id": r.get("decision_id") or "", "call_attempt": int(last)}
        k = f"decision:{key['decision_id']}:{key['call_attempt']}"
    else:
        raise ProjectionError(f"消費行の dedup_key が読めない: {dedup}")
    u = {"key": key, "dedup_key": k, "source_dedup_key": dedup, "role": r["role"], "is_retry": r.get("is_retry", 0) != 0,
         "decision_id": r.get("decision_id"), "run_id": r.get("run_id"), "campaign_id": r.get("campaign_id"),
         "task_id": r.get("task_id"), "attribution_rule": r.get("attribution_rule"),
         "shared_overhead": (r.get("shared_overhead") or 0) != 0}
    for f in STAT_FIELDS:
        u[f] = _f(r.get(f)) if f in FLOAT_FIELDS else _u64(r.get(f))
    return u


# ---------------------------------------------------------------- 集計（decision_log_export.rs と同じ）
def _stat():
    return {"sum": None, "n_observed": 0, "n_missing": 0, "n_estimated": 0}


def _stat_add(s, v, estimated):
    if v is None:
        s["n_missing"] += 1
    else:
        s["sum"] = (0.0 if s["sum"] is None else s["sum"]) + float(v)
        s["n_observed"] += 1
        if estimated:
            s["n_estimated"] += 1


def _cons():
    c = {"n_rows": 0}
    for f in STAT_FIELDS:
        c[f] = _stat()
    return c


def _cons_add(c, u):
    c["n_rows"] += 1
    for f in STAT_FIELDS:
        _stat_add(c[f], u[f], f == "list_cost_usd")


def _observed(c):
    return any(c[f]["n_observed"] > 0 for f in STAT_FIELDS)


def _identity(p):
    return {k: (p or {}).get(k) for k in ("arm", "case_id", "repetition", "manifest_hash", "routing_setting")}


def _exec_same(run):
    first, e = run["p"]["proposed_first"], run["e"]
    if first is None or e["exec_source_id"] is None or e["exec_model"] is None:
        return None
    return first["source_id"] == e["exec_source_id"] and first["model_id"] == e["exec_model"]


def _ts_ns(s):
    m = re.fullmatch(r"(\d{4}-\d{2}-\d{2}[Tt]\d{2}:\d{2}:\d{2})(\.\d+)?([Zz]|[+-]\d{2}:\d{2})", s or "")
    if not m:
        raise ProjectionError(f"時刻が RFC 3339 として読めない: {s!r}")
    base = datetime.datetime.fromisoformat(m.group(1).replace("t", "T") + ("+00:00" if m.group(3) in "Zz" else m.group(3)))
    frac = (m.group(2) or ".")[1:]
    return int(base.timestamp()) * 10**9 + int((frac + "000000000")[:9])


def build_export(campaign, manifest_hash, runs, usage, interventions, started_at=None, last_activity_at=None):
    """build_campaign_export の移植。runs/usage/interventions は store の list と同じ campaign 一致の行。"""
    excluded = 0
    integ = {"duplicates_identical": 0, "conflicts": [], "excluded_manifest_mismatch": 0, "usage_without_run": 0}
    run_map, mismatched = {}, set()
    for r in runs:
        p = r["p"]
        if p["campaign_id"] != campaign:
            excluded += 1
        elif manifest_hash is not None and p["manifest_hash"] is not None and manifest_hash != p["manifest_hash"]:
            integ["excluded_manifest_mismatch"] += 1
            mismatched.add(p["run_id"])
        elif p["run_id"] in run_map:
            if run_map[p["run_id"]] == r:
                integ["duplicates_identical"] += 1
            else:
                integ["conflicts"].append(f"run:{p['run_id']}")
        else:
            run_map[p["run_id"]] = r
    run_ids = sorted(run_map)
    decision_ids = {run_map[k]["p"]["decision_id"] for k in run_ids if run_map[k]["p"]["decision_id"]}
    task_ids = {run_map[k]["p"]["task_id"] for k in run_ids if run_map[k]["p"]["task_id"]}
    usage_map = {}
    for u in usage:
        if u["run_id"] in mismatched:
            integ["excluded_manifest_mismatch"] += 1
            continue
        if u["campaign_id"] is not None:
            belongs = u["campaign_id"] == campaign
        else:
            belongs = u["run_id"] in run_map or u["decision_id"] in decision_ids
        if not belongs:
            excluded += 1
            continue
        k = u["dedup_key"]
        if k not in usage_map:
            usage_map[k] = u
        elif _usage_eq(usage_map[k], u):
            integ["duplicates_identical"] += 1
        else:
            integ["conflicts"].append(k)
    iv_map = {}
    for i in interventions:
        belongs = i["campaign_id"] == campaign if i.get("campaign_id") is not None else i.get("task_id") in task_ids
        if belongs:
            iv_map[i["intervention_id"]] = i
        else:
            excluded += 1

    def proposal_of(u):
        if u["run_id"] in run_map:
            return run_map[u["run_id"]]["p"]
        if u["decision_id"] is not None:
            for k in run_ids:
                if run_map[k]["p"]["decision_id"] == u["decision_id"]:
                    return run_map[k]["p"]
        if u["task_id"] is not None:
            for k in run_ids:
                if run_map[k]["p"]["task_id"] == u["task_id"]:
                    return run_map[k]["p"]
        return None

    by_role = {r: _cons() for r in ROLES}
    by_retry = {"initial": _cons(), "retry": _cons()}
    by_arm, total, shared, by_rule = {}, _cons(), _cons(), {}
    run_wall = _stat()
    records, trials = [], []
    for k in sorted(usage_map):
        u = usage_map[k]
        rule = u["attribution_rule"] or "none"
        by_rule[rule] = by_rule.get(rule, 0) + 1
        if u["key"]["kind"] == "run":
            _stat_add(run_wall, u["wall_ms"], False)
        p = proposal_of(u)
        if u["shared_overhead"]:
            _cons_add(shared, u)
        else:
            _cons_add(by_role.setdefault(u["role"], _cons()), u)
            _cons_add(by_retry["retry" if u["is_retry"] else "initial"], u)
            _cons_add(by_arm.setdefault((p or {}).get("arm") or "unassigned", _cons()), u)
            _cons_add(total, u)
        run_known = u["key"]["run_id"] in run_map if u["key"]["kind"] == "run" else True
        if not run_known:
            integ["usage_without_run"] += 1
        tmp = _cons()
        _cons_add(tmp, u)
        state = "shared_unattributable" if u["shared_overhead"] else ("observed" if _observed(tmp) else "missing")
        rec = {"record_type": "usage", "schema": SCHEMA, "dedup_key": k, "usage_key": u["key"],
               "usage_state": state, "run_known": run_known, "campaign_id": campaign,
               "attribution_rule": u["attribution_rule"], "shared_overhead": u["shared_overhead"],
               "run_id": u["run_id"], "decision_id": u["decision_id"], "task_id": u["task_id"], "role": u["role"],
               "is_retry": u["is_retry"], "list_cost_usd_estimated": True}
        for f in STAT_FIELDS:
            rec[f] = u[f]
        rec.update(_identity(p))
        records.append(rec)

    runs_by_end, wu_v, camp_v, evp, arms, seen = {}, {}, {}, {}, set(), set()

    def cnt(m, key):
        m[key] = m.get(key, 0) + 1

    for rid in run_ids:
        r = run_map[rid]
        p, e, o = r["p"], r["e"], r["o"]
        if p["arm"] is not None:
            arms.add(p["arm"])
        cnt(runs_by_end, o["run_end"] or "unrecorded")
        cnt(wu_v, o["wu_verdict"] or "unrecorded")
        cnt(camp_v, o["campaign_verdict"] or "unrecorded")
        same = _exec_same(r)
        cnt(evp, {True: "exec_is_proposed_first", False: "exec_differs", None: "exec_unknown"}[same])
        dkey = f"decision:{p['decision_id']}" if p["decision_id"] is not None else f"proposal:{rid}"
        c = p["classifier"] or {}
        if dkey not in seen:
            seen.add(dkey)
            rec = {"record_type": "decision", "schema": SCHEMA, "dedup_key": dkey, "campaign_id": campaign,
                   "project_id": p["project_id"], "task_id": p["task_id"], "work_unit_id": p["work_unit_id"],
                   "decision_id": p["decision_id"], "request_class": p["request_class"],
                   "routing_mode": p["routing_mode"], "classifier_kind": c.get("kind"),
                   "classifier_model": c.get("model"), "classifier_snapshot": c.get("snapshot"),
                   "classifier_confidence": c.get("confidence"), "classifier_probs": c.get("probs"),
                   "classifier_fallback": c.get("fallback"), "propensity": p["propensity"],
                   "proposed_lane": p["proposed_lane"], "decided_lane": p["decided_lane"],
                   "proposed_candidates": p["proposed_candidates"], "proposed_first": p["proposed_first"],
                   "exclusions": p["exclusions"], "policy_version": p["policy_version"],
                   "prompt_version": p["prompt_version"], "question_schema_version": p["question_schema_version"],
                   "provider_model_version": p["provider_model_version"], "features_hash": p["features_hash"],
                   "options_hash": p["options_hash"]}
            rec.update(_identity(p))
            records.append(rec)
        rec = {"record_type": "run", "schema": SCHEMA, "dedup_key": f"run:{rid}", "campaign_id": campaign,
               "task_id": p["task_id"], "work_unit_id": p["work_unit_id"], "run_id": rid,
               "decision_id": p["decision_id"], "parent_run_id": p["parent_run_id"], "attempt_no": p["attempt_no"],
               "is_retry": p["parent_run_id"] is not None, "exec_equals_proposed_first": same, **e}
        rec.update(_identity(p))
        records.append(rec)
        rec = {"record_type": "outcome", "schema": SCHEMA, "dedup_key": f"outcome:{rid}", "campaign_id": campaign,
               "run_id": rid, **o}
        rec.update(_identity(p))
        records.append(rec)
        trials.append(_trial(campaign, r, usage_map, run_map, run_ids))

    ivs = {k: {"count": 0, "wall_ms": _stat()} for k in INTERVENTION_KINDS}
    for iid in sorted(iv_map):
        i = iv_map[iid]
        if i["kind"] not in INTERVENTION_KINDS:
            raise ProjectionError(f"介入 {iid}: 未知の kind {i['kind']!r}（store の export が失敗する）")
        s = ivs[i["kind"]]
        s["count"] += 1
        _stat_add(s["wall_ms"], _u64(i.get("wall_ms")), False)
        records.append({"record_type": "intervention", "schema": SCHEMA, "dedup_key": f"intervention:{iid}",
                        "campaign_id": campaign, "intervention_id": iid, "kind": i["kind"], "task_id": i.get("task_id"),
                        "at": i["at"], "wall_ms": _u64(i.get("wall_ms")), "source_event": i.get("source_event")})
    incomplete = [f for f in STAT_FIELDS if total[f]["n_missing"] > 0]
    if total["n_rows"] == 0:
        incomplete.append("usage")
    incomplete.append("quota")
    elapsed = None
    if started_at is not None and last_activity_at is not None:
        diff = _ts_ns(last_activity_at) - _ts_ns(started_at)
        elapsed = diff // 10**6 if diff > 0 else 0
    summary = {
        "schema": SCHEMA, "campaign_id": campaign, "export_cursor": None, "manifest_hash": manifest_hash,
        "arms": sorted(arms), "n_runs": len(run_map), "by_role": by_role, "by_retry": by_retry, "by_arm": by_arm,
        "total": total, "shared_overhead": shared, "by_attribution_rule": by_rule, "elapsed_ms": elapsed,
        "run_wall_sum_ms": run_wall,
        "cluster": {"wait_ms": total["cluster_wait_ms"], "run_ms": total["cluster_run_ms"],
                    "node_hours": total["cluster_node_hours"], "queue_ms": total["cluster_queue_ms"]},
        "runs_by_end": runs_by_end, "wu_verdicts": wu_v, "campaign_verdicts": camp_v,
        "interventions_by_kind": ivs, "exec_vs_proposed": evp, "incomplete": incomplete, "quota": None,
        "excluded_rows": excluded, "integrity": integ,
    }
    return {"records": scrub_value(records + trials), "summary": scrub_value(summary),
            "run_map": run_map, "usage_map": usage_map}


def _usage_eq(a, b):
    return {k: v for k, v in a.items() if k != "source_dedup_key"} == {k: v for k, v in b.items() if k != "source_dedup_key"}


def _trial(campaign, r, usage_map, run_map, run_ids):
    p, e, o = r["p"], r["e"], r["o"]
    rid = p["run_id"]
    first_of_task = p["task_id"] is not None and next(
        (k for k in run_ids if run_map[k]["p"]["task_id"] == p["task_id"]), None) == rid
    own, task_level = _cons(), _cons()
    for k in sorted(usage_map):
        u = usage_map[k]
        if u["shared_overhead"]:
            continue
        if u["run_id"] == rid:
            _cons_add(own, u)
        elif u["run_id"] is None and first_of_task and u["task_id"] is not None and u["task_id"] == p["task_id"]:
            _cons_add(task_level, u)
    if _observed(own) or _observed(task_level):
        state = "observed"
    elif own["n_rows"] + task_level["n_rows"] > 0:
        state = "missing"
    elif e["exec_model"] is None and o["run_end"] is None:
        state = "not_executed"
    else:
        state = "unmetered"
    c = p["classifier"] or {}
    rec = {"record_type": "trial", "schema": SCHEMA, "dedup_key": f"trial:{rid}", "campaign_id": campaign,
           "run_id": rid, "task_id": p["task_id"], "work_unit_id": p["work_unit_id"], "decision_id": p["decision_id"],
           "attempt_no": p["attempt_no"], "is_retry": p["parent_run_id"] is not None,
           "request_class": p["request_class"], "routing_mode": p["routing_mode"], "proposed_first": p["proposed_first"],
           "proposed_candidates": p["proposed_candidates"], "decided_lane": p["decided_lane"],
           "exec_source_id": e["exec_source_id"], "exec_model": e["exec_model"], "exec_harness": e["exec_harness"],
           "exec_equals_proposed_first": _exec_same(r), "fallback_reason": e["fallback_reason"],
           "classifier_kind": c.get("kind"), "classifier_model": c.get("model"),
           "classifier_snapshot": c.get("snapshot"), "classifier_fallback": c.get("fallback"),
           "prompt_version": p["prompt_version"], "question_schema_version": p["question_schema_version"],
           "provider_model_version": p["provider_model_version"], "policy_version": p["policy_version"],
           "run_end": o["run_end"], "wu_verdict": o["wu_verdict"], "grader_verdict": o["campaign_verdict"],
           "consumption_state": state, "run_consumption": own, "task_level_consumption": task_level}
    rec.update(_identity(p))
    return rec


# ---------------------------------------------------------------- 読取元の抽出 → 期待集合
def project(src):
    """jev_source_reader の抽出 → 期待する export・task 木・routing_events と、原記録側の不足（problems）。"""
    campaign = src["campaign"]["campaign_id"]
    problems = []
    hashes = src["campaign"]["manifest_hashes"]
    if len(hashes) != 1:
        raise ProjectionError(f"読取元の task_campaigns に manifest_hash が 1 つに決まらない: {hashes}")
    mhash = hashes[0]
    runs = [run_from_row(r) for r in src["decision_runs"]]
    exported_usage = [usage_from_row(u) for u in src["usage"] if u.get("campaign_id") == campaign]
    for u in src["usage"]:
        if u.get("campaign_id") is None:
            problems.append(("usage-not-exported",
                             f"読取元の消費行 {u['dedup_key']}（role {u.get('role')}）は run/decision でこの campaign に属すが "
                             "campaign_id が NULL で export に出ない（費用が欠ける）"))
    started = min((r["created_at"] for r in src["campaign"]["roots"] if r.get("created_at")), default=None)
    ex = build_export(campaign, mhash, runs, exported_usage, src.get("interventions", []), started,
                      src.get("last_activity_at"))
    tasks = {t["id"]: {"parent": t["parent_id"]} for t in src["tasks"]}
    routing_events = [{"task_id": e["task_id"], "run_id": (e["json"] or {}).get("run_id"), "event": e["json"]}
                      for e in src["routing_decided"]]
    for d in src.get("diagnostics", []):
        if d["code"] in ("decision_run_outside_campaign_tree", "manifest_hash_mixed", "manifest_hash_missing"):
            problems.append(("source", f"{d['code']}: {d['detail']}"))
    return {"campaign_id": campaign, "manifest_hash": mhash, "records": ex["records"], "summary": ex["summary"],
            "run_map": ex["run_map"], "usage_map": ex["usage_map"], "tasks": tasks,
            "routing_events": routing_events, "problems": problems}


# ---------------------------------------------------------------- 照合
def canon(v):
    return json.dumps(v, sort_keys=True, ensure_ascii=False)


def _diff_fields(want, got, prefix=""):
    out = []
    if isinstance(want, dict) and isinstance(got, dict):
        for k in sorted(set(want) | set(got)):
            if k not in got:
                out.append(f"{prefix}{k}: 欄が無い")
            elif k not in want:
                out.append(f"{prefix}{k}: 原記録に無い未知欄")
            else:
                out.extend(_diff_fields(want[k], got[k], f"{prefix}{k}."))
        return out
    if canon(want) != canon(got):
        return [f"{prefix.rstrip('.')}: 原記録={canon(want)[:80]} export={canon(got)[:80]}"]
    return []


def reconcile_lines(name, want, got, key, campaign=None):
    """want/got は dict の列。key(row) で 1 件を識別。重複・欠落・余剰・改変・順序を (code, msg) で返す。"""
    out = []
    wk = [key(w) for w in want]
    seen, gk = {}, []
    for n, g in enumerate(got, 1):
        k = key(g) if isinstance(g, dict) else None
        if k is None:
            out.append(("extra", f"{name}:{n}: 識別できない行（record_type/dedup_key 等が無い）"))
            continue
        if k in seen:
            same = canon(seen[k]) == canon(g)
            out.append(("duplicate", f"{name}:{n}: {k} が重複（{'同一内容' if same else '内容違い'}）"))
            continue
        seen[k] = g
        gk.append(k)
    wmap = dict(zip(wk, want))
    for k in wk:
        if k not in seen:
            out.append(("missing", f"{name}: 原記録の {k} が欠落"))
    for k in gk:
        if k not in wmap:
            g = seen[k]
            if campaign is not None and isinstance(g, dict) and g.get("campaign_id") not in (None, campaign):
                out.append(("mixed", f"{name}: {k} は別 campaign {g.get('campaign_id')!r} の行（混入）"))
            else:
                out.append(("extra", f"{name}: {k} は原記録に無い（孤立・余剰）"))
        else:
            for d in _diff_fields(wmap[k], seen[k]):
                out.append(("altered", f"{name}: {k}: {d}"))
    if not out and gk != wk:
        out.append(("order", f"{name}: 行の順序が原記録からの投影と違う"))
    return out


def record_key(r):
    if "record_type" not in r or "dedup_key" not in r:
        return None
    return (r["record_type"], r["dedup_key"])
