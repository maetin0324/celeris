#!/usr/bin/env python3
"""P2-G（ADR 2026-11-03-llm-self-improve-trial）: 試行の子 task の LLM 利用・時間・やり直し・人の介入を DB から読み、1 子 1 行の JSONL を書く。
  python3 -I scripts/dev/llm-improve/collect.py --db <celeris.sqlite3> --parent <task id> --out <usage.jsonl>
DB は読み取り専用で開く（`mode=ro`）。書き込みはしない。親の子のうち題名が「P2-G 試行:」で始まるものを読む。
欄: task_id・title・status・attempts（task の attempts）・runs（worker の run の数）・runs_by_outcome・models（adapter・model・provider・account）・
usage（input・output・cache read・cache creation の token と cost_usd の和）・wall_secs（metrics.wall_ms の和）・
first_started・last_finished（event の ts）・elapsed_secs（最初の worker_started から最後の worker_finished まで）・
check_failures（work_unit_check_finished の失敗と、command の review_verdict の fail）・review_fail（transitioned の review_fail）・
human_interventions（answered・decision_answered・comment の event のうち主体が human: のもの）・human_events（同じ event の種類）・
by_role（worker_started の role で分けた run の数・token・cost・wall。role の無い run は "worker"、review の run は "reviewer"）。
"""
import argparse, json, sqlite3, sys
from datetime import datetime


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--db", required=True)
    ap.add_argument("--parent", required=True)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    c = sqlite3.connect(f"file:{a.db}?mode=ro", uri=True, timeout=30)
    kids = c.execute("SELECT id, title, status, json FROM tasks WHERE parent_id = ? AND title LIKE 'P2-G 試行:%' ORDER BY id",
                     (a.parent,)).fetchall()
    if not kids:
        print("collect: 試行の子 task が無い", file=sys.stderr)
        sys.exit(1)
    rows = []
    for tid, title, status, tj in kids:
        t = json.loads(tj)
        evs = [(e_ts, json.loads(j)) for e_ts, j in
               c.execute("SELECT ts, json FROM events WHERE task_id = ? ORDER BY id", (tid,))]
        usage = {"input_tokens": 0, "output_tokens": 0, "cache_read_tokens": 0, "cache_creation_tokens": 0, "cost_usd": 0.0}
        wall_ms, runs, outcomes, models = 0, 0, {}, []
        started, finished = [], []
        run_role, by_role = {}, {}
        check_fail, review_fail, human, human_kinds = 0, 0, 0, []
        for e_ts, e in evs:
            ty = e.get("type")
            if ty == "worker_started":
                started.append(e_ts)
                run_role[e.get("run_id")] = e.get("role") or "worker"
                m = {k: e.get(k) for k in ("adapter", "model", "provider", "account")}
                if m not in models:
                    models.append(m)
            elif ty == "worker_finished":
                runs += 1
                finished.append(e_ts)
                kind = (e.get("outcome") or "").split(":", 1)[0]
                outcomes[kind] = outcomes.get(kind, 0) + 1
                u = e.get("usage") or {}
                for k in usage:
                    usage[k] += u.get(k) or 0
                wall_ms += ((e.get("metrics") or {}).get("wall_ms") or 0)
                r = by_role.setdefault(run_role.get(e.get("run_id"), "worker"),
                                       {"runs": 0, "output_tokens": 0, "cache_read_tokens": 0, "cost_usd": 0.0, "wall_secs": 0.0})
                r["runs"] += 1
                r["output_tokens"] += u.get("output_tokens") or 0
                r["cache_read_tokens"] += u.get("cache_read_tokens") or 0
                r["cost_usd"] = round(r["cost_usd"] + (u.get("cost_usd") or 0), 4)
                r["wall_secs"] = round(r["wall_secs"] + ((e.get("metrics") or {}).get("wall_ms") or 0) / 1000, 1)
            elif ty == "work_unit_check_finished" and e.get("ok") is False:
                check_fail += 1
            elif ty == "review_verdict" and not e.get("pass") and str(e.get("reason", "")).startswith("cmd="):
                check_fail += 1
            elif ty == "transitioned" and e.get("reason") == "review_fail":
                review_fail += 1
            if ty in ("answered", "decision_answered") or "comment" in (ty or ""):
                by = e.get("by") or e.get("actor") or e.get("author") or ""
                if isinstance(by, str) and by.startswith("human"):
                    human += 1
                    human_kinds.append(ty)
        usage["cost_usd"] = round(usage["cost_usd"], 4)
        el = None
        if started and finished:
            f = lambda v: datetime.strptime(v[:19], "%Y-%m-%dT%H:%M:%S")
            el = (f(finished[-1]) - f(started[0])).total_seconds()
        rows.append({"schema": "celeris.llm-improve-usage/1", "task_id": tid, "title": title, "status": status,
                     "attempts": t.get("attempts"), "runs": runs, "runs_by_outcome": outcomes, "models": models,
                     "usage": usage, "wall_secs": round(wall_ms / 1000, 1),
                     "first_started": started[0] if started else None, "last_finished": finished[-1] if finished else None,
                     "elapsed_secs": el, "check_failures": check_fail, "review_fail": review_fail,
                     "human_interventions": human, "human_events": human_kinds, "by_role": by_role})
    with open(a.out, "w", encoding="utf-8") as f:
        for r in rows:
            f.write(json.dumps(r, ensure_ascii=False) + "\n")
    for r in rows:
        print(f"{r['task_id']} {r['status']} runs={r['runs']} cost={r['usage']['cost_usd']} wall={r['wall_secs']}s "
              f"checks_failed={r['check_failures']} human={r['human_interventions']} by_role={json.dumps(r['by_role'])} {r['title']}")


if __name__ == "__main__":
    main()
