#!/usr/bin/env python3
"""本番 DB を読み取り専用（sqlite の mode=ro）で読み、fixture と再監査の JSON を標準出力に出す。

    python3 -I agent-docs/progress/2026-11-21-strategy-v1-1/decision-failclosed/extract.py fixture
    python3 -I agent-docs/progress/2026-11-21-strategy-v1-1/decision-failclosed/extract.py audit

DB への書き込みはしない（接続は URI の mode=ro だけ）。fixture には type・seq・ts・状態・進行の文の必要な欄だけを残し、
commit の hash・account・path・コマンドの本文は落とす。reproduce.py はこの script を呼ばず、DB も読まない。
"""
import json
import pathlib
import re
import sqlite3
import subprocess
import sys

DB_URI = "file:/local/celeris/data/db/celeris.sqlite3?mode=ro"
RELEASES = pathlib.Path("/local/celeris/state/releases")
LOGS = pathlib.Path("/local/celeris/state/logs")
TASK = "01M7Q5ZABN9NFQCH6QBTTP3EZ8"
PROGRESS_SEQ = 436
CONTEXT_SEQS = (419, 422)
PROGRESS_PREFIX = "worker の決定の要求を記録できませんでした"
FIX_SUBJECT = "worker の決定: 受け付けられない要求を 1 件の決定にまとめて人に出し、その仕事を止める"
REJECTED_KEY = re.compile(r"^rejected(-[0-9]+)?$")
# 事象の worker の報告のうち、人の確認を求めたと書いた部分（ほかの文は入れない）
OUTCOME_MARK = "人の確認を decision work-units-size-trim で求めた（integrate-impl の前まで）"
MSG_MARK = "so I need your OK on that before integrate-impl."

KEEP = {
    "work_unit_transitioned": ("key", "from", "to", "reason"),
    "transitioned": ("from", "to", "reason"),
    "work_unit_check_finished": ("key", "index", "total", "pass"),
    "integration_check_finished": ("key", "index", "total", "pass"),
    "work_unit_check_started": ("key", "index", "total"),
    "integration_check_started": ("key", "index", "total"),
    "work_unit_committed": ("key",),
    "phase_integrated": ("phase",),
    "worker_progress": ("run_id",),
    "worker_finished": ("run_id",),
    "worker_started": ("run_id",),
    "decision_requested": (),
    "decision_answered": (),
}


def connect():
    return sqlite3.connect(DB_URI, uri=True)


def slim(seq, ts, raw):
    ev = json.loads(raw)
    kind = ev.get("type")
    out = {"seq": seq, "ts": ts, "type": kind}
    for k in KEEP.get(kind, ()):
        if k in ev:
            out[k] = ev[k]
    if kind == "worker_progress":
        msg = ev.get("msg", "")
        if msg.startswith(PROGRESS_PREFIX):
            out["msg"] = msg
        elif MSG_MARK in msg:
            out["msg_excerpt"] = MSG_MARK
    if kind == "worker_finished" and OUTCOME_MARK in ev.get("outcome", ""):
        out["outcome_excerpt"] = OUTCOME_MARK
    return out


def fixture():
    con = connect()
    rows = con.execute(
        "select seq, ts, json from events where task_id=? order by seq", (TASK,)
    ).fetchall()
    done = next(
        s for s, _, j in rows
        if s > PROGRESS_SEQ and json.loads(j).get("type") == "transitioned"
        and json.loads(j).get("to") == "done"
    )
    context = [slim(s, t, j) for s, t, j in rows if s in CONTEXT_SEQS]
    events = [slim(s, t, j) for s, t, j in rows if PROGRESS_SEQ - 1 <= s <= done]
    progress_ts = next(t for s, t, _ in rows if s == PROGRESS_SEQ)
    drows = con.execute(
        "select key, kind, status, needed_before_json, created_at, answered_at,"
        " json_extract(json,'$.raised_by.origin') from decisions where task_id=? order by created_at",
        (TASK,),
    ).fetchall()
    decision_ids = [
        r[0] for r in con.execute("select id from decisions where task_id=?", (TASK,))
    ]
    inbox_after = 0
    for did in decision_ids:
        inbox_after += con.execute(
            "select count(*) from cos_inbox_items where source_key=? and created_at>=?",
            ("decision-" + did, progress_ts),
        ).fetchone()[0]
    inbox_mention = con.execute(
        "select count(*) from cos_inbox_items where created_at>=? and"
        " (source_key like ? or summary like ? or detail like ? or detail like ?)",
        (progress_ts, "%" + TASK + "%", "%" + TASK + "%", "%" + TASK + "%",
         "%work-units-size-trim%"),
    ).fetchone()[0]
    return {
        "schema": "decision-failclosed-fixture/1",
        "source": "events・decisions・cos_inbox_items（sqlite3 " + DB_URI + "）",
        "task_id": TASK,
        "unit": "target-leaf",
        "requested_decision": {
            "key": "work-units-size-trim",
            "needed_before": ["integrate-impl"],
        },
        "decision_rows": [
            {
                "key": k, "kind": kd, "status": st, "needed_before": json.loads(nb),
                "created_at": ca, "answered_at": aa, "origin": og,
            }
            for k, kd, st, nb, ca, aa, og in drows
        ],
        "decisions_rows_after_progress": sum(1 for r in drows if r[4] > progress_ts),
        "inbox_items_after_progress": inbox_after + inbox_mention,
        "context_before": context,
        "events": events,
    }


