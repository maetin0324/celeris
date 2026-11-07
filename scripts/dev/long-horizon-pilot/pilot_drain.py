#!/usr/bin/env python3
"""pilot の試行の終わりの待機（drain）と共有の期限（ADR 2026-11-06-cos-run-launch-accounting-and-drain D3、
ADR 2026-11-07-pilot-drain-shared-deadline）。

driver が daemon を止める前に、案件の CoS chat run が全部「終わって usage が書かれた」か「起動前に確かに終わった」
状態になるのを DB の読み取りで待つ。時計・sleep・読み取りは注入できる（試験は注入した時計で決定的に走らせる）。
"""
import json
import time

# backend の閉じた列挙（crates/task-core の ChatPrelaunchReason）。ここに無い reason は「境界が不確か」で、
# 起動前に終わったとは数えない（fail closed）。
PRELAUNCH_REASONS = frozenset({
    "route_unavailable", "skills_unavailable", "wall_expired_before_launch", "slot_wait_expired",
    "daemon_restart_before_launch", "stopped_before_launch"})

LAUNCHED, NOT_LAUNCHED, UNRECORDED = "launched", "not_launched", "unrecorded"


def chat_run_columns(conn):
    return {r[1] for r in conn.execute("PRAGMA table_info(chat_runs)")}


def launch_state(row):
    """chat run の行から起動の状態を決める。起動したと言えない・起動前と言い切れない行は unrecorded（起動済みとして扱う）。

    - launched: launched_at がある。
    - not_launched: 終わっていて、起動境界を追跡した行（launch_tracked=1）で launched_at が無く、prelaunch_reason が列挙内。
    - unrecorded: それ以外（旧記録・境界不明・列挙外の reason・まだ終わっていない行）。
    """
    if row.get("launched_at"):
        return LAUNCHED
    if (row.get("finished_at") and row.get("launch_tracked") == 1
            and (row.get("prelaunch_reason") or "") in PRELAUNCH_REASONS):
        return NOT_LAUNCHED
    return UNRECORDED


def settled(row):
    """drain が待つ条件: 終わって usage がある、または起動前に終わった。"""
    if launch_state(row) == NOT_LAUNCHED:
        return True
    return bool(row.get("finished_at")) and bool(row.get("usage_json"))


def read_chat_runs(conn, pid):
    """案件の CoS thread の chat run を全件読む（started_at が無い queued の行も含む）。列の無い旧い DB は欄を None にする。"""
    cols = chat_run_columns(conn)
    want = ["run_id", "thread_id", "state", "reason", "started_at", "finished_at", "usage_json",
            "launched_at", "prelaunch_reason", "launch_tracked"]
    sel = ", ".join(f"r.{c}" if c in cols else f"NULL AS {c}" for c in want)
    cur = conn.execute(f"SELECT {sel} FROM chat_runs r JOIN chat_threads t ON t.id = r.thread_id "
                       "WHERE t.project_id = ? ORDER BY r.started_at, r.rowid", (pid,))
    names = [d[0] for d in cur.description]
    return [dict(zip(names, row)) for row in cur.fetchall()]


def unsettled(rows):
    return [r for r in rows if not settled(r)]


def describe(row):
    return {"run_id": row["run_id"], "state": row.get("state"), "launch": launch_state(row),
            "finished": bool(row.get("finished_at")), "usage": bool(row.get("usage_json")),
            "reason": (row.get("reason") or "")[:80] or None}


def drain(read_pending, now=time.time, sleep=time.sleep, deadline=None, settle_secs=2.0, poll_secs=1.0):
    """read_pending() が空を返すまで待つ。空になっても settle_secs の間は読み直し、その間に新しい run が現れたら待ち直す
    （最後の wake・off の後に新しい CoS run が生まれる競合）。deadline（epoch 秒）に達しても空でなければ時間切れ。

    戻り値: {"waited_secs", "timed_out": [describe の dict], "rounds"}。usage を 0 にして足すことはしない。
    """
    start = now()
    quiet_since = None
    rounds = 0
    while True:
        pending = read_pending()
        rounds += 1
        t = now()
        if pending:
            quiet_since = None
            if deadline is not None and t >= deadline:
                return {"waited_secs": round(t - start, 1), "timed_out": [describe(r) for r in pending], "rounds": rounds}
        else:
            if quiet_since is None:
                quiet_since = t
            if t - quiet_since >= settle_secs or (deadline is not None and t >= deadline):
                return {"waited_secs": round(t - start, 1), "timed_out": [], "rounds": rounds}
        step = poll_secs if deadline is None else max(0.0, min(poll_secs, deadline - t))
        sleep(step)


def compute_deadline(trial_start, max_wall_secs, approval_start=None, approval_wall_secs=None, expires_at=None):
    """共有の期限（epoch 秒）。試行の開始＋試行の上限、承認の開始＋承認の wall の総量、承認の期限の最小。

    承認の max_wall_secs は承認全体の総量（judge の wall_cap は全試行の wall の和と比べる）。試行ごとにリセットしない。"""
    cands = [trial_start + max_wall_secs]
    if approval_start is not None and approval_wall_secs is not None:
        cands.append(approval_start + approval_wall_secs)
    if expires_at is not None:
        cands.append(expires_at)
    return min(cands)


def dumps(v):
    return json.dumps(v, ensure_ascii=False)
