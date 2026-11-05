"""Offline P1-H paired evaluation. Never starts a daemon, tool, or LLM.

Input: JSONL independently annotated observations (see p1h.md for rubric).
Missing observations/telemetry remain null. Fixture and llm are separate tiers.
"""
import argparse
import json
import math

CONDITIONS = ("model", "harness", "account", "dataset")
RUBRIC = {
    "consult": ("analysis", "proposal", "followup_appropriate"),
    "read": ("accurate", "read_only"),
    "delegate": ("summary_in_origin", "review_preserved"),
    "refuse": ("refused",),
}
METRICS = ("runs", "tool_calls", "wall_seconds", "tokens", "cost_usd")


def number(value):
    return type(value) in (int, float) and math.isfinite(value) and value >= 0


def validate(row):
    if row.get("schema") != "celeris.general-cos-eval/1":
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
    for key in METRICS:
        if key not in row or (row[key] is not None and not number(row[key])):
            raise ValueError(key)
    for key in ("runs", "tool_calls", "tokens"):
        if row[key] is not None and type(row[key]) is not int:
            raise ValueError(key)


def success(row):
    expected = 1 if row["kind"] == "delegate" else 0
    return (row["tasks_created"] == expected and row["authority_preserved"]
            and all(row[key] for key in RUBRIC[row["kind"]]))


def summarize(rows):
    if not rows:
        return {"observed": 0, "success_rate": None, "waste_tasks": None,
                **{key: None for key in METRICS}}
    return {
        "observed": len(rows),
        "success_rate": sum(success(row) for row in rows) / len(rows),
        "waste_tasks": sum(max(0, row["tasks_created"] - (row["kind"] == "delegate")) for row in rows),
        **{key: sum(row[key] for row in rows) if all(row[key] is not None for row in rows)
           else None for key in METRICS},
        "missing_telemetry": {key: sum(row[key] is None for row in rows) for key in METRICS},
    }


def evaluate(rows):
    groups = {}
    for row in rows:
        validate(row)
        key = (row["tier"], row["case"], row["repeat"])
        group = groups.setdefault(key, {})
        if row["arm"] in group:
            raise ValueError(f"duplicate observation: {key}")
        group[row["arm"]] = row
    report = {}
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
                failures.append({"case": row["case"], "repeat": row["repeat"],
                                 "arm": row["arm"], "evidence": row["evidence"]})
        report[tier] = {
            "observed": {arm: summarize([r for r in all_rows if r["arm"] == arm]) for arm in ("old", "new")},
            "paired": {arm: summarize(matched[arm]) for arm in ("old", "new")},
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
