#!/usr/bin/env python3
"""A9 の stub harness（LLM を呼ばない）。fake adapter の RunRequest を stdin で受ける。

- Review の run: artifacts/review.json に全基準の合格を書く。
- 実行の run: acceptance の文から Outcome を見つけ、作業 tree の out/<id>.txt を作り、git commit する。acceptance が公開の手順
  （`python3 minichunk.py verify <case> <id>`）を書いていれば、作業 tree の直下でその手順を実行する（実 LLM と同じ
  公開の指示に従う）。手順の無い Outcome だけ台本の答え（--answers）を書く。
- 公開（effect）: 既定の stub 配線は driver が渡す `--effect-kinds`（台本）と `--effect-command`（公開の command）を
  使う。`--from-objective` は実 LLM と同じ入口だけを使う（config harness の確認用）: task の objective に driver が
  書いた公開の手順（`- o1: `<command> publish <logical_id>``）を読み、台本も記録の場所も受け取らない。
  どちらも command が exit 3（承認が無い）なら `pilot-effect:<logical_id>` を含む question を出して止まり、人が
  daemon の承認 API で答えた後の run で同じ command を再実行する。exit 4（却下）なら公開せずに終える。
- `--tamper-dir`（陰性の試験用）: worker が記録の場所を知った場合を模し、効果の記録に偽の行を足そうとする。
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
    staged = subprocess.run(["git", "-C", str(cwd), "diff", "--cached", "--quiet", "--", *paths], env=env)
    if staged.returncode == 0:
        return  # 承認の後の再開の run: 成果は前の run で commit 済み
    subprocess.run(["git", "-C", str(cwd), "commit", "-q", "-m", "stub: " + " ".join(outcomes), "--", *paths],
                   check=True, env=env, capture_output=True)


def published_commands(objective):
    """driver が objective に書いた公開の手順（`- o1: `<command> publish <logical_id>``）を読む。"""
    return {o: (cmd, lid) for o, cmd, lid in
            re.findall(r"^- (o\d+): `(\S+) publish (\S+)`", objective or "", re.M)}


def tamper(directory):
    """陰性の試験: 記録に偽の行を足す（権限で断られたら chmod してから）。成否を返す。"""
    p = pathlib.Path(directory) / "effects.jsonl"
    forged = json.dumps({"seq": 1, "at": time.time(), "logical_id": "C06:o2:publish", "approved": True,
                         "prev": "0" * 64, "mac": "0" * 64}) + "\n"
    try:
        os.chmod(directory, 0o700)
        os.chmod(p, 0o600)
        with open(p, "a", encoding="utf-8") as f:
            f.write(forged)
        return "appended"
    except OSError as e:
        return f"refused: {e}"


def main():
    req = json.load(sys.stdin)
    answers = json.loads(pathlib.Path(arg("--answers")).read_text()) if "--answers" in sys.argv else {}
    from_objective = "--from-objective" in sys.argv
    effect_kinds = json.loads(arg("--effect-kinds")) if "--effect-kinds" in sys.argv else {}
    req_case = arg("--case") if "--case" in sys.argv else ""
    task = req["task"]
    kind = task.get("kind")
    cwd = pathlib.Path(req.get("work_dir") or req["workspace"])
    artifacts = pathlib.Path(req["artifacts_dir"])
    artifacts.mkdir(parents=True, exist_ok=True)
    wrote = []
    effects = {}
    published = []
    question = None
    tampered = None
    if kind == "review":
        criteria = task.get("acceptance") or []
        verdicts = [{"criterion": i, "pass": True, "reason": "stub"} for i in range(max(1, len(criteria)))]
        (artifacts / "review.json").write_text(json.dumps({"verdicts": verdicts}))
    else:
        text = json.dumps(task.get("acceptance") or [], ensure_ascii=False)
        steps = {o: c for c, o in re.findall(r"`python3 minichunk\.py verify (C\d+) (o\d+)`", text)}
        if from_objective:
            commands = published_commands(task.get("objective"))
        else:
            command = arg("--effect-command") if "--effect-command" in sys.argv else None
            commands = {o: (command, f"{req_case}:{o}:{k}") for o, k in effect_kinds.items() if command}
        for outcome in sorted(set(re.findall(r"[a-z]+-(o\d+):", text))):
            if outcome in steps:
                subprocess.run([sys.executable, "minichunk.py", "verify", steps[outcome], outcome], cwd=cwd, check=True)
            elif outcome in answers:
                out = cwd / "out"
                out.mkdir(parents=True, exist_ok=True)
                (out / f"{outcome}.txt").write_text(answers[outcome])
            else:
                continue
            wrote.append(outcome)
        if wrote:
            commit_outcomes(cwd, wrote)
        if "--tamper-dir" in sys.argv and wrote:
            tampered = tamper(arg("--tamper-dir"))
        for outcome in wrote:
            if outcome not in commands:
                continue
            command, logical_id = commands[outcome]
            # 公開は公開された command を呼ぶだけ（記録は worker の外の broker が持つ）。
            rc = subprocess.run([command, "publish", logical_id]).returncode
            effects[outcome] = rc
            if rc == 0:
                published.append(outcome)
            elif rc == 3:
                question = f"pilot-effect:{logical_id} の公開（{outcome}）を承認しますか。承認なら公開し、却下なら公開しません。"
                break
            elif rc != 4:
                raise SystemExit(f"stub: pilot-effect exited {rc}")
    # 作業場所の外は読み取り専用（worker の sandbox）。記録は run の成果物の dir に置く。
    with open(artifacts / "stub-run.jsonl", "a", encoding="utf-8") as f:
        f.write(json.dumps({"at": time.time(), "kind": kind, "task": task.get("id"), "title": task.get("title"),
                            "wrote": wrote, "effects": effects, "published": published, "question": question,
                            "tampered": tampered}, ensure_ascii=False) + "\n")
    print(json.dumps({"type": "progress", "msg": "stub", "kind": "status", "summary": "stub"}))
    if question:
        print(json.dumps({"type": "question", "text": question}))
        return
    print(json.dumps({"type": "done", "summary": "stub harness (no LLM)", "evidence": []}))


if __name__ == "__main__":
    main()
