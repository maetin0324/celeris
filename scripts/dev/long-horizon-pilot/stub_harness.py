#!/usr/bin/env python3
"""A9 の stub harness（LLM を呼ばない）。fake adapter の RunRequest を stdin で受ける。

- Review の run: artifacts/review.json に全基準の合格を書く。
- 実行の run: acceptance の文から Outcome を見つけ、作業 tree の out/<id>.txt を作る。acceptance が公開の手順
  （`python3 minichunk.py verify <case> <id>`）を書いていれば、作業 tree の直下でその手順を実行する（実 LLM と同じ
  公開の指示に従う）。手順の無い Outcome だけ台本の答え（--answers）を書く。
- CoS の chat run と planner はそのまま done を返す。起動ごとに 1 行を artifacts/stub-run.jsonl に書く。
"""
import json
import pathlib
import re
import subprocess
import sys
import time


def arg(name):
    return sys.argv[sys.argv.index(name) + 1]


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
        steps = {o: c for c, o in re.findall(r"`python3 minichunk\.py verify (C\d+) (o\d+)`", text)}
        for outcome in sorted(set(re.findall(r"[a-z]+-(o\d+):", text))):
            if outcome in steps:
                subprocess.run([sys.executable, "minichunk.py", "verify", steps[outcome], outcome], cwd=cwd, check=True)
                wrote.append(outcome)
            elif outcome in answers:
                out = cwd / "out"
                out.mkdir(parents=True, exist_ok=True)
                (out / f"{outcome}.txt").write_text(answers[outcome])
                wrote.append(outcome)
    # 作業場所の外は読み取り専用（worker の sandbox）。記録は run の成果物の dir に置く。
    with open(artifacts / "stub-run.jsonl", "a", encoding="utf-8") as f:
        f.write(json.dumps({"at": time.time(), "kind": kind, "task": task.get("id"), "title": task.get("title"),
                            "wrote": wrote}, ensure_ascii=False) + "\n")
    print(json.dumps({"type": "progress", "msg": "stub", "kind": "status", "summary": "stub"}))
    print(json.dumps({"type": "done", "summary": "stub harness (no LLM)", "evidence": []}))


if __name__ == "__main__":
    main()
