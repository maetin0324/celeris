#!/usr/bin/env python3
"""アーキv4 の子 A・C・D・E・F・G の歴史 replay（時点別の入力の切り出しと、未来の漏れの検査）。

使い方（repo の root で。stdlib だけ）:
  python3 -I <dir>/replay.py export --db /local/celeris/data/db/celeris.sqlite3   # operator だけ。fixture を作り直す
  python3 -I <dir>/replay.py build --out "$TMPDIR/replay"                          # 12 の slice を作る
  python3 -I <dir>/replay.py check "$TMPDIR/replay"                                # 漏れ・根拠の検査（違反で exit 1）
  python3 -I <dir>/replay.py verify                                                # 再生の一致 + 否定対照 3 件（全部期待どおりで exit 0）
  python3 -I <dir>/replay.py manifest "$TMPDIR/replay"                             # manifest.sha256 の行を出す

入力は 2 つだけ: (1) 本番 DB の events を export した fixture `events.jsonl`（ts・task・type と白 list の欄。
worker_progress・routing・quota・人の名前の入る note は落とす）と親の作成時の目的 `goal.json`、
(2) git の main（基準 BASE）の first-parent の commit の tree（arch-v4 の進捗 file だけ）。
時点 T の slice には、ts <= T の event と、committer date <= T の最新の first-parent commit の tree だけを入れる。
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import sqlite3
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
BASE = "7f69057907ad649829ec5a08b5ac7c892b9fe055"  # main 2026-11-20（依頼の基準）
WINDOW_START = "2026-11-17T14:00:00Z"
PATH_RE = re.compile(r"^agent-docs/progress/[^/]*arch-v4[^/]*(/|$)")
TASKS = {
    "P": "01M7MZT8KX547464YYS76QNP4G",
    "A": "01M7MZTKAXM8GJGK8BPE13CQRM",
    "B": "01M7MZTKAZQY72EX2R4RQ1WVWF",
    "C": "01M7MZTKB29VF5TX7FCQ15XEMW",
    "D": "01M7MZTKB4RYM80S6CT3RST2N9",
    "E": "01M7MZTKB6GQ241F6JJYSPJNS1",
    "F": "01M7MZTKB8A4NJQEEA35Q4G5JV",
    "G": "01M7MZTKBKXKEVBWS87PS5J9NK",
    "H": "01M7MZTKBPZPQE4XN451F8X84F",
}
CASES = ["A", "C", "D", "E", "F", "G"]
KEEP = {
    "created": lambda d: {"title": d["task"]["title"][:200]},
    "transitioned": lambda d: {"from": d.get("from"), "to": d.get("to"), "reason": str(d.get("reason", ""))[:200]},
    "execution_planned": lambda d: {
        "version": d.get("version"),
        "units": [u.get("key") for u in (d.get("plan") or {}).get("units", [])],
        "rationale": str((d.get("plan") or {}).get("rationale", ""))[:400],
    },
    "work_unit_transitioned": lambda d: {"key": d.get("key"), "from": d.get("from"), "to": d.get("to")},
    "work_unit_checks_failed": lambda d: {"key": d.get("key")},
    "phase_integrated": lambda d: {"phase": d.get("phase")},
    "worker_finished": lambda d: {
        "role": d.get("role"),
        "outcome": str(d.get("outcome", ""))[:200],
        "cost_usd": (d.get("usage") or {}).get("cost_usd"),
        "wall_ms": (d.get("metrics") or {}).get("wall_ms"),
    },
    "review_verdict": lambda d: {"criterion_idx": d.get("criterion_idx"), "pass": d.get("pass"), "reason": str(d.get("reason", ""))[:400]},
    "reviewer_passed": lambda d: {"criteria": d.get("criteria")},
    "design_reviewed": lambda d: {"pass": d.get("pass"), "finding_kinds": sorted({f.get("kind") for f in d.get("findings", [])})},
    "answered": lambda d: {"question": str(d.get("question", ""))[:300]},
    "decision_requested": lambda d: {"key": (d.get("decision") or {}).get("key"), "question": str((d.get("decision") or {}).get("question", ""))[:300]},
    "decision_answered": lambda d: {"option": d.get("option")},
    "plan_approval_requested": lambda d: {"reasons": d.get("reasons")},
    "auto_replan_requested": lambda d: {"key": d.get("key")},
    "integration_merged": lambda d: {},
    "artifact_produced": lambda d: {"name": (d.get("artifact") or {}).get("name")},
    "child_task_created": lambda d: {"unit_key": d.get("unit_key"), "child": d.get("child_task_id")},
}
SECRET_RE = re.compile(r"(?i)(authorization:|bearer\s+[a-z0-9._-]{16,}|password\s*[=:]|cookie\s*[=:]|https://hooks\.|sk-[a-z0-9]{20,}|ghp_[a-z0-9]{20,})")
ANSWER_KEY_MARK = "celeris.replay-answer-key/1"


def git(*args: str) -> str:
    return subprocess.run(["git", *args], check=True, capture_output=True, text=True).stdout


def load_events() -> list[dict]:
    with open(HERE / "events.jsonl", encoding="utf-8") as f:
        return [json.loads(line) for line in f if line.strip()]


def dumps(obj) -> str:
    return json.dumps(obj, ensure_ascii=False, sort_keys=True)


# ---------------------------------------------------------------- export（operator だけ）
def cmd_export(db: str) -> int:
    conn = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    ids = dict(TASKS)
    for (tid,) in conn.execute(
        "select id from tasks where parent_id in (%s) order by id" % ",".join("?" * len(TASKS)), list(TASKS.values())
    ):
        if tid not in ids.values():
            ids[f"sub:{tid}"] = tid
    rows = []
    for key, tid in ids.items():
        for seq, ts, js in conn.execute("select seq, ts, json from events where task_id=? order by seq", (tid,)):
            d = json.loads(js)
            f = KEEP.get(d.get("type"))
            if f is None:
                continue
            row = {"ts": ts, "task": key, "seq": seq, "type": d["type"], **f(d)}
            if SECRET_RE.search(dumps(row)):
                print(f"export: secret-like text in {key} seq {seq}; refusing", file=sys.stderr)
                return 1
            rows.append(row)
    rows.sort(key=lambda r: (r["ts"], r["task"], r["seq"]))
    with open(HERE / "events.jsonl", "w", encoding="utf-8") as f:
        for r in rows:
            f.write(dumps(r) + "\n")
    created = json.loads(
        conn.execute("select json from events where task_id=? and seq=0", (TASKS["P"],)).fetchone()[0]
    )["task"]
    goal = {"task": TASKS["P"], "created_at": created["created_at"], "title": created["title"], "objective": created["objective"]}
    with open(HERE / "goal.json", "w", encoding="utf-8") as f:
        f.write(dumps(goal) + "\n")
    print(f"export: {len(rows)} events from {len(ids)} tasks")
    return 0


# ---------------------------------------------------------------- 時点の規則
def checkpoints(events: list[dict]) -> list[dict]:
    """各 case に 2 時点: mid = 2 つ目の phase_integrated（無ければ最初の running→reviewing）、done = integration_merged。"""
    out = []
    for c in CASES:
        ev = [e for e in events if e["task"] == c]
        phases = [e["ts"] for e in ev if e["type"] == "phase_integrated"]
        reviewing = [e["ts"] for e in ev if e["type"] == "transitioned" and e.get("to") == "reviewing"]
        mid = phases[1] if len(phases) >= 2 else reviewing[0]
        done = [e["ts"] for e in ev if e["type"] == "integration_merged"][0]
        out.append({"id": f"{c}-mid", "case": c, "cutoff": mid, "rule": "2nd phase_integrated or 1st reviewing"})
        out.append({"id": f"{c}-done", "case": c, "cutoff": done, "rule": "integration_merged"})
    return out


def snapshot_commit(cutoff: str) -> tuple[str, str]:
    sha = git("rev-list", "-1", "--first-parent", f"--before={cutoff[:19]}Z", BASE).strip()
    date = norm(git("show", "-s", "--format=%cI", sha).strip())
    return sha, date


def snapshot_paths(sha: str) -> list[str]:
    names = git("ls-tree", "-r", "--name-only", sha, "--", "agent-docs/progress").splitlines()
    return sorted(n for n in names if PATH_RE.match(n))


def cost_so_far(events: list[dict]) -> float:
    return round(sum(e.get("cost_usd") or 0 for e in events if e["type"] == "worker_finished"), 4)


def write_slice(root: Path, cp: dict, events: list[dict], goal: dict, snap_sha: str | None = None, extra_events=(), extra_files=()):
    d = root / cp["id"]
    d.mkdir(parents=True)
    cut = cp["cutoff"]
    ev = [e for e in events if e["ts"] <= cut] + list(extra_events)
    sha, cdate = snapshot_commit(cut)
    if snap_sha is not None:
        sha, cdate = snap_sha, norm(git("show", "-s", "--format=%cI", snap_sha).strip())
    (d / "goal.md").write_text(f"# {goal['title']}\n\ncreated_at: {goal['created_at']}\n\n{goal['objective']}\n", encoding="utf-8")
    with open(d / "events.jsonl", "w", encoding="utf-8") as f:
        for e in ev:
            f.write(dumps(e) + "\n")
    paths = snapshot_paths(sha)
    for p in paths:
        out = d / "snapshot" / p
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_bytes(subprocess.run(["git", "show", f"{sha}:{p}"], check=True, capture_output=True).stdout)
    for name, text in extra_files:
        (d / name).write_text(text, encoding="utf-8")
    meta = {
        "id": cp["id"],
        "case": cp["case"],
        "cutoff": cut,
        "rule": cp["rule"],
        "snapshot_commit": sha,
        "snapshot_committer_date": cdate,
        "snapshot_files": len(paths),
        "events": len(ev),
        "children_done": sorted({e["task"] for e in ev if e["type"] == "integration_merged" and len(e["task"]) == 1}),
        "cost_so_far_usd": cost_so_far(ev),
    }
    (d / "slice.json").write_text(dumps(meta) + "\n", encoding="utf-8")
    (d / "question.md").write_text(QUESTION.format(cutoff=cut), encoding="utf-8")


QUESTION = """# 評価の問い（時点 {cutoff}）

