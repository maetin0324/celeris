#!/usr/bin/env python3
"""対照 pilot の stub（LLM を呼ばない。fake adapter の RunRequest を stdin で受ける）。

本番の起動口の経路を確かめるためだけに使う。作業 tree の file を読み、読んだ path を progress の `read: <path>` で出す
（driver が run の stdout.jsonl から reads.json に写す）。run の役で書くものを変える:
- 評価（`context.review.parent_checkpoint` がある。controller の evaluate → start_side_review）: prompt の出力の例と同じ形の
  `parent_evaluation` を artifacts/review.json に書く。
- 版を書く worker（`context.review` が無く、objective に成果物の名 `strategy/<id>/v1.json` がある。方式 c の段）: 形だけの
  `celeris.strategy/1` の文書（候補 2 件）を artifacts/strategy/<id>/v1.json に書き、`artifact` 行でその名を申告する
  （申告しない成果物は file の basename の名で記録され、Critic の対象にならない）。
- final review（上の 2 つ以外の `context.review`）: 条件ごとの合格の verdict を artifacts/review.json に書く。
  `strategy_critic` があれば（版を書いた task。Critic）、対象の文書の `candidates[].id` ごとの `critic` も書く。
判定・文書の中身は評価ではない（run.json の harness は stub で、試験 parent_checkpoint_blind_live_checks は既定の dir の
stub の出力を拒む）。
"""
import json
import pathlib
import re
import sys

USAGE = {"input_tokens": 10, "output_tokens": 5, "cache_read_tokens": 100, "cache_creation_tokens": 20,
         "cost_usd": 0.0001}
EVALUATION = {
    "schema": "celeris.parent-evaluation/1",
    "verdict": "unknown",
    "next": "ask_human",
    "axes": {"execution": "partial", "epistemic": "none", "goal": "not_met"},
    "measured": [],
    "learned": [],
    "refuted": [],
    "uncertainty": [{"statement": "stub: 評価していない", "resolve_by": "実 LLM の評価 run"}],
    "reason": "stub reviewer（LLM を呼ばない経路の確認）",
}


STRATEGY_NAME = re.compile(r"strategy/([a-z0-9][a-z0-9-]*)/v1\.json")


def read_tree(cwd):
    for p in sorted(cwd.iterdir()):
        if p.is_file():
            p.read_bytes()
            print(json.dumps({"type": "progress", "kind": "status", "msg": f"read: {p}", "summary": "stub"}), flush=True)


def strategy_document(strategy_id, project_id):
    """形だけの版 v1（stub。時点の slice の語を使わない）。"""
    def candidate(cid, hyp):
        return {"id": cid, "title": f"stub 候補 {cid}", "approach": "stub", "decision_method": "stub",
                "cost": {"usd_max": 1, "wall_hours_max": 1}, "risks": [], "reevaluate_when": ["stub"],
                "hypotheses": [hyp], "selected": False, "state": "open",
                "experiment": {"content": "stub", "discriminates": [hyp], "budget": {"usd_max": 1, "wall_hours_max": 1}},
                "expected_information_value": "stub", "stop_when": ["stub"]}
    return {"schema": "celeris.strategy/1", "project_id": project_id, "strategy_id": strategy_id, "version": 1,
            "supersedes": None, "idempotency_key": f"strategy:{strategy_id}:v1",
            "source": {"kind": "celeris_task_run", "label": "stub"}, "actor": {"kind": "worker", "id": "stub"},
            "decision_state": "proposed", "research_question": {"text": "stub", "references": []},
            "candidates": [candidate("stub-a", "h-a"), candidate("stub-b", "h-b")],
            "held_assumptions": [],
            "hypotheses": [{"id": h, "statement": "stub", "confidence": "estimated", "falsified_by": "stub"}
                           for h in ("h-a", "h-b")],
            "evidence": []}


def final_review_output(review, workspace):
    verdicts = [{"criterion": i, "pass": True, "reason": "stub"} for i in review.get("criteria", [])]
    critic = review.get("strategy_critic")
    if not critic:
        return {"verdicts": verdicts}
    ok = {"ok": True, "reason": "stub（LLM を呼ばない経路の確認）"}
    subjects = []
    for s in critic["subjects"]:
        doc = json.loads((workspace / s["path"]).read_text())
        subjects.append({"name": s["name"], "sha256": s["sha256"],
                         "candidates": [{"id": c["id"], "defects": [], "falsification": ok, "experiment": ok}
                                        for c in doc.get("candidates", [])],
                         "comparison": ok, "revisit": ok})
    return {"verdicts": verdicts, "critic": {"subjects": subjects}}


def main():
    req = json.load(sys.stdin)
    workspace = pathlib.Path(req["workspace"])
    cwd = pathlib.Path(req.get("work_dir") or req["workspace"])
    artifacts = pathlib.Path(req["artifacts_dir"])
    artifacts.mkdir(parents=True, exist_ok=True)
    read_tree(cwd)
    review = (req.get("context") or {}).get("review")
    if review and review.get("parent_checkpoint"):
        out = {"verdicts": [], "parent_evaluation": EVALUATION}
        (artifacts / "review.json").write_text(json.dumps(out, ensure_ascii=False))
        summary = "stub reviewer (no LLM)"
    elif review:
        (artifacts / "review.json").write_text(json.dumps(final_review_output(review, workspace), ensure_ascii=False))
        summary = "stub final review (no LLM)"
    else:
        m = STRATEGY_NAME.search(req["task"].get("objective") or "")
        if not m:
            print(json.dumps({"type": "error", "message": "stub: no strategy/<id>/v1.json in the objective",
                              "retryable": False}))
            return
        dest = artifacts / m.group(0)
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(json.dumps(strategy_document(m.group(1), req["task"].get("project_id")), ensure_ascii=False,
                                   indent=1))
        print(json.dumps({"type": "artifact", "name": m.group(0), "path": dest.relative_to(workspace).as_posix()}),
              flush=True)
        summary = "stub strategy writer (no LLM)"
    print(json.dumps({"type": "done", "summary": summary, "evidence": [], "usage": USAGE}))


if __name__ == "__main__":
    main()
