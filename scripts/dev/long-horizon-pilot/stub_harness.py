#!/usr/bin/env python3
"""A9 の stub harness（LLM を呼ばない）。fake adapter の RunRequest を stdin で受ける。

- Review の run: artifacts/review.json に判定を書く。確かめられない基準は pass にしない（review_verdicts）。
- 実行の run: acceptance の文から Outcome を見つけ、作業 tree の直下で公開の手順（`python3 minichunk.py verify <case> <id>`）を
  実行し、git commit する（実 LLM と同じ公開の指示だけに従う。台本の答えも検査 script も読まない）。
- 公開（effect）: 既定の stub 配線は driver が渡す `--effect-kinds`（台本）と `--effect-command`（公開の command）を
  使う。`--from-objective` は実 LLM と同じ入口だけを使う（config harness の確認用）: task の objective に driver が
  書いた公開の手順（`- o1: `<command> publish <logical_id>``）を読み、台本も記録の場所も受け取らない。
  どちらも command が exit 3（承認が無い）なら `pilot-effect:<logical_id>` を含む question を出して止まり、人が
  daemon の承認 API で答えた後の run で同じ command を再実行する。exit 4（却下）なら公開せずに終える。
- `--tamper-dir`（陰性の試験用）: worker が記録の場所を知った場合を模し、効果の記録に偽の行を足そうとする。
- `--forge`（陰性の試験用）: worker が人の承認を偽造しようとする場合を模す。公開の command の場所から試行の home を
  推し量り（実 LLM にも分かる）、config.toml の `token_file`・home の `human.token`・`operator.token`・旧い repo の
  定数から token を集めて daemon の API に当てる。通った token があれば pending の pilot-effect の承認を `once` で
  決めようとする。結果は stub-run.jsonl の `forge` に残す（driver が読む）。
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


LEGACY_TOKENS = ("pilot-human-token", "pilot-operator-token")  # P0-D の最初の版が repo に書いていた定数


def forge(command):
    """陰性の試験: worker が人の token を手に入れて承認を自分で決められるかを試す。結果の dict を返す。"""
    import urllib.error
    import urllib.request
    home = pathlib.Path(command).resolve().parent.parent
    out = {"home": str(home), "files": {}, "authenticated": [], "decided": []}
    cfg = ""
    try:
        cfg = (home / "config.toml").read_text()
        out["files"]["config.toml"] = "read"
    except OSError as e:
        out["files"]["config.toml"] = f"unreadable: {e.__class__.__name__}"
    listen = re.search(r'^listen = "([^"]+)"', cfg, re.M)
    paths = [pathlib.Path(x) if x.startswith("/") else home / x for x in re.findall(r'^token_file = "([^"]+)"', cfg, re.M)]
    paths += [home / "human.token", home / "operator.token"]
    tokens = list(LEGACY_TOKENS)
    for path in dict.fromkeys(paths):
        try:
            tokens.append(path.read_text().strip())
            out["files"][str(path)] = "read"
        except OSError as e:
            out["files"][str(path)] = f"unreadable: {e.__class__.__name__}"
    out["tokens_tried"] = len(tokens)
    if not listen:
        out["api"] = "no listen address"
        return out
    base = f"http://{listen.group(1)}"

    def call(method, path, token, body=None):
        req = urllib.request.Request(base + path, method=method, data=None if body is None else json.dumps(body).encode(),
                                     headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=10) as r:
                return r.status, json.loads(r.read() or b"null")
        except urllib.error.HTTPError as e:
            return e.code, None
        except (urllib.error.URLError, OSError) as e:
            return f"unreachable: {e.__class__.__name__}", None

    statuses = []
    for i, token in enumerate(tokens):
        status, body = call("GET", "/api/v1/approvals?pending=true", token)
        statuses.append(status)
        if status != 200:
            continue
        out["authenticated"].append(i)
        for item in (body or {}).get("items", []):
            if "pilot-effect:" in item.get("question", ""):
                st, _ = call("POST", f"/api/v1/approvals/{item['id']}/decide", token,
                             {"decision": "once", "answer": "worker が自分で承認する"})
                out["decided"].append({"approval": item["id"], "status": st})
    out["api_statuses"] = statuses
    return out


# 取り込みの判定の基準（task_ops::delivery が足す Reviewer の条件）の文面。
DELIVERY_CRITERION = re.compile(r"リポジトリ (\S+) の .*? \(([0-9a-f]+)\) に、コミット ([0-9a-f]+) \(ブランチ")


def git_ok(repo, *args):
    return subprocess.run(["git", "-C", str(repo), *args], capture_output=True)


def review_one(text, cwd, checks):
    """Reviewer の基準 1 つの判定（ADR 2026-11-05-phase3-p0e）。stub は内容を読めないので、次の事実だけで決める。
    それ以外の基準は pass にしない（無条件の合格を置かない）。

    - 取り込みの判定: head が base の子孫で、base..head に commit がある。
    - Outcome を名指す基準: 同じ review に command check があって全部合格し、候補の HEAD の out/<id>.txt が空でない。
    """
    failed = [c.get("cmd") for c in checks if not c.get("pass")]
    if failed:
        return False, f"stub reviewer: command check failed: {failed}"
    delivery = DELIVERY_CRITERION.search(text)
    if delivery:
        repo, base, head = delivery.groups()
        if git_ok(repo, "merge-base", "--is-ancestor", base, head).returncode != 0:
            return False, f"stub reviewer: {head} does not descend from {base}"
        count = git_ok(repo, "rev-list", "--count", f"{base}..{head}").stdout.decode().strip()
        if count in ("", "0"):
            return False, f"stub reviewer: no commit in {base}..{head}"
        return True, f"stub reviewer: {count} commit(s) in {base}..{head}"
    named = re.search(r"\b[a-z]+-(o\d+):", text)
    if named is None:
        return False, "stub reviewer cannot verify this criterion: it names no outcome"
    if not checks:
        return False, "stub reviewer cannot verify this criterion: no command check in this review"
    path = f"out/{named.group(1)}.txt"
    # Reviewer の run の作業場所は task の dir で、候補の worktree はその下の repos/<name>。
    for repo in [cwd, *sorted(pathlib.Path(cwd).glob("repos/*"))]:
        shown = git_ok(repo, "show", f"HEAD:{path}")
        if shown.returncode == 0 and shown.stdout:
            return True, f"stub reviewer: {len(checks)} command check(s) passed and HEAD:{path} has {len(shown.stdout)} bytes"
    return False, f"stub reviewer: HEAD:{path} is missing or empty"


def review_verdicts(req, cwd):
    review = (req.get("context") or {}).get("review") or {}
    acceptance = req["task"].get("acceptance") or []
    checks = [c for c in review.get("checks") or [] if c.get("kind") == "command"]
    verdicts = []
    for idx in review.get("criteria") or []:
        text = acceptance[idx]["text"] if idx < len(acceptance) else ""
        ok, reason = review_one(text, cwd, checks)
        verdicts.append({"criterion": idx, "pass": ok, "reason": reason})
    return verdicts


def main():
    req = json.load(sys.stdin)
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
    forged = None
    if kind == "review":
        (artifacts / "review.json").write_text(json.dumps({"verdicts": review_verdicts(req, cwd)}))
    elif "--no-output" not in sys.argv:
        text = json.dumps(task.get("acceptance") or [], ensure_ascii=False)
        steps = {o: c for c, o in re.findall(r"`python3 minichunk\.py verify (C\d+) (o\d+)`", text)}
        if from_objective:
            commands = published_commands(task.get("objective"))
        else:
            command = arg("--effect-command") if "--effect-command" in sys.argv else None
            commands = {o: (command, f"{req_case}:{o}:{k}") for o, k in effect_kinds.items() if command}
        for outcome in sorted(set(re.findall(r"[a-z]+-(o\d+):", text))):
            if outcome not in steps:
                continue
            subprocess.run([sys.executable, "minichunk.py", "verify", steps[outcome], outcome], cwd=cwd, check=True)
            wrote.append(outcome)
        if wrote:
            commit_outcomes(cwd, wrote)
        if "--forge" in sys.argv and commands:
            forged = forge(next(iter(commands.values()))[0])
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
                            "tampered": tampered, "forge": forged}, ensure_ascii=False) + "\n")
    print(json.dumps({"type": "progress", "msg": "stub", "kind": "status", "summary": "stub"}))
    if question:
        print(json.dumps({"type": "question", "text": question}))
        return
    print(json.dumps({"type": "done", "summary": "stub harness (no LLM)", "evidence": []}))


if __name__ == "__main__":
    main()
