#!/usr/bin/env python3
"""対照 pilot の比較と盲検の材料の試験に使う合成の run の記録を作る（LLM も daemon も使わない）。

    python3 -I scripts/dev/parent-eval-blind/testdata/make_pilot_runs.py <testdata の dir>

書くもの（driver の出力と同じ配置。<dir>/<case>/<point>/<arm>/ と <dir>/<case>/strategy-v1/）:
- pilot-runs/iter-1/: replay の C-mid・D-done・F-done と研究ケースの 11.17 × 方式 a・b・c、事例ごとの版 v1 の段。漏れは無い。
  研究ケースの作業 tree は合成の testdata/research/c-results.md から research.files で切る（実データを使わない）。
- pilot-runs-leak/iter-1/: 漏れを 1 つずつ混ぜた 4 件。replay/C-mid/b の events.jsonl に F-done の時点にだけある史実の event、
  replay/C-mid/c の prompt.md に answer key の C-mid の理由の文、replay/D-done/b の出力に未来にだけ現れる語 parent-unmet、
  research/11.17/b の slice に 11.20 で初めて現れる語 1 / 488。
判定・主張・数値は作り物で、評価ではない。試験 test_pilot_compare.py が、この script の出力と commit した file の一致を確かめる。
"""
import hashlib
import json
import pathlib
import shutil
import sys

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
import research  # noqa: E402

CHILD0 = HERE.parents[3] / "agent-docs/progress/2026-11-21-strategy-v1-0"
SHADOW = HERE.parents[3] / "crates/task-api/tests/fixtures/parent_checkpoint_shadow"
SYNTH = HERE / "research/c-results.md"
CHILDREN = "ABCDEFGH"
# 方式 b・c の合成の判定（時点ごと）。a は親の途中評価が無い（verdict 無し・continue）。
VERDICTS = {
    "b": {"C-mid": ("at_risk", "continue"), "D-done": ("off_track", "pivot"), "F-done": ("off_track", "ask_human"),
          "11.17": ("at_risk", "continue")},
    "c": {"C-mid": ("at_risk", "discriminating_experiment"), "D-done": ("off_track", "pivot"),
          "F-done": ("off_track", "ask_human"), "11.17": ("at_risk", "discriminating_experiment")},
}
USAGE = {"input_tokens": 4, "output_tokens": 900, "cache_read_tokens": 1000, "cache_creation_tokens": 2000,
         "cost_usd": 0.125}


