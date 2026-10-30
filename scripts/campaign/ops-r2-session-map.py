#!/usr/bin/env python3
"""ops-r2 の実験専用 operator session の対応表を作る（LLM を呼ばない。ADR 2026-10-30-ops-r2-isolation-principal D7）。

subcommand:
  base    --session-id ID --trials T --out-map M --out-trials S
          試行開始前に、試行ごとの prep・observe の区間から session-map.base.json（operator_usage::Mapping の配列。
          schema は今のまま）と、trial_key ↔ condition_id・request_ref の対応 session-trials.json を作る。
  final   --base M --trials S --human-ops-list L --out OUT
          `celeris-campaign human-op list` の出力（訂正を解いた記録）から、この session の介入（actor が
          external_operator かつ id が session id の op_start/op_end）を拾い、observe・prep の区間を intervene で分ける。
          同じ入力なら byte 一致。
  summary --trials S --usage U --snapshot DIR --out OUT
          operator-usage.jsonl のこの session の行を (category, trial_key) で合計する。取れない消費は 0 にせず
          null と理由（transcript_missing・usage_missing・not_recorded）を書く。
"""
import argparse
import hashlib
import json
import os
import sys
from datetime import datetime

SCHEMA_TRIALS = "ops-r2-session-trials/1"
SCHEMA_SUMMARY = "ops-r2-operator-usage-summary/1"
TOKENS = ("input_tokens", "output_tokens", "cache_read_tokens", "cache_creation_tokens")
SPLITTABLE = ("prep", "observe")


class Fail(Exception):
    pass


def ts(s, where):
    if s is None:
        return None
    if not isinstance(s, str) or "T" not in s:
        raise Fail(f"{where}: RFC3339 の時刻が要る（{s!r}）")
    try:
        t = datetime.fromisoformat(s.replace("Z", "+00:00"))
    except ValueError as e:
        raise Fail(f"{where}: 時刻が読めない {s!r}: {e}") from None
    if t.tzinfo is None:
        raise Fail(f"{where}: 時差の無い時刻 {s!r}")
    return t


def key_start(m):
    t = ts(m["start"], "start")
    return (0, "") if t is None else (1, t.timestamp())


def read_json(p):
    with open(p, encoding="utf-8") as f:
        return json.load(f)


