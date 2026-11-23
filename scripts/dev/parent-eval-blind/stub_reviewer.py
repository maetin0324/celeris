#!/usr/bin/env python3
"""盲検の評価の stub reviewer（LLM を呼ばない。fake adapter の RunRequest を stdin で受ける）。

本番の起動口（controller の evaluate → start_side_review）の経路を確かめるためだけに使う。作業 tree（slice だけの seed の
git repo）の file を読み、読んだ path を progress の `read: <path>` で出し（driver が run の stdout.jsonl から reads.json に写す）、
prompt の出力の例と同じ形の `parent_evaluation` を artifacts/review.json に書く。判定の中身は評価ではない（run.json の
harness は stub で、試験 parent_checkpoint_blind_live_checks は既定の dir の stub の出力を拒む）。
"""
import json
import pathlib
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


def main():
    req = json.load(sys.stdin)
    cwd = pathlib.Path(req.get("work_dir") or req["workspace"])
    artifacts = pathlib.Path(req["artifacts_dir"])
    artifacts.mkdir(parents=True, exist_ok=True)
    for p in sorted(cwd.iterdir()):
        if p.is_file():
            p.read_bytes()
            print(json.dumps({"type": "progress", "kind": "status", "msg": f"read: {p}", "summary": "stub"}), flush=True)
    (artifacts / "review.json").write_text(json.dumps({"verdicts": [], "parent_evaluation": EVALUATION},
                                                      ensure_ascii=False))
    print(json.dumps({"type": "done", "summary": "stub reviewer (no LLM)", "evidence": [], "usage": USAGE}))


if __name__ == "__main__":
    main()