def git(*args):
    return subprocess.run(("git",) + args, capture_output=True, text=True)


def contains_fix(fix, rel):
    if git("cat-file", "-e", rel + "^{commit}").returncode != 0:
        return None
    return git("merge-base", "--is-ancestor", fix, rel).returncode == 0


def promotions():
    seen = {}
    for p in sorted(RELEASES.glob("*/promoted.json")):
        d = json.loads(p.read_text())
        seen[p.parent.name] = (d["promoted_at"], d.get("from"), "promoted.json")
    line = re.compile(
        r"^(\S+) \[promote\] promoted\.json: \S+/releases/([0-9a-f]+)/promoted\.json"
        r" \(mode=live from=([0-9a-f]+)\)"
    )
    for log in sorted(LOGS.glob("promote-*.log")):
        for text in log.read_text(errors="replace").splitlines():
            m = line.match(text)
            if m and m.group(2) not in seen:
                seen[m.group(2)] = (m.group(1), m.group(3), "promote log")
    return seen


def audit():
    fix = git("log", "--format=%H", "-F", "--grep=" + FIX_SUBJECT, "main").stdout.split()
    if len(fix) != 1:
        sys.exit("fix commit not found uniquely: %d" % len(fix))
    fix = fix[0]
    proms = promotions()
    firsts = sorted(
        (at, rel, src) for rel, (at, frm, src) in proms.items()
        if contains_fix(fix, rel) and frm and contains_fix(fix, frm) is False
    )
    if not firsts:
        sys.exit("no promotion containing the fix")
    since = firsts[0][0]
    con = connect()
    progress = con.execute(
        "select task_id, json_extract(json,'$.run_id') from events"
        " where json_extract(json,'$.type')='worker_progress' and ts>=? and"
        " json_extract(json,'$.msg') like ?",
        (since, PROGRESS_PREFIX + "%"),
    ).fetchall()
    decisions = [
        (t, r) for k, t, r, o in con.execute(
            "select key, task_id, json_extract(json,'$.raised_by.run_id'),"
            " json_extract(json,'$.raised_by.origin') from decisions where created_at>=?",
            (since,),
        )
        if REJECTED_KEY.match(k) and o == "worker"
    ]
    prog_runs = {r for _, r in progress}
    dec_runs = {r for _, r in decisions}
    matched = sorted(prog_runs & dec_runs)
    unmatched = [
        {"side": "progress_line", "task_id": t, "run_id": r}
        for t, r in progress if r not in dec_runs
    ] + [
        {"side": "rejected_decision", "task_id": t, "run_id": r}
        for t, r in decisions if r not in prog_runs
    ]
    return {
        "since": since,
        "progress_lines": len(progress),
        "rejected_decisions": len(decisions),
        "matched": len(matched),
        "task_ids": sorted({t for t, _ in progress} | {t for t, _ in decisions}),
        "unmatched": unmatched,
    }


def main():
    if sys.argv[1:] == ["fixture"]:
        out = fixture()
    elif sys.argv[1:] == ["audit"]:
        out = audit()
    else:
        sys.exit("usage: extract.py fixture|audit")
    json.dump(out, sys.stdout, ensure_ascii=False, indent=1)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
