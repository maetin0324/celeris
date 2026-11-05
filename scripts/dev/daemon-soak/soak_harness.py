#!/usr/bin/env python3
"""P4-F の fake harness（LLM を呼ばない）。fake adapter の RunRequest を stdin で受け、すぐ答える。

- review の run: artifacts/review.json に、渡された基準を全部 pass と書く（acceptance は `true` の command だけ）。
- CoS の chat run・planner・実行の run: done を返す（chat の返事は done.summary）。
外部ネットワーク・API・DB に触れない。CPU を使わない（1 回の起動で数十 ms と `--sleep` の秒）。
"""
import json
import pathlib
import sys
import time


def main():
    req = json.load(sys.stdin)
    # `--sleep N`: 実行の run を N 秒だけ持つ（restart の回収を測るため。sleep は CPU を使わない）。
    if "--sleep" in sys.argv and (req.get("task") or {}).get("kind") != "review":
        time.sleep(float(sys.argv[sys.argv.index("--sleep") + 1]))
    task = req.get("task") or {}
    artifacts = req.get("artifacts_dir")
    if task.get("kind") == "review" and artifacts:
        review = (req.get("context") or {}).get("review") or {}
        path = pathlib.Path(artifacts)
        path.mkdir(parents=True, exist_ok=True)
        verdicts = [{"criterion": i, "pass": True, "reason": "soak: command check `true`"}
                    for i in review.get("criteria") or []]
        (path / "review.json").write_text(json.dumps({"verdicts": verdicts}))
    print(json.dumps({"type": "progress", "msg": "soak", "kind": "status", "summary": "soak"}))
    print(json.dumps({"type": "done", "summary": "soak harness (no LLM)", "evidence": []}))


if __name__ == "__main__":
    main()
