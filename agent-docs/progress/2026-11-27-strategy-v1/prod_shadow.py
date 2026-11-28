"""戦略層 v1 親 count-rule: 本番の R1 shadow の記録を、事前に固定した規則（prod-shadow-rule.md）で数える。

使い方（repo の root で）:
    python3 -I agent-docs/progress/2026-11-27-strategy-v1/prod_shadow.py export --out <snapshot>
    python3 -I agent-docs/progress/2026-11-27-strategy-v1/prod_shadow.py run --snapshot <snapshot> --out <result>
    python3 -I agent-docs/progress/2026-11-27-strategy-v1/prod_shadow.py verify

export は本番 DB を URI `file:...?mode=ro` で開き（書き込まない。path は 2026-11-27-strategy-v1-9/audit_query.py と同じ）、
id・時刻・status・規則の id・effect_state・件数だけを snapshot に写す（本文・問い・note・payload は写さない）。
run は snapshot だけから判定する（DB を読まない）。verify は prod-shadow/ の `<名>-snapshot.json` ごとに
`<名>.json` を数え直して比べ、snapshot の key と文字列の長さを確かめる。違えば exit 1。
"""

import argparse
import json
import re
import sqlite3
import sys
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "prod-shadow"
# audit_query.py（2026-11-27-strategy-v1-9）と同じ本番 DB。
DB_PATH = "/local/celeris/data/db/celeris.sqlite3"

K = 3  # crates/task-core/src/parent_checkpoint.rs の STALL_K と同じ
R1_RULE = "pc/r1-stall"
GUARD_PREFIX = "pc/guard/"
NO_PROGRESS_RULE = "pc/no-progress"
CONTROLLER_LABEL = "project-controller"  # task_core::project_proposal::CONTROLLER_LABEL
COS_INPUT_PREFIX = "pc-input:"  # task_core::project_wake::COS_INPUT_KEY_PREFIX
NOTICE_PREFIX = "project_controller:"  # controller の notice の source_key（feed_sources）
R1_KEY = re.compile(r"r1_stall:([^:]+):[0-9]+")
STRATEGY_NAME = re.compile(r"strategy/([^/]+)/v(0|[1-9][0-9]*)\.json")
TERMINAL = frozenset({"done", "failed", "cancelled"})  # Status::is_terminal
STRING_MAX = 200
SNAPSHOT_SCHEMA = "celeris.prod-shadow-snapshot/1"
RESULT_SCHEMA = "celeris.prod-shadow-result/1"
VERDICTS = ("unsafe", "inconclusive", "falsified", "useful", "no_signal")

ID_KEYS = frozenset({
    "schema_id", "project_id", "root_id", "task_id", "decision_id", "event_id", "outcome_id",
    "message_id", "thread_id", "run_id", "item_id", "domain_id", "generation_id",
})
TIME_KEYS = frozenset({"exported_at", "created_at", "updated_at"})
STATUS_KEYS = frozenset({"status", "mode", "phase", "state", "controller_mode", "controller_phase"})
RULE_KEYS = frozenset({"rule_id"})
EFFECT_KEYS = frozenset({"effect_state"})
COUNT_KEYS = frozenset({
    "k", "n_projects", "n_decisions", "n_r1_waiting", "n_strategy_creates",
})
LIST_KEYS = frozenset({
    "projects", "strategy_roots", "children", "decisions", "guard_escalates", "outcome_reached",
    "controller_tasks", "controller_cos_inputs", "controller_inbox_items", "script_domains",
})
DICT_KEYS = frozenset({"counts"})
ALLOWED_KEYS = (ID_KEYS | TIME_KEYS | STATUS_KEYS | RULE_KEYS | EFFECT_KEYS | COUNT_KEYS
                | LIST_KEYS | DICT_KEYS)


def dumps(obj):
    return json.dumps(obj, indent=2, sort_keys=True) + "\n"