この dir の goal.md・events.jsonl・snapshot/ だけを根拠に答える（この時点より後の知識を使わない）。
1. 親の目的（goal.md の共通受入条件 (1)〜(6)）に対する現状: on_track / at_risk / off_track / unknown。
2. 根拠: この dir の file と行・event の (task, seq) を挙げる。根拠の無い主張は unsupported として数える。
3. いま持っている仮説（例: 狭い pilot を積めば module の循環・SCC が減る）を保つか改めるか。改めるなら代わりの仮説。
4. 次の手: continue / discriminating_experiment（何を測るか）/ pivot（どの戦略へ）/ ask_human（何を聞くか）。
5. ここまでの費用（slice.json の cost_so_far_usd）に対し、次の手の費用の見積もり。
"""


def cmd_build(out: str, leak: str | None = None) -> int:
    events = load_events()
    goal = json.loads((HERE / "goal.json").read_text(encoding="utf-8"))
    root = Path(out)
    if root.exists():
        shutil.rmtree(root)
    root.mkdir(parents=True)
    cps = checkpoints(events)
    if leak is None:
        for cp in cps:
            write_slice(root, cp, events, goal)
        return 0
    # 否定対照: わざと未来を混ぜた slice を 1 つ作る（check が落ちなければならない）
    by = {cp["id"]: cp for cp in cps}
    if leak == "N1-future-snapshot":
        write_slice(root, by["A-done"], events, goal, snap_sha=BASE)
    elif leak == "N2-future-event":
        g_done = [e for e in events if e["task"] == "G" and e["type"] == "integration_merged"]
        write_slice(root, by["D-done"], events, goal, extra_events=g_done)
    elif leak == "N3-answer-key":
        key = (HERE / "answer_key.json").read_text(encoding="utf-8")
        write_slice(root, by["C-done"], events, goal, extra_files=[("notes.md", key)])
    else:
        print(f"unknown leak control {leak}", file=sys.stderr)
        return 2
    return 0


# ---------------------------------------------------------------- 検査
def first_seen_map(sentinels: list[str], events: list[dict]) -> dict[str, str]:
    seen: dict[str, str] = {}
    goal = json.loads((HERE / "goal.json").read_text(encoding="utf-8"))
    for t in sentinels:
        if t in goal["objective"] or t in goal["title"]:
            seen[t] = norm(goal["created_at"])
    for e in events:
        s = dumps(e)
        for t in sentinels:
            if t not in seen and t in s:
                seen[t] = norm(e["ts"])
    commits = git("rev-list", "--first-parent", "--reverse", f"--since={WINDOW_START}", "--format=%H %cI", BASE).splitlines()
    commits = [c.split() for c in commits if not c.startswith("commit ")]
    for sha, cdate in commits:
        cdate = norm(cdate)
        paths = snapshot_paths(sha)
        if not paths:
            continue
        for t in sentinels:
            if t in seen and seen[t] <= cdate:
                continue
            r = subprocess.run(["git", "grep", "-q", "-F", "-e", t, sha, "--", *paths], capture_output=True)
            if r.returncode == 0:
                if t not in seen or cdate < seen[t]:
                    seen[t] = cdate
    return seen


def norm(ts: str) -> str:
    """ISO 時刻を文字列で比べられる形（UTC・小数 9 桁・Z）にそろえる。events の ts はこの形。"""
    if ts.startswith("9999"):
        return ts
    m = re.match(r"^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)(\.\d+)?(Z|[+-]\d\d:\d\d)$", ts)
    if not m:
        raise ValueError(f"bad timestamp {ts}")
    base, frac, tz = m.groups()
    dt = datetime.fromisoformat(base + ("+00:00" if tz == "Z" else tz)).astimezone(timezone.utc)
    return dt.strftime("%Y-%m-%dT%H:%M:%S") + (frac or ".").ljust(10, "0")[:10] + "Z"


def check_dir(root: Path, verbose: bool = True) -> list[str]:
    events = load_events()
    sent = json.loads((HERE / "sentinels.json").read_text(encoding="utf-8"))
    key = json.loads((HERE / "answer_key.json").read_text(encoding="utf-8"))
    bad: list[str] = []
    for d in sorted(p for p in root.iterdir() if p.is_dir()):
        meta = json.loads((d / "slice.json").read_text(encoding="utf-8"))
        cut = meta["cutoff"]
        # (1) event の時刻
        for line in (d / "events.jsonl").read_text(encoding="utf-8").splitlines():
            e = json.loads(line)
            if norm(e["ts"]) > norm(cut):
                bad.append(f"{d.name}: event {e['task']}#{e['seq']} ts {e['ts']} > cutoff {cut}")
        # (2) snapshot の commit の時刻
        if norm(meta["snapshot_committer_date"]) > norm(cut):
            bad.append(f"{d.name}: snapshot commit date {meta['snapshot_committer_date']} > cutoff {cut}")
        # (3) 未来にだけ現れる語（sentinel）と answer key の印
        text = "\n".join(p.read_text(encoding="utf-8", errors="replace") for p in sorted(d.rglob("*")) if p.is_file())
        if ANSWER_KEY_MARK in text:
            bad.append(f"{d.name}: answer key の印 {ANSWER_KEY_MARK} が入力にある")
        for s in sent["sentinels"]:
            if s["first_seen"] > norm(cut) and s["text"] in text:
                bad.append(f"{d.name}: sentinel {s['text']!r}（first_seen {s['first_seen']}）が cutoff {cut} の入力にある")
        # (4) answer key の根拠がこの slice の中にあること（未来の知識で正解を作っていない）
        for ev in key["checkpoints"].get(d.name, {}).get("evidence", []):
            f = d / ev["file"]
            if not f.is_file() or ev["contains"] not in f.read_text(encoding="utf-8", errors="replace"):
                bad.append(f"{d.name}: answer key の根拠 {ev['file']} に {ev['contains']!r} が無い")
    if verbose:
        for b in bad:
            print(b)
    return bad


def manifest_lines(root: Path) -> list[str]:
    lines = []
    for p in sorted(x for x in root.rglob("*") if x.is_file()):
        lines.append(f"{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.relative_to(root).as_posix()}")
    return lines


def cmd_verify() -> int:
    ok = True
    tmp = Path(tempfile.mkdtemp(prefix="replay-", dir=os.environ.get("TMPDIR")))
    try:
        # sentinel の first_seen を fixture と git から測り直す
        sent = json.loads((HERE / "sentinels.json").read_text(encoding="utf-8"))
        fs = first_seen_map([s["text"] for s in sent["sentinels"]], load_events())
        for s in sent["sentinels"]:
            got = norm(fs.get(s["text"], "9999"))
            if got != s["first_seen"]:
                print(f"FAIL sentinel first_seen drift {s['text']!r}: recorded {s['first_seen']} measured {got}")
                ok = False
        cmd_build(str(tmp / "slices"))
        got = manifest_lines(tmp / "slices")
        want = (HERE / "manifest.sha256").read_text(encoding="utf-8").splitlines()
        if got != want:
            print(f"FAIL manifest: {len(set(got) ^ set(want))} lines differ")
            ok = False
        else:
            print(f"ok   replay: {len(got)} files reproduced byte-for-byte")
        bad = check_dir(tmp / "slices", verbose=True)
        if bad:
            print(f"FAIL check on clean slices: {len(bad)} violations")
            ok = False
        else:
            print("ok   check: 12 slices, 0 leaks, answer-key evidence grounded")
        for n in ["N1-future-snapshot", "N2-future-event", "N3-answer-key"]:
            cmd_build(str(tmp / n), leak=n)
            bad = check_dir(tmp / n, verbose=False)
            if bad:
                print(f"ok   negative control {n}: detected ({len(bad)} violations, e.g. {bad[0][:120]})")
            else:
                print(f"FAIL negative control {n}: leak NOT detected")
                ok = False
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    print("verify:", "ok" if ok else "FAILED")
    return 0 if ok else 1


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print(__doc__)
        return 2
    cmd = argv[1]
    if cmd == "export":
        return cmd_export(argv[argv.index("--db") + 1])
    if cmd == "build":
        leak = argv[argv.index("--leak") + 1] if "--leak" in argv else None
        return cmd_build(argv[argv.index("--out") + 1], leak)
    if cmd == "check":
        return 1 if check_dir(Path(argv[2])) else 0
    if cmd == "manifest":
        print("\n".join(manifest_lines(Path(argv[2]))))
        return 0
    if cmd == "first-seen":
        sent = json.loads((HERE / "sentinels.json").read_text(encoding="utf-8"))
        fs = first_seen_map([s["text"] for s in sent["sentinels"]], load_events())
        for s in sent["sentinels"]:
            print(f"{norm(fs.get(s['text'], '9999'))}\t{s['text']}")
        return 0
    if cmd == "verify":
        return cmd_verify()
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
