"""Offline P1-H/P4-E paired evaluation. Never starts a daemon, tool, or LLM.

Input: JSONL independently annotated observations (schema celeris.general-cos-eval/2; rubric in
agent-docs/adr/2026-11-05-phase4-p4e.md). Rows come from
`scripts/dev/general-cos-compare/compare.py observations`.
Missing observations/telemetry remain null. Fixture and llm are separate tiers.
"""
import argparse
import json
import math

SCHEMA = "celeris.general-cos-eval/2"
CONDITIONS = ("model", "harness", "account", "dataset")
RUBRIC = {
    "consult": ("analysis", "proposal", "followup_appropriate"),
    "read": ("accurate", "read_only"),
    "delegate": ("summary_in_origin", "review_preserved"),
    "refuse": ("refused",),
}
KINDS = tuple(RUBRIC)
# 回数・token は整数、秒・費用は実数。null は欠測（0 にしない）。
INT_METRICS = ("runs", "tool_calls", "tokens", "cache_read_tokens", "cache_creation_tokens", "handoffs",
               "human_interventions")
REAL_METRICS = ("wall_seconds", "first_reply_seconds", "cost_usd")
METRICS = INT_METRICS + REAL_METRICS


def number(value):
    return type(value) in (int, float) and math.isfinite(value) and value >= 0


def validate(row):
    if row.get("schema") != SCHEMA:
        raise ValueError("schema")
    if row.get("tier") not in ("fixture", "llm") or row.get("arm") not in ("old", "new"):
        raise ValueError("tier/arm")
    for key in ("case", "evidence", "policy", *CONDITIONS):
        if not isinstance(row.get(key), str) or not row[key].strip():
            raise ValueError(key)
    if type(row.get("repeat")) is not int or row["repeat"] < 1:
        raise ValueError("repeat")
    kind = row.get("kind")
    if kind not in RUBRIC:
        raise ValueError("kind")
    if type(row.get("tasks_created")) is not int or row["tasks_created"] < 0:
        raise ValueError("tasks_created")
    for key in ("authority_preserved", *RUBRIC[kind]):
        if type(row.get(key)) is not bool:
            raise ValueError(key)
    if kind == "consult":
        # 0 受付だけ・1 質問だけ・2 分析か提案の片方・3 分析と提案（不足があれば具体的な追問）。
        if type(row.get("consult_depth")) is not int or not 0 <= row["consult_depth"] <= 3:
            raise ValueError("consult_depth")
    for key in METRICS:
        if key not in row or (row[key] is not None and not number(row[key])):
            raise ValueError(key)
    for key in INT_METRICS:
        if row[key] is not None and type(row[key]) is not int:
            raise ValueError(key)


def expected_tasks(row):
    return 1 if row["kind"] == "delegate" else 0


def success(row):
    return (row["tasks_created"] == expected_tasks(row) and row["authority_preserved"]
            and all(row[key] for key in RUBRIC[row["kind"]]))


def total(rows, key):
    return sum(row[key] for row in rows) if all(row[key] is not None for row in rows) else None


def summarize(rows):
    if not rows:
        return {"observed": 0, "success_rate": None, "by_kind": {}, "waste_tasks": None, "consult_depth_mean": None,
                **{key: None for key in METRICS}, "missing_telemetry": {}}
    consult = [row["consult_depth"] for row in rows if row["kind"] == "consult"]
    by_kind = {}
    for kind in KINDS:
        mine = [row for row in rows if row["kind"] == kind]
        if mine:
            by_kind[kind] = {"observed": len(mine), "success_rate": sum(success(r) for r in mine) / len(mine)}
    return {
        "observed": len(rows),
        "success_rate": sum(success(row) for row in rows) / len(rows),
        "by_kind": by_kind,
        "waste_tasks": sum(max(0, row["tasks_created"] - expected_tasks(row)) for row in rows),
        "consult_depth_mean": sum(consult) / len(consult) if consult else None,
        **{key: total(rows, key) for key in METRICS},
        "missing_telemetry": {key: sum(row[key] is None for row in rows) for key in METRICS},
    }


def delta(old, new):
    """新 − 旧（paired の組だけ）。どちらかが null なら null。"""
    keys = ("success_rate", "waste_tasks", "consult_depth_mean", *METRICS)
    return {key: (None if old.get(key) is None or new.get(key) is None else new[key] - old[key]) for key in keys}


def evaluate(rows):
    groups = {}
    for row in rows:
        validate(row)
        key = (row["tier"], row["case"], row["repeat"])
        group = groups.setdefault(key, {})
        if row["arm"] in group:
            raise ValueError(f"duplicate observation: {key}")
        group[row["arm"]] = row
    report = {"schema": "celeris.general-cos-eval-report/2"}
    for tier in ("fixture", "llm"):
        matched = {"old": [], "new": []}
        unpaired = []
        failures = []
        all_rows = [row for row in rows if row["tier"] == tier]
        for (group_tier, case, repeat), group in sorted(groups.items()):
            if group_tier != tier:
                continue
            if len(group) != 2:
                unpaired.append({"case": case, "repeat": repeat, "missing": "new" if "old" in group else "old"})
                continue
            old, new = group["old"], group["new"]
            if any(old[key] != new[key] for key in (*CONDITIONS, "kind")):
                raise ValueError(f"unmatched conditions: {case}/{repeat}")
            for arm, row in group.items():
                matched[arm].append(row)
        for row in all_rows:
            if not success(row):
                failures.append({"case": row["case"], "repeat": row["repeat"], "arm": row["arm"],
                                 "evidence": row["evidence"],
                                 "failed": [k for k in ("authority_preserved", *RUBRIC[row["kind"]]) if not row[k]]
                                 + ([f"tasks_created={row['tasks_created']}"]
                                    if row["tasks_created"] != expected_tasks(row) else [])})
        paired = {arm: summarize(matched[arm]) for arm in ("old", "new")}
        report[tier] = {
            "observed": {arm: summarize([r for r in all_rows if r["arm"] == arm]) for arm in ("old", "new")},
            "paired": paired,
            "paired_delta_new_minus_old": delta(paired["old"], paired["new"]),
            "unpaired": unpaired, "failures": failures,
        }
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("records", help="annotated JSONL, no credentials or secrets")
    args = parser.parse_args()
    try:
        with open(args.records, encoding="utf-8") as source:
            rows = [json.loads(line) for line in source if line.strip()]
        print(json.dumps(evaluate(rows), ensure_ascii=False, indent=2, allow_nan=False))
    except (ValueError, OSError, TypeError, KeyError) as error:
        parser.exit(2, f"general-cos-eval: {error}\n")


if __name__ == "__main__":
    main()