def parse_ts(text):
    """RFC 3339（Z か ±hh:mm、小数は 9 桁まで）を aware な datetime にする。小数は 6 桁に切る。"""
    m = re.fullmatch(r"(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)(?:\.(\d+))?(Z|[+-]\d\d:\d\d)", text or "")
    if m is None:
        raise ValueError(f"time not RFC 3339: {text!r}")
    frac = (m.group(2) or "")[:6].ljust(6, "0")
    tz = "+00:00" if m.group(3) == "Z" else m.group(3)
    return datetime.fromisoformat(f"{m.group(1)}.{frac}{tz}").astimezone(timezone.utc)


def strategy_artifact(name):
    """task_core::strategy_ref::strategy_artifact と同じ: `strategy/<id>/v<版>.json`（版は u32、先頭 0 なし）。"""
    m = STRATEGY_NAME.fullmatch(name or "")
    return m is not None and int(m.group(2)) < 2**32


def short(value):
    """snapshot に写す文字列（id・時刻・status）。200 字を超えるものは写さない（verify が落とす値を作らない）。"""
    if value is None:
        return None
    text = str(value)
    if len(text) > STRING_MAX:
        raise ValueError(f"string over {STRING_MAX} chars in a field meant for ids/times/status")
    return text


# ---------------------------------------------------------------- export（本番 DB を読み取り専用で）

def is_root(task_json):
    """task_core::org::is_root_task の json の側（parent_id・project_id は SQL で絞る）。"""
    body = json.loads(task_json)
    tree = body.get("tree") or {}
    return tree.get("parent_unit") is None and body.get("conversation") is None


def effect_of(effect_json):
    try:
        effect = json.loads(effect_json)
    except (TypeError, ValueError):
        return {}
    return effect if isinstance(effect, dict) else {}


