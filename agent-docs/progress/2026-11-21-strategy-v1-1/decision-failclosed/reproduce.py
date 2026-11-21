#!/usr/bin/env python3
"""アーキv4 E（task 01M7Q5ZABN9NFQCH6QBTTP3EZ8）の決定の要求の取りこぼしを fixture で再現し、修正前の原因の行を確かめる。

repo の root で、引数はこの形だけ:

    python3 -I agent-docs/progress/2026-11-21-strategy-v1-1/decision-failclosed/reproduce.py verify

確かめること（全部合えば exit 0、1 つでも違えば理由を出して exit 1）:
  1. fixture: seq 436 付近に「worker の決定の要求を記録できませんでした」の進行の行が 1 行だけあり、その後の events に
     decision_requested が無く、decisions の行が 0 件のまま、task が done に達し、要求が止めるはずだった integrate-impl も
     done になっている。events は seq が連続している（間が抜けていない）。人の名前・秘密・hash を入れない欄の制限を守っている。
  2. git: ADR 2026-11-18-rejected-worker-decisions-reach-human を足した commit を git log --diff-filter=A で引き、その first
     parent（修正前）の tree_units.rs と task-core の decision.rs の file:line に期待の文字列があり、fixture の進行の文が
     その 2 つの format の文字列から組み立てた文と一致する。

標準ライブラリだけを使い、DB は読まない。
"""
import json
import pathlib
import re
import subprocess
import sys

HERE = pathlib.Path(__file__).resolve().parent
FIXTURE = HERE / "fixtures" / "e-target-leaf-events.json"
TASK = "01M7Q5ZABN9NFQCH6QBTTP3EZ8"
ADR = "agent-docs/adr/2026-11-18-rejected-worker-decisions-reach-human.md"
ADR_SUBJECT = "adr: 受け付けられない worker の決定の要求を"
TREE = "crates/task-dispatch/src/dispatcher/tree_units.rs"
DECISION = "crates/task-core/src/decision.rs"
PREFIX = "worker の決定の要求を記録できませんでした"

# 修正前（ADR の commit の first parent）の file:line と、その行に含まれるべき文字列
EXPECTED_LINES = [
    # 計画の unit のうち Integrate を除いた集合が needed_before の指せる先（integrate-impl が unknown になる理由）
    (TREE, 390, ".filter(|u| u.kind != task_core::WorkUnitKind::Integrate)"),
    # rejected を worker_progress の 1 行にするだけ
    (TREE, 419, "for reason in &batch.rejected {"),
    (TREE, 420, "events.push(Event::worker_progress("),
    (TREE, 422, 'format!("worker の決定の要求を記録できませんでした（ADR-0079 D7）: {reason}"),'),
    # accepted が空なら、止める行も self の止めも無いまま返す（run は成功のまま進む）
    (TREE, 434, "if batch.accepted.is_empty() {"),
    (TREE, 437, "held_rows: Vec::new(),"),
    (TREE, 438, "self_hold: false,"),
    # task-core decision: 捨てた要素は理由の文字列として積むだけ
    (DECISION, 568, "/// 捨てた要素と理由（人に見える進行の 1 行にする）。"),
    (DECISION, 569, "pub rejected: Vec<String>,"),
    (DECISION, 628, 'bad = Some(format!("unknown unit {n:?}"));'),
    (DECISION, 638, "out.rejected.push(format!("),
    (DECISION, 639, '"decision {}: needed_before names an {why} (use self, a unit key of the plan or stage:<key>)",'),
]

# fixture の event に置いてよい欄（これ以外の欄 = hash・account・path・コマンドの本文などは入れない）
ALLOWED_KEYS = {
    "seq", "ts", "type", "run_id", "key", "from", "to", "reason", "index", "total", "pass",
    "phase", "msg", "msg_excerpt", "outcome_excerpt",
}
HEX_WORD = re.compile(r"^[0-9a-f]{7,40}$")


class Failure(Exception):
    pass


def need(cond, why):
    if not cond:
        raise Failure(why)


def hex_words(text):
    """progress-index.sh の hex_words と同じ規則（7〜40 桁、数字と a-f を両方含む小文字の語）。"""
    out = []
    for w in re.split(r"[^0-9A-Za-z]+", text):
        if HEX_WORD.match(w) and re.search(r"[0-9]", w) and re.search(r"[a-f]", w):
            out.append(w)
    return out


