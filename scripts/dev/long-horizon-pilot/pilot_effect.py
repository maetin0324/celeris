#!/usr/bin/env python3
"""pilot の公開相当の effect stub（実際の外部通信はしない）。ADR 2026-11-05-a9-llm-pilot-findings D3。

    pilot_effect.py publish <logical_id> --dir <effect dir> [--timeout <secs>]

effect dir は試行の --state の下（worker の作業場所の外）。worker（LLM）は command を呼ぶだけで、記録は書き換えない
（`effects.jsonl` は driver だけが読み、facts の `effects` になる）。
- 承認待ち: `pending/<id>.json` を置き、operator（driver）が `decisions/<id>.json` に approve / reject を書くまで待つ。
- approve: `effects.jsonl` に 1 行（時刻・logical_id・approved）を追記する。同じ logical_id は 1 回だけ（重複排除）。
- reject: 何も記録せず exit 4。承認が来なければ exit 5。
`--unapproved`（陰性の試験用）は承認を待たずに記録する。
"""
import argparse
import json
import pathlib
import sys
import time


def publish(directory, logical_id, timeout, unapproved=False):
    d = pathlib.Path(directory)
    (d / "pending").mkdir(parents=True, exist_ok=True)
    (d / "decisions").mkdir(parents=True, exist_ok=True)
    record = d / "effects.jsonl"
    key = logical_id.replace("/", "_")
    if not unapproved:
        pending = d / "pending" / f"{key}.json"
        if not pending.exists():
            pending.write_text(json.dumps({"logical_id": logical_id, "at": time.time()}))
        decision = d / "decisions" / f"{key}.json"
        deadline = time.time() + timeout
        while not decision.exists():
            if time.time() > deadline:
                return 5
            time.sleep(0.2)
        if json.loads(decision.read_text()).get("decision") != "approve":
            return 4
    done = {json.loads(x)["logical_id"] for x in record.read_text().splitlines()} if record.exists() else set()
    if logical_id not in done:
        with open(record, "a", encoding="utf-8") as f:
            f.write(json.dumps({"at": time.time(), "logical_id": logical_id, "approved": not unapproved}) + "\n")
    return 0


def main():
    p = argparse.ArgumentParser()
    p.add_argument("verb", choices=["publish"])
    p.add_argument("logical_id")
    p.add_argument("--dir", required=True)
    p.add_argument("--timeout", type=float, default=120)
    p.add_argument("--unapproved", action="store_true")
    a = p.parse_args()
    sys.exit(publish(a.dir, a.logical_id, a.timeout, a.unapproved))


if __name__ == "__main__":
    main()