def export(conn, exported_at):
    projects = []
    creates = conn.execute(
        "SELECT target_id, json_extract(payload_json, '$.strategy.name') FROM cos_operations "
        "WHERE action = 'task.create' AND state = 'applied' "
        "AND json_extract(payload_json, '$.strategy.name') IS NOT NULL ORDER BY created_at, id"
    ).fetchall()
    first_ref = {}
    for target, name in creates:
        first_ref.setdefault(target, name)  # strategy_task_creates(store, Some(root)) の最初の行
    states = {pid: (mode, phase) for pid, mode, phase in conn.execute(
        "SELECT project_id, mode, phase FROM project_controller_state")}
    for (pid,) in conn.execute("SELECT id FROM projects ORDER BY created_at, id"):
        roots = []
        for tid, status, created, raw in conn.execute(
            "SELECT id, status, created_at, json FROM tasks WHERE project_id = ? AND parent_id IS NULL "
            "ORDER BY created_at, id", (pid,)):
            if not is_root(raw) or not strategy_artifact(first_ref.get(tid)):
                continue
            children = [
                {"task_id": short(cid), "status": short(cst), "updated_at": short(cup)}
                for cid, cst, cup in conn.execute(
                    "SELECT id, status, updated_at FROM tasks WHERE parent_id = ? ORDER BY created_at, id",
                    (tid,))
            ]
            roots.append({"root_id": short(tid), "status": short(status), "created_at": short(created),
                          "children": children})
        mode, phase = states.get(pid, (None, None))
        projects.append({"project_id": short(pid), "controller_mode": short(mode),
                         "controller_phase": short(phase), "strategy_roots": roots})

    decisions, guards, waiting, total = [], [], 0, 0
    for did, pid, created, rule, effect_state, mode, effect_json in conn.execute(
        "SELECT id, project_id, created_at, validation, effect_state, mode, effect_json "
        "FROM project_controller_decisions ORDER BY created_at, id"):
        total += 1
        effect = effect_of(effect_json)
        escalate = effect.get("kind") == "open_escalation"
        if rule == R1_RULE:
            m = R1_KEY.fullmatch(str(effect.get("question_key") or "")) if escalate else None
            decisions.append({"decision_id": short(did), "project_id": short(pid), "created_at": short(created),
                              "rule_id": R1_RULE, "effect_state": short(effect_state), "mode": short(mode),
                              "root_id": short(m.group(1)) if m else None})
        elif rule.startswith(R1_RULE + "/"):
            waiting += 1
        elif escalate and (rule.startswith(GUARD_PREFIX) or rule == NO_PROGRESS_RULE):
            # guard の rule_id の ':' より後ろは Outcome の id の列（200 字を超えうる）なので写さない。
            guards.append({"decision_id": short(did), "project_id": short(pid), "created_at": short(created),
                           "rule_id": short(rule.split(":", 1)[0]), "effect_state": short(effect_state)})

    reached = [
        {"event_id": short(eid), "project_id": short(pid), "outcome_id": short(oid), "status": "reached",
         "created_at": short(created)}
        for eid, pid, oid, created in conn.execute(
            "SELECT id, project_id, outcome_id, created_at FROM project_roadmap_outcome_events "
            "WHERE kind = 'status' AND status = 'reached' ORDER BY created_at, id")
    ]
    controller_tasks = []
    for tid, pid, status, created, labels in conn.execute(
        "SELECT id, project_id, status, created_at, labels_json FROM tasks "
        "WHERE labels_json LIKE ? ORDER BY created_at, id", (f"%{CONTROLLER_LABEL}%",)):
        if CONTROLLER_LABEL in (json.loads(labels or "[]") or []):
            controller_tasks.append({"task_id": short(tid), "project_id": short(pid), "status": short(status),
                                     "created_at": short(created)})
    cos_inputs = [
        {"message_id": short(mid), "thread_id": short(th), "run_id": short(run), "state": short(st),
         "created_at": short(created)}
        for mid, th, run, st, created in conn.execute(
            "SELECT id, thread_id, run_id, state, created_at FROM chat_messages "
            "WHERE substr(client_message_id, 1, ?) = ? ORDER BY created_at, id",
            (len(COS_INPUT_PREFIX), COS_INPUT_PREFIX))
    ]
    inbox = [
        {"item_id": short(iid), "state": short(st), "created_at": short(created)}
        for iid, st, created in conn.execute(
            "SELECT i.id, i.state, i.created_at FROM cos_inbox_items i WHERE i.source_kind = 'notice' "
            "AND i.source_key IN (SELECT notice_id FROM feed_sources WHERE substr(source_key, 1, ?) = ?) "
            "ORDER BY i.created_at, i.id", (len(NOTICE_PREFIX), NOTICE_PREFIX))
    ]
    domains = [
        {"domain_id": short(dom), "mode": short(mode), "generation_id": short(gen), "updated_at": short(up)}
        for dom, mode, gen, up in conn.execute(
            "SELECT domain, mode, generation_id, updated_at FROM script_domain_modes ORDER BY domain")
    ]
    return {
        "schema_id": SNAPSHOT_SCHEMA,
        "exported_at": exported_at,
        "k": K,
        "projects": projects,
        "decisions": decisions,
        "guard_escalates": guards,
        "outcome_reached": reached,
        "controller_tasks": controller_tasks,
        "controller_cos_inputs": cos_inputs,
        "controller_inbox_items": inbox,
        "script_domains": domains,
        "counts": {"n_projects": len(projects), "n_decisions": total, "n_r1_waiting": waiting,
                   "n_strategy_creates": len(creates)},
    }


# ---------------------------------------------------------------- run（snapshot だけから判定する）