def check_fixture():
    raw = FIXTURE.read_text(encoding="utf-8")
    fx = json.loads(raw)
    need(fx.get("task_id") == TASK, "fixture task_id is not %s" % TASK)
    need(not hex_words(raw), "fixture contains hash-like hex words: %s" % hex_words(raw)[:3])
    need("/home/" not in raw, "fixture contains a home path")
    events = fx["events"]
    for ev in events + fx["context_before"]:
        extra = set(ev) - ALLOWED_KEYS
        need(not extra, "event seq %s has fields outside the allow list: %s" % (ev.get("seq"), sorted(extra)))
    seqs = [e["seq"] for e in events]
    need(seqs == list(range(seqs[0], seqs[0] + len(seqs))), "fixture events are not contiguous by seq")

    req = fx["requested_decision"]
    lines = [e for e in events if e["type"] == "worker_progress" and e.get("msg", "").startswith(PREFIX)]
    need(len(lines) == 1, "expected exactly one progress line, found %d" % len(lines))
    line = lines[0]
    need(430 <= line["seq"] <= 440, "progress line seq %d is not near 436" % line["seq"])
    need(req["key"] in line["msg"], "progress line does not name the requested key")
    need('unknown unit "integrate-impl"' in line["msg"], "progress line does not name integrate-impl")

    after = [e for e in events if e["seq"] > line["seq"]]
    need(not [e for e in after if e["type"].startswith("decision_")],
         "a decision event follows the progress line")
    late_rows = [r for r in fx["decision_rows"] if r["created_at"] > line["ts"]]
    need(len(late_rows) == 0 and fx["decisions_rows_after_progress"] == 0,
         "decisions rows exist after the progress line")
    need(not [r for r in fx["decision_rows"] if r["key"] == req["key"]],
         "a decisions row with the requested key exists")
    need(fx["inbox_items_after_progress"] == 0, "inbox items exist after the progress line")
    need(not [e for e in after if e["type"] == "work_unit_transitioned" and e.get("to") == "blocked"],
         "a unit was blocked after the progress line")

    # 要求を出した leaf は進行の行の時点で done（run は成功のまま）
    leaf_done = [e for e in events if e["type"] == "work_unit_transitioned" and e.get("key") == fx["unit"]
                 and e.get("to") == "done" and e["seq"] < line["seq"]]
    need(leaf_done, "the requesting leaf was not done before the progress line")
    # 要求が止めるはずだった unit は答え無しに走って done になる
    target = req["needed_before"][0]
    tgt = [(e["from"], e["to"]) for e in after if e["type"] == "work_unit_transitioned" and e.get("key") == target]
    need(("pending", "running") in tgt and ("running", "done") in tgt,
         "%s did not run to done after the progress line: %s" % (target, tgt))
    # task の状態の遷移は連続し、done で終わる
    trans = [e for e in after if e["type"] == "transitioned"]
    need(trans and trans[-1]["to"] == "done", "the task did not reach done")
    for a, b in zip(trans, trans[1:]):
        need(a["to"] == b["from"], "task transitions are not continuous at seq %d" % b["seq"])
    need(events[-1] is trans[-1], "fixture does not end at the done transition")
    return fx, line, trans[-1]


def git(*args):
    p = subprocess.run(("git",) + args, capture_output=True, text=True, cwd=HERE)
    if p.returncode != 0:
        raise Failure("git %s failed: %s" % (" ".join(args), p.stderr.strip()))
    return p.stdout


def literal(src_line):
    m = re.search(r'"(.*)"', src_line)
    need(m is not None, "no string literal in: %s" % src_line.strip())
    return m.group(1)


def check_source(fx, line):
    adds = git("log", "--diff-filter=A", "--format=%H%x09%s", "--", ":(top)" + ADR).strip().splitlines()
    need(len(adds) == 1, "expected one commit adding the ADR, found %d" % len(adds))
    commit, subject = adds[0].split("\t", 1)
    need(subject.startswith(ADR_SUBJECT), "ADR commit subject differs: %s" % subject)
    files = {}
    for path in (TREE, DECISION):
        files[path] = git("show", "%s^1:%s" % (commit, path)).split("\n")
    shown = []
    for path, no, want in EXPECTED_LINES:
        have = files[path][no - 1] if no <= len(files[path]) else ""
        need(want in have, "%s:%d does not contain %r (has %r)" % (path, no, want, have.strip()))
        shown.append("%s:%d" % (path, no))
    # rejected の使い道は進行の 1 行だけ（tree_units.rs の中で batch.rejected を読むのは 1 か所）
    uses = sum(1 for t in files[TREE] if "batch.rejected" in t)
    need(uses == 1, "batch.rejected is read %d times in the pre-fix tree_units.rs" % uses)
    # fixture の進行の文 = 2 つの format の文字列から組み立てた文
    outer = literal(files[TREE][421])
    inner = literal(files[DECISION][638])
    req = fx["requested_decision"]
    reason = inner.replace("{}", req["key"], 1).replace(
        "{why}", 'unknown unit "%s"' % req["needed_before"][0])
    need(outer.replace("{reason}", reason) == line["msg"],
         "the progress line differs from the pre-fix format strings")
    return subject, shown


def main():
    if sys.argv[1:] != ["verify"]:
        print("usage: python3 -I %s verify" % sys.argv[0], file=sys.stderr)
        return 2
    try:
        fx, line, done = check_fixture()
        subject, shown = check_source(fx, line)
    except (Failure, OSError, KeyError, ValueError, IndexError) as e:
        print("FAIL: %s" % e)
        return 1
    print("ok: progress line seq %d (%s): %s" % (line["seq"], line["ts"], line["msg"]))
    print("ok: no decision event / decisions row / inbox item after it; task done at seq %d (%s)"
          % (done["seq"], done["ts"]))
    print("ok: pre-fix tree (first parent of '%s'):" % subject)
    for s in shown:
        print("    " + s)
    return 0


if __name__ == "__main__":
    sys.exit(main())
