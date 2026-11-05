#!/bin/sh
# stub reviewer（stub_harness.py の Review の分岐）と driver の公開 check の試験（LLM・外部 network なし。K21）。
# 使い方: sh scripts/dev/long-horizon-pilot/test-stub-reviewer.sh
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/stub-reviewer.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
python3 -I - "$here" "$tmp" <<'PY'
import importlib.util, json, pathlib, shutil, subprocess, sys
here, tmp = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
fixtures = here.parents[2] / "tests/fixtures/long-horizon"
errors = []
def need(ok, msg):
    if not ok:
        errors.append(msg)
def git(repo, *args):
    env = ["-c", "user.name=t", "-c", "user.email=t@localhost"]
    return subprocess.run(["git", "-C", str(repo), *env, *args], check=True, capture_output=True)
ws = tmp / "task"
repo = ws / "repos" / "trial-repo"
repo.mkdir(parents=True)
git(repo, "init", "-q", "-b", "main")
shutil.copy(fixtures / "public/C08/repo-seed/minichunk.py", repo / "minichunk.py")
git(repo, "add", "-A")
git(repo, "commit", "-q", "-m", "seed")
(tmp / "answers.json").write_text("{}")
OK_CHECK = {"kind": "command", "cmd": "true", "pass": True, "reason": "exit 0"}
BAD_CHECK = {"kind": "command", "cmd": "false", "pass": False, "reason": "exit 1"}
def review(name, text, checks):
    art = tmp / "art" / name
    req = {"task": {"id": "t", "kind": "review", "title": "r",
                    "acceptance": [{"text": text, "check": {"type": "reviewer"}}]},
           "context": {"review": {"criteria": [0], "checks": checks}}, "workspace": str(ws),
           "artifacts_dir": str(art)}
    subprocess.run(["python3", "-I", str(here / "stub_harness.py"), "--answers", str(tmp / "answers.json")],
                   input=json.dumps(req), text=True, check=True, capture_output=True)
    return json.loads((art / "review.json").read_text())["verdicts"][0]
NAMED = "dorastiq-o1: 成果を書く"
# K21: 確かめられない基準は pass にしない。
v = review("no-outcome", "読みやすい文章であること", [OK_CHECK])
need(not v["pass"] and "cannot verify" in v["reason"], f"unverifiable criterion: {v}")
v = review("no-check", NAMED, [])
need(not v["pass"] and "cannot verify" in v["reason"], f"no command check: {v}")
v = review("failed-check", NAMED, [BAD_CHECK])
need(not v["pass"] and "failed" in v["reason"], f"failed check: {v}")
v = review("no-file", NAMED, [OK_CHECK])
need(not v["pass"] and "missing or empty" in v["reason"], f"no file: {v}")
(repo / "out").mkdir()
(repo / "out/o1.txt").write_text("not committed\n")
v = review("uncommitted", NAMED, [OK_CHECK])
need(not v["pass"], f"uncommitted file: {v}")
git(repo, "add", "-A")
git(repo, "commit", "-q", "-m", "o1")
v = review("verified", NAMED, [OK_CHECK])
need(v["pass"] and "reason" in v and "bytes" in v["reason"], f"verified: {v}")
# 取り込みの判定: head が base の子孫で、間に commit があるときだけ pass。
def rev(ref):
    return git(repo, "rev-parse", ref).stdout.decode().strip()
head, base = rev("HEAD"), rev("HEAD~1")
def delivery(b, h):
    return f"【部署内のマージ可否判定】リポジトリ {repo} の main ({b}) に、コミット {h} (ブランチ x) を取り込めること。"