def judge(snap):
    end = parse_ts(snap["exported_at"])
    projects = snap["projects"]
    root_created = {}
    shadow_projects, roots_in_shadow, ripe_in_shadow, project_rows = [], 0, 0, []
    for p in projects:
        roots = p["strategy_roots"]
        ripe = sum(1 for r in roots if sum(c["status"] in TERMINAL for c in r["children"]) >= K)
        for r in roots:
            root_created[(p["project_id"], r["root_id"])] = parse_ts(r["created_at"])
        if p["controller_mode"] == "shadow":
            shadow_projects.append(p["project_id"])
            roots_in_shadow += len(roots)
            ripe_in_shadow += ripe
        project_rows.append({"project_id": p["project_id"], "controller_mode": p["controller_mode"],
                             "controller_phase": p["controller_phase"], "strategy_roots": len(roots),
                             "roots_with_k_terminal_children": ripe})

    reached = [(e["project_id"], parse_ts(e["created_at"])) for e in snap["outcome_reached"]]
    guards = [(g["project_id"], parse_ts(g["created_at"])) for g in snap["guard_escalates"]]
    fires = []
    for d in snap["decisions"]:
        if d["rule_id"] != R1_RULE:
            continue
        at = parse_ts(d["created_at"])
        pid = d["project_id"]
        # 誤検知: 発火より後（同時刻は含まない）・export の時刻まで（含む）に同じ案件の Outcome が reached。
        false_positive = any(rp == pid and at < rt <= end for rp, rt in reached)
        # 先行: 同じ案件で root の起票以後の guard の escalate が、発火の時刻まで（同時刻を含む）に 1 件も無い。
        since = root_created.get((pid, d["root_id"]))
        lead = not any(gp == pid and gt <= at and (since is None or gt >= since) for gp, gt in guards)
        fires.append({"decision_id": d["decision_id"], "project_id": pid, "root_id": d["root_id"],
                      "created_at": d["created_at"], "effect_state": d["effect_state"],
                      "false_positive": false_positive, "lead": lead})

    safety = {
        "r1_not_suppressed": [d["decision_id"] for d in snap["decisions"]
                              if d["rule_id"] == R1_RULE and d["effect_state"] != "suppressed"],
        "controller_tasks": [t["task_id"] for t in snap["controller_tasks"]],
        "controller_cos_inputs": [m["message_id"] for m in snap["controller_cos_inputs"]],
        "controller_inbox_items": [i["item_id"] for i in snap["controller_inbox_items"]],
    }
    n_safety = sum(len(v) for v in safety.values())
    n_fp = sum(f["false_positive"] for f in fires)
    n_lead = sum(f["lead"] for f in fires)
    if n_safety >= 1:
        verdict, reason = "unsafe", f"safety violations {n_safety}"
    elif not shadow_projects:
        verdict, reason = "inconclusive", "no project with controller mode shadow"
    elif roots_in_shadow == 0:
        verdict, reason = "inconclusive", "no strategy root in a shadow project"
    elif ripe_in_shadow == 0:
        verdict, reason = "inconclusive", f"no strategy root in a shadow project with {K}+ terminal children"
    elif n_fp >= 2:
        verdict, reason = "falsified", f"false positives {n_fp} (>= 2)"
    elif n_lead >= 1 and n_fp == 0:
        verdict, reason = "useful", f"leads {n_lead}, false positives 0"
    else:
        verdict, reason = "no_signal", f"fires {len(fires)}, leads {n_lead}, false positives {n_fp}"
    return {
        "schema_id": RESULT_SCHEMA,
        "k": K,
        "exported_at": snap["exported_at"],
        "verdict": verdict,
        "reason": reason,
        "counts": {
            "shadow_projects": len(shadow_projects),
            "strategy_roots": sum(len(p["strategy_roots"]) for p in projects),
            "strategy_roots_in_shadow": roots_in_shadow,
            "roots_with_k_terminal_children_in_shadow": ripe_in_shadow,
            "r1_fires": len(fires),
            "r1_waiting": snap["counts"]["n_r1_waiting"],
            "false_positives": n_fp,
            "leads": n_lead,
            "guard_escalates": len(guards),
            "outcome_reached": len(reached),
            "safety_violations": n_safety,
            "decisions": snap["counts"]["n_decisions"],
        },
        "fires": fires,
        "safety": safety,
        "projects": project_rows,
        "script_domains": [{"domain_id": d["domain_id"], "mode": d["mode"]} for d in snap["script_domains"]],
    }