def write_json(p, v):
    data = json.dumps(v, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    tmp = p + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        f.write(data)
    os.replace(tmp, p)
    return hashlib.sha256(data.encode()).hexdigest()


def nonempty(v, where):
    if not isinstance(v, str) or not v.strip():
        raise Fail(f"{where}: 空でない文字列が要る")
    return v


def validate_map(entries):
    """operator_usage::validate_map と同じ規則（区間の逆転・同じ session の重複・shared と trial_key）。"""
    for i, m in enumerate(entries):
        nonempty(m["session_id"], f"mapping {i} session_id")
        if m["category"] not in ("prep", "observe", "intervene", "shared"):
            raise Fail(f"mapping {i}: category {m['category']!r}")
        if (m["category"] == "shared") != (m["trial_key"] is None):
            raise Fail(f"mapping {i}: shared だけが trial_key=null")
        s, e = ts(m["start"], f"mapping {i} start"), ts(m["end"], f"mapping {i} end")
        if s is not None and e is not None and s >= e:
            raise Fail(f"mapping {i}: start は end より前")
        for j, o in enumerate(entries[:i]):
            if o["session_id"] != m["session_id"]:
                continue
            os_, oe = ts(o["start"], "start"), ts(o["end"], "end")
            if not ((e is not None and os_ is not None and e <= os_) or (oe is not None and s is not None and oe <= s)):
                raise Fail(f"mapping {i} と {j}: 区間が重なる")


def entry(session, start, end, category, trial_key):
    return {"session_id": session, "start": start, "end": end, "category": category, "trial_key": trial_key}


def base(a):
    session = nonempty(a.session_id, "--session-id")
    spec = read_json(a.trials)
    trials = spec.get("trials") if isinstance(spec, dict) else None
    if not isinstance(trials, list) or not trials:
        raise Fail("--trials: {\"trials\": [...]} に 1 件以上の試行が要る")
    allowed = {"trial_key", "condition_id", "request_ref", "prep", "observe"}
    seen, rows, out = set(), [], []
    for i, t in enumerate(trials):
        if not isinstance(t, dict) or set(t) - allowed:
            raise Fail(f"trial {i}: 知らない欄 {sorted(set(t) - allowed) if isinstance(t, dict) else t!r}")
        key = nonempty(t.get("trial_key"), f"trial {i} trial_key")
        if key in seen:
            raise Fail(f"trial {i}: trial_key の重複 {key}")
        seen.add(key)
        req = nonempty(t.get("request_ref"), f"trial {i} request_ref")
        if ":" not in req:
            raise Fail(f"trial {i}: request_ref は SOURCE_KIND:SOURCE_KEY")
        rows.append({"trial_key": key, "condition_id": nonempty(t.get("condition_id"), f"trial {i} condition_id"),
                     "request_ref": req})
        if "observe" not in t:
            raise Fail(f"trial {i}: observe の区間が要る")
        for cat in SPLITTABLE:
            w = t.get(cat)
            if w is None:
                continue
            if not isinstance(w, dict) or set(w) != {"start", "end"}:
                raise Fail(f"trial {i}: {cat} は {{start, end}}")
            out.append(entry(session, w["start"], w["end"], cat, key))
    out.sort(key=key_start)
    validate_map(out)
    write_json(a.out_map, out)
    write_json(a.out_trials, {"schema": SCHEMA_TRIALS, "session_id": session, "trials": rows})
    print(json.dumps({"mappings": len(out), "trials": len(rows)}))


def op_spans(records, session):
    """この session の op を (trial_key, logical_op_id, attempt, op_kind) で対にする。端が欠ければ拒否する。"""
    pairs = {}
    for r in records:
        actor = r.get("actor") or {}
        if r.get("record_type") not in ("op_start", "op_end"):
            continue
        if actor.get("kind") != "external_operator" or actor.get("id") != session:
            continue
        k = (r.get("trial_key"), r.get("logical_op_id"), r.get("attempt"), r.get("op_kind"))
        p = pairs.setdefault(k, {"cond": r.get("condition_id")})
        side = "start" if r["record_type"] == "op_start" else "end"
        if side in p:
            raise Fail(f"op {k}: {r['record_type']} が 2 行ある")
        p[side] = r.get(side)
        if r.get("condition_id") is not None:
            p["cond"] = r.get("condition_id")
    spans = []
    for k, p in sorted(pairs.items(), key=lambda kv: json.dumps(kv[0], default=str)):
        if p.get("start") is None or p.get("end") is None:
            raise Fail(f"op {k}: 開始か終了の時刻が無い（区間を決められないので session-map を作らない）")
        s, e = ts(p["start"], f"op {k} start"), ts(p["end"], f"op {k} end")
        if s >= e:
            # 点の操作（開始＝終了）は長さ 0。その瞬間の応答は無いので区間を作らない。
            continue
        spans.append({"trial_key": k[0], "cond": p["cond"], "start": p["start"], "end": p["end"], "s": s, "e": e})
    return spans


def inside(s, e, m):
    ms, me = ts(m["start"], "start"), ts(m["end"], "end")
    return (ms is None or ms <= s) and (me is None or e <= me)


def overlaps(s, e, m):
    ms, me = ts(m["start"], "start"), ts(m["end"], "end")
    return (me is None or s < me) and (ms is None or ms < e)


def final(a):
    base_map = read_json(a.base)
    trials = read_json(a.trials)
    if trials.get("schema") != SCHEMA_TRIALS:
        raise Fail("--trials: session-trials.json ではない")
    session = trials["session_id"]
    conds = {t["trial_key"]: t["condition_id"] for t in trials["trials"]}
    validate_map(base_map)
    if any(m["session_id"] != session for m in base_map):
        raise Fail("--base: 別の session の行がある")
    listing = read_json(a.human_ops_list)
    records = listing.get("records") if isinstance(listing, dict) else None
    if not isinstance(records, list):
        raise Fail("--human-ops-list: `celeris-campaign human-op list` の出力（records）が要る")
    spans = op_spans(records, session)
    by_entry = {i: [] for i in range(len(base_map))}
    extra = []
    for sp in spans:
        key = sp["trial_key"]
        hits = [i for i, m in enumerate(base_map) if inside(sp["s"], sp["e"], m)]
        if key is None:
            if any(overlaps(sp["s"], sp["e"], m) for m in base_map):
                raise Fail(f"共有の op {sp['start']}..{sp['end']} が試行の区間に重なる")
            extra.append(entry(session, sp["start"], sp["end"], "shared", None))
            continue
        if key not in conds:
            raise Fail(f"op の trial_key {key} は session-trials.json に無い")
        if sp["cond"] is not None and sp["cond"] != conds[key]:
            raise Fail(f"op の condition_id {sp['cond']} が試行 {key} の {conds[key]} と違う")
        own = [i for i in hits if base_map[i]["trial_key"] == key and base_map[i]["category"] in SPLITTABLE]
        if len(own) != 1:
            raise Fail(f"op {sp['start']}..{sp['end']} は試行 {key} の prep・observe の区間 1 つに収まらない")
        by_entry[own[0]].append(sp)
    out = list(extra)
    for i, m in enumerate(base_map):
        cuts = sorted(by_entry[i], key=lambda x: (x["s"], x["e"]))
        merged = []
        for sp in cuts:
            if merged and sp["s"] <= merged[-1]["e"]:
                if sp["e"] > merged[-1]["e"]:
                    merged[-1].update(e=sp["e"], end=sp["end"])
                continue
            merged.append(dict(sp))
        cursor = m["start"]
        for sp in merged:
            if cursor is None or ts(cursor, "start") < sp["s"]:
                out.append(entry(session, cursor, sp["start"], m["category"], m["trial_key"]))
            out.append(entry(session, sp["start"], sp["end"], "intervene", m["trial_key"]))
            cursor = sp["end"]
        if m["end"] is None or ts(cursor, "start") is None or ts(cursor, "start") < ts(m["end"], "end"):
            out.append(entry(session, cursor, m["end"], m["category"], m["trial_key"]))
    out.sort(key=key_start)
    validate_map(out)
    digest = write_json(a.out, out)
    print(json.dumps({"mappings": len(out), "intervene": sum(m["category"] == "intervene" for m in out),
                      "sha256": digest}))


def summary(a):
    trials = read_json(a.trials)
    if trials.get("schema") != SCHEMA_TRIALS:
        raise Fail("--trials: session-trials.json ではない")
    session = trials["session_id"]
    main = os.path.join(a.snapshot, "main.jsonl")
    groups = {}
    for t in trials["trials"]:
        for cat in ("prep", "observe", "intervene"):
            groups[(cat, t["trial_key"])] = []
    for cat in ("shared", "shared_unattributable"):
        groups[(cat, None)] = []
    session_reason = None
    if not os.path.isfile(main):
        session_reason = "transcript_missing"
    else:
        rows = []
        if os.path.isfile(a.usage):
            with open(a.usage, encoding="utf-8") as f:
                rows = [json.loads(l) for l in f if l.strip()]
        rows = [r for r in rows if r.get("session_id") == session]
        if not rows:
            session_reason = "not_recorded"
        for r in rows:
            groups.setdefault((r["category"], r["trial_key"]), []).append(r)
    out = []
    for (cat, key), rows in sorted(groups.items(), key=lambda kv: (kv[0][0], kv[0][1] or "")):
        cond = next((t for t in trials["trials"] if t["trial_key"] == key), None)
        g = {"category": cat, "trial_key": key,
             "condition_id": cond and cond["condition_id"], "request_ref": cond and cond["request_ref"],
             "messages": None if session_reason else len(rows), "reason": session_reason}
        for tok in TOKENS:
            vals = [r.get(tok) for r in rows]
            g[tok] = None if session_reason or None in vals else sum(vals)
        if not session_reason:
            missing = sum(any(r.get(tok) is None for tok in TOKENS) for r in rows)
            if missing:
                g["reason"] = f"usage_missing:{missing}"
        out.append(g)
    write_json(a.out, {"schema": SCHEMA_SUMMARY, "session_id": session, "reason": session_reason, "groups": out})
    print(json.dumps({"groups": len(out), "reason": session_reason}))


def main(argv):
    p = argparse.ArgumentParser(prog="ops-r2-session-map.py")
    sub = p.add_subparsers(dest="cmd", required=True)
    b = sub.add_parser("base")
    b.add_argument("--session-id", required=True)
    b.add_argument("--trials", required=True)
    b.add_argument("--out-map", required=True)
    b.add_argument("--out-trials", required=True)
    f = sub.add_parser("final")
    f.add_argument("--base", required=True)
    f.add_argument("--trials", required=True)
    f.add_argument("--human-ops-list", required=True)
    f.add_argument("--out", required=True)
    s = sub.add_parser("summary")
    s.add_argument("--trials", required=True)
    s.add_argument("--usage", required=True)
    s.add_argument("--snapshot", required=True)
    s.add_argument("--out", required=True)
    a = p.parse_args(argv)
    try:
        {"base": base, "final": final, "summary": summary}[a.cmd](a)
    except (Fail, KeyError, json.JSONDecodeError, OSError) as e:
        print(f"ops-r2-session-map: {e}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