def dump(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n")


def ids(case, point, arm):
    tag = f"{case}-{point}-{arm}".upper().replace(".", "").replace("-", "")
    root = f"01SYNROOT{tag}"[:26].ljust(26, "0")
    return root, {k: f"01SYNCHILD{k}{tag}"[:26].ljust(26, "0") for k in CHILDREN}


def replay_tree(point):
    cutoff = json.loads((SHADOW / point / "slice.json").read_text())["cutoff"]
    done = json.loads((SHADOW / point / "slice.json").read_text())["children_done"]
    events = "".join(json.dumps({"seq": i, "task": k, "ts": cutoff[:11] + "00:00:0%d.000000000Z" % i,
                                 "type": "reviewer_passed"}, sort_keys=True) + "\n" for i, k in enumerate(done))
    return {"goal.md": "# 合成の親の目的（試験用）\n\n合成の指標を 17 から減らす。\n",
            "events.jsonl": events,
            "slice.json": json.dumps({"cutoff": cutoff, "children_done": done}, sort_keys=True) + "\n"}


def research_tree(point):
    tree, _ = research.files(SYNTH, point)
    return {k: v.decode() for k, v in tree.items()}


def write_input(d, tree, request, prompt):
    files = []
    for name, text in sorted(tree.items()):
        (d / "input/tree" / name).parent.mkdir(parents=True, exist_ok=True)
        (d / "input/tree" / name).write_text(text)
        data = text.encode()
        files.append({"path": name, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})
    dump(d / "input/files.json", {"work_tree": "/synthetic/home/slice-repo", "files": files})
    dump(d / "input/request.json", request)
    if prompt is not None:
        (d / "input/prompt.md").write_text(prompt)


def reads(extra=()):
    return {"work_tree": "/synthetic/home/slice-repo", "home": "/synthetic/home", "logs": [],
            "reads": [{"tool": "Read", "path": "/synthetic/home/slice-repo/goal.md"},
                      {"tool": "Bash", "command": "cat > /synthetic/home/slice-repo/artifacts/review.json <<'EOF'"},
                      *extra]}


def point(root, case, pt, arm, strategy=None):
    d = root / case / pt / arm
    rid, children = ids(case, pt, arm)
    tree = replay_tree(pt) if case == "replay" else research_tree(pt)
    if strategy is not None:
        tree["strategy-v1.json"] = json.dumps(strategy, ensure_ascii=False, sort_keys=True) + "\n"
    done = json.loads(tree["slice.json"])["children_done"] if case == "replay" else []
    if arm == "a":
        write_input(d, tree, {"arm": "a", "llm_runs": 0}, None)
        claims = [{"kind": "local_review", "statement": f"子 {k} の局所の review が合格（合成）",
                   "refs": [{"kind": "task", "task_id": children[k]}]} for k in done]
        output = {"schema": "celeris.pilot-arm-output/1", "case": case, "point": pt, "arm": "a", "verdict": None,
                  "next": "continue", "status": "fixed_plan", "claims": claims, "usage": None}
        run = {"llm_runs": 0, "usd": None, "harness": "stub"}
        rd = {"work_tree": "/synthetic/home/slice-repo", "home": "/synthetic/home", "logs": [], "reads": []}
    else:
        verdict, nxt = VERDICTS[arm][pt]
        write_input(d, tree, {"context": {"review": {"parent_checkpoint": {"point": "synthetic"}}}},
                    f"# 合成の prompt（{pt}）\n\n親の Outcome: 合成の指標を減らす（到達: false）。\n")
        claims = [{"kind": "measured", "statement": f"合成の指標は {pt} の時点で変わっていない",
                   "refs": [{"kind": "task", "task_id": children["A"]}]},
                  {"kind": "learned", "statement": "合成の仮説は支持されていない", "refs": []},
                  {"kind": "refuted", "statement": "合成の外の資料による主張",
                   "refs": [{"kind": "url", "url": "https://example.invalid/outside"}]},
                  {"kind": "uncertainty", "statement": "合成の残りの不確実性", "refs": []}]
        output = {"schema": "celeris.pilot-arm-output/1", "case": case, "point": pt, "arm": arm, "verdict": verdict,
                  "next": nxt, "status": "checked", "claims": claims, "usage": USAGE,
                  "record": {"status": "checked", "decision_id": "01SYNDECISION", "effective": {"verdict": verdict, "next": nxt},
                             "evaluation": {"reason": f"合成の理由（{pt}）", "axes": {"execution": "succeeded",
                                                                                  "epistemic": "advanced", "goal": "not_met"}}}}
        if strategy is not None:
            output["strategy"] = {"name": f"strategy/pilot-{case}/v1.json", "critic": [{"pass": True, "reason": "合成"}]}
        run = {"llm_runs": 1, "usd": 0.125, "harness": "stub", "run_id": f"01SYNRUN{arm.upper()}"}
        rd = reads()
    run.update({"case": case, "point": pt, "arm": arm, "root_id": rid, "children": children, "project_id": "01SYNPROJECT"})
    dump(d / "output.json", output)
    dump(d / "reads.json", rd)
    dump(d / "run.json", run)
    return d


def strategy_doc(case):
    return {"schema": "celeris.strategy/1", "strategy_id": f"pilot-{case}", "version": 1,
            "candidates": [{"id": "syn-a", "title": "合成の候補 a", "hypotheses": ["h-a"],
                            "experiment": {"content": "合成の識別実験 a"}},
                           {"id": "syn-b", "title": "合成の候補 b", "hypotheses": ["h-b"],
                            "experiment": {"content": "合成の識別実験 b"}}]}


def stage(root, case, slice_point):
    d = root / case / "strategy-v1"
    tree = replay_tree(slice_point) if case == "replay" else research_tree(slice_point)
    write_input(d, tree, {"objective": "合成の版 v1 を書く", "artifact": f"strategy/pilot-{case}/v1.json"}, None)
    doc = strategy_doc(case)
    (d / "strategy-v1.json").write_text(json.dumps(doc, ensure_ascii=False, sort_keys=True) + "\n")
    dump(d / "strategy-v1.critic.json", {"task_status": "done", "review_events": [
        {"type": "review_verdict", "pass": True, "reason": "合成の Critic"}], "review_json": None})
    dump(d / "reads.json", {"work_tree": "/synthetic/home/slice-repo", "home": "/synthetic/home", "reads": []})
    dump(d / "run.json", {"case": case, "stage": "strategy-v1", "slice_point": slice_point, "harness": "stub",
                          "task_status": "done", "llm_runs": 2, "usd": 0.25,
                          "strategy": {"name": f"strategy/pilot-{case}/v1.json", "sha256": "sha256:" + "0" * 64}})
    return doc


def clean(root):
    for case, pts, slice_point in (("replay", ("C-mid", "D-done", "F-done"), "A-mid"), ("research", ("11.17",), "11.15")):
        doc = stage(root, case, slice_point)
        for pt in pts:
            for arm in "abc":
                point(root, case, pt, arm, doc if arm == "c" else None)


def refresh_listing(d):
    """漏れを混ぜた後の input/files.json（mutation でなく漏れだけが出るように、混ぜた後の sha256 を書く）。"""
    listing = json.loads((d / "input/files.json").read_text())
    for f in listing["files"]:
        data = (d / "input/tree" / f["path"]).read_bytes()
        f.update({"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})
    dump(d / "input/files.json", listing)


def future_event(after, until):
    """C-mid の後の時点（F-mid の cutoff の後、F-done の cutoff まで）にだけある史実の event の行。"""
    for line in (CHILD0 / "replay/events.jsonl").read_text().splitlines():
        e = json.loads(line)
        if after < e["ts"] <= until and e["type"] == "worker_finished":
            return line
    raise SystemExit("no historical event between F-mid and F-done")


def leak(root):
    doc = strategy_doc("replay")
    d = point(root, "replay", "C-mid", "b")
    f_mid = json.loads((SHADOW / "F-mid/slice.json").read_text())["cutoff"]
    f_done = json.loads((SHADOW / "F-done/slice.json").read_text())["cutoff"]
    with open(d / "input/tree/events.jsonl", "a") as f:
        f.write(future_event(f_mid, f_done) + "\n")
    refresh_listing(d)
    d = point(root, "replay", "C-mid", "c", doc)
    why = json.loads((CHILD0 / "replay/answer_key.json").read_text())["checkpoints"]["C-mid"]["why"]
    with open(d / "input/prompt.md", "a") as f:
        f.write(f"\n{why}\n")
    d = point(root, "replay", "D-done", "b")
    out = json.loads((d / "output.json").read_text())
    out["claims"][0]["statement"] += "（parent-unmet）"
    dump(d / "output.json", out)
    d = point(root, "research", "11.17", "b")
    with open(d / "input/tree" / research.SLICE_FILE, "a") as f:
        f.write("\n- 合成の 1 / 488 cell\n")
    refresh_listing(d)


def main(argv):
    if len(argv) != 1:
        print("usage: make_pilot_runs.py <testdata dir>", file=sys.stderr)
        return 2
    base = pathlib.Path(argv[0])
    for name, build in (("pilot-runs", clean), ("pilot-runs-leak", leak)):
        shutil.rmtree(base / name, ignore_errors=True)
        build(base / name / "iter-1")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