v = review("delivery-ok", delivery(base, head), [])
need(v["pass"] and "1 commit" in v["reason"], f"delivery ok: {v}")
v = review("delivery-empty", delivery(head, head), [])
need(not v["pass"] and "no commit" in v["reason"], f"delivery empty: {v}")
v = review("delivery-reverse", delivery(head, base), [])
need(not v["pass"] and "does not descend" in v["reason"], f"delivery reverse: {v}")
v = review("delivery-failed-check", delivery(base, head), [BAD_CHECK])
need(not v["pass"], f"delivery with a failed check: {v}")
# review の情報が無い run は何も通さない（基準が無ければ verdict 0 件 = daemon が no verdict で落とす）。
art = tmp / "art/none"
req = {"task": {"id": "t", "kind": "review", "acceptance": [{"text": NAMED, "check": {"type": "reviewer"}}]},
       "context": {}, "workspace": str(ws), "artifacts_dir": str(art)}
subprocess.run(["python3", "-I", str(here / "stub_harness.py"), "--answers", str(tmp / "answers.json")],
               input=json.dumps(req), text=True, check=True, capture_output=True)
need(json.loads((art / "review.json").read_text())["verdicts"] == [], "a review without criteria passed")

# driver の公開 check（I8）: 全課題の全 Outcome に 1 つ以上付き、非公開の材料を含まない。
# 非公開の材料は oracle/<case>/oracle.json と oracle/holdout.json の canary、非公開の場所の名前（P0-C の bench v2）。
LOCATIONS = ("oracle/", "grader", "check_outcome")
def leaked(cmds, secrets):
    return sorted({s for c in cmds for s in (*secrets, *LOCATIONS) if s in c})
holdout_canary = json.loads((fixtures / "oracle/holdout.json").read_text())["canary"]
need(leaked(["true"], [holdout_canary]) == [], "the leak check hit a clean command")
need(leaked([f"echo {holdout_canary}"], [holdout_canary]) == [holdout_canary], "the leak check missed a canary")
need(leaked(["cat ../oracle/C01/oracle.json"], []) == ["oracle/"], "the leak check missed a private location")
spec = importlib.util.spec_from_file_location("driver", here / "driver.py")
driver = importlib.util.module_from_spec(spec)
spec.loader.exec_module(driver)
for f in sorted((fixtures / "public").glob("C*/case.json")):
    case = json.loads(f.read_text())
    secrets = [holdout_canary, json.loads((fixtures / "oracle" / case["case"] / "oracle.json").read_text())["canary"]]
    for o in case["request_outcomes"]:
        checks, source = driver.public_checks(case["case"], o)
        need(len(checks) >= 1 and all(c["cmd"].strip() for c in checks), f"{case['case']} {o['id']}: no check")
        need(any(x in source for x in ("public/", "request:")), f"{case['case']} {o['id']}: source {source}")
        hits = leaked([c["cmd"] for c in checks], secrets)
        need(not hits, f"{case['case']} {o['id']}: private material in a public check: {hits}")
# 公開の手順の check: 手順の出力と同じ commit だけ通り、別の内容・未 commit は落ちる。作業 tree を変えない。
c08 = next(o for o in json.loads((fixtures / "public/C08/case.json").read_text())["request_outcomes"] if o["id"] == "o1")
(cmd,), _ = driver.public_checks("C08", c08)
def run_check():
    return subprocess.run(["sh", "-c", cmd["cmd"]], cwd=repo, capture_output=True).returncode
need(run_check() != 0, "the public check passed a committed file with other content")
need((repo / "out/o1.txt").read_text() == "not committed\n", "the public check changed the work tree")
subprocess.run(["python3", "minichunk.py", "verify", "C08", "o1"], cwd=repo, check=True)
need(run_check() != 0, "the public check passed an uncommitted file")
git(repo, "add", "-A")
git(repo, "commit", "-q", "-m", "o1 by the public step")
need(run_check() == 0, "the public check failed the output of the public step")
git(repo, "rm", "-q", "out/o1.txt")
git(repo, "commit", "-q", "-m", "drop o1")
need(run_check() != 0, "the public check passed a missing file")
if errors:
    print("\n".join(errors), file=sys.stderr)
    sys.exit(1)
PY
echo "test-stub-reviewer: OK"
