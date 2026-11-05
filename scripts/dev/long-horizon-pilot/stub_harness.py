#!/usr/bin/env python3
"""A9 の stub harness（LLM を呼ばない）。fake adapter の RunRequest を stdin で受ける。

- Review の run: artifacts/review.json に全基準の合格を書く。
- 実行の run: acceptance の文から Outcome を見つけ、作業 tree の out/<id>.txt に台本の答えを書き、git commit する。
- CoS の chat run と planner はそのまま done を返す。起動ごとに 1 行を artifacts/stub-run.jsonl に書く。
"""
import json
import os
import pathlib
import re
import subprocess
import sys
import time


def arg(name):
    return sys.argv[sys.argv.index(name) + 1]


def commit_outcomes(cwd, outcomes):
    """out/<id>.txt を作業 tree の branch に commit する（案件の tree に載せる）。"""
    env = {**os.environ, "GIT_AUTHOR_NAME": "stub", "GIT_AUTHOR_EMAIL": "stub@localhost",
           "GIT_COMMITTER_NAME": "stub", "GIT_COMMITTER_EMAIL": "stub@localhost"}
    paths = [f"out/{o}.txt" for o in outcomes]
    subprocess.run(["git", "-C", str(cwd), "add", "--", *paths], check=True, env=env, capture_output=True)
    subprocess.run(["git", "-C", str(cwd), "commit", "-q", "-m", "stub: " + " ".join(outcomes), "--", *paths],
                   check=True, env=env, capture_output=True)


def main():
    req = json.load(sys.stdin)
    answers = json.loads(pathlib.Path(arg("--answers")).read_text())
    task = req["task"]
    kind = task.get("kind")
    cwd = pathlib.Path(req.get("work_dir") or req["workspace"])
    artifacts = pathlib.Path(req["artifacts_dir"])
    artifacts.mkdir(parents=True, exist_ok=True)
    wrote = []
    if kind == "review":
        criteria = task.get("acceptance") or []
        verdicts = [{"criterion": i, "pass": True, "reason": "stub"} for i in range(max(1, len(criteria)))]
        (artifacts / "review.json").write_text(json.dumps({"verdicts": verdicts}))
    else:
        text = json.dumps(task.get("acceptance") or [], ensure_ascii=False)
        for outcome in sorted(set(re.findall(r"[a-z]+-(o\d+):", text))):
            if outcome in answers:
                out = cwd / "out"
                out.mkdir(parents=True, exist_ok=True)
                (out / f"{outcome}.txt").write_text(answers[outcome])
                wrote.append(outcome)
        if wrote:
            commit_outcomes(cwd, wrote)
    # 作業場所の外は読み取り専用（worker の sandbox）。記録は run の成果物の dir に置く。
    with open(artifacts / "stub-run.jsonl", "a", encoding="utf-8") as f:
        f.write(json.dumps({"at": time.time(), "kind": kind, "task": task.get("id"), "title": task.get("title"),
                            "wrote": wrote}, ensure_ascii=False) + "\n")
    print(json.dumps({"type": "progress", "msg": "stub", "kind": "status", "summary": "stub"}))
    print(json.dumps({"type": "done", "summary": "stub harness (no LLM)", "evidence": []}))


if __name__ == "__main__":
    main()