# ---------------------------------------------------------------- verify

def snapshot_problems(node, path="$"):
    """snapshot が許した key（id・時刻・status・規則の id・effect_state・件数と入れ物）だけを持ち、
    200 字を超える文字列が無いことを確かめる。問題の一覧を返す。"""
    out = []
    if isinstance(node, dict):
        for key, value in node.items():
            here = f"{path}.{key}"
            if key not in ALLOWED_KEYS:
                out.append(f"{here}: key not allowed")
            elif key in COUNT_KEYS and (not isinstance(value, int) or isinstance(value, bool)):
                out.append(f"{here}: count must be an integer")
            elif key in LIST_KEYS and not isinstance(value, list):
                out.append(f"{here}: must be a list")
            elif key in DICT_KEYS and not isinstance(value, dict):
                out.append(f"{here}: must be an object")
            elif key not in LIST_KEYS | DICT_KEYS and isinstance(value, (list, dict)):
                out.append(f"{here}: scalar key holds a container")
            out.extend(snapshot_problems(value, here))
    elif isinstance(node, list):
        for i, value in enumerate(node):
            if not isinstance(value, dict):
                out.append(f"{path}[{i}]: list items must be objects")
            out.extend(snapshot_problems(value, f"{path}[{i}]"))
    elif isinstance(node, str):
        if len(node) > STRING_MAX:
            out.append(f"{path}: string over {STRING_MAX} chars")
    elif node is not None and not isinstance(node, (int, bool)):
        out.append(f"{path}: unexpected value type {type(node).__name__}")
    return out


def verify(directory):
    snaps = sorted(directory.glob("*-snapshot.json"))
    if not snaps:
        print(f"verify: no *-snapshot.json in {directory}")
        return 1
    failed = 0
    for snap_path in snaps:
        name = snap_path.name[: -len("-snapshot.json")]
        result_path = directory / f"{name}.json"
        snap = json.loads(snap_path.read_text())
        problems = snapshot_problems(snap)
        if problems:
            failed += 1
            print(f"verify: {snap_path.name}: {len(problems)} problem(s): {problems[:5]}")
            continue
        if not result_path.exists():
            failed += 1
            print(f"verify: {result_path.name} missing")
            continue
        if json.loads(result_path.read_text()) != judge(snap):
            failed += 1
            print(f"verify: {result_path.name} differs from a recount of {snap_path.name}")
            continue
        print(f"verify: {name}: ok (verdict {judge(snap)['verdict']})")
    return 1 if failed else 0


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    ex = sub.add_parser("export")
    ex.add_argument("--out", required=True, type=Path)
    ex.add_argument("--db", default=DB_PATH, help="sqlite の path（読み取り専用で開く）")
    rn = sub.add_parser("run")
    rn.add_argument("--snapshot", required=True, type=Path)
    rn.add_argument("--out", required=True, type=Path)
    vf = sub.add_parser("verify")
    vf.add_argument("--dir", default=OUT_DIR, type=Path)
    args = ap.parse_args(argv)
    if args.cmd == "export":
        conn = sqlite3.connect(f"file:{args.db}?mode=ro", uri=True)
        try:
            now = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%fZ")
            snap = export(conn, now)
        finally:
            conn.close()
        problems = snapshot_problems(snap)
        if problems:
            print(f"export: snapshot problems: {problems[:5]}")
            return 1
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(dumps(snap))
        print(f"export: {args.out} ({snap['counts']})")
        return 0
    if args.cmd == "run":
        snap = json.loads(args.snapshot.read_text())
        problems = snapshot_problems(snap)
        if problems:
            print(f"run: snapshot problems: {problems[:5]}")
            return 1
        result = judge(snap)
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(dumps(result))
        print(f"run: {args.out} verdict={result['verdict']} ({result['reason']})")
        return 0
    return verify(args.dir)


if __name__ == "__main__":
    sys.exit(main())
