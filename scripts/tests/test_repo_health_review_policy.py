"""repo health の profile の [review] 節と review policy の出力（ADR 2026-11-07-health-review-integration D2・D6.2）。

検査器の profile.validate が [review] の下位項目を検査すること、Celeris profile の値が実在の path を指すこと、
default.toml の空の [review] が extends の celeris の値を変えないこと、`--print-review-policy` が base 側の
profile だけを読んで schema の形の JSON を出すこと、check-health.sh が policy の file だけの変更を exit 0 に
することを確かめる。
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import textwrap
import tomllib
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TOOL_SRC = ROOT / "scripts/dev/repo-health"
sys.path.insert(0, str(TOOL_SRC))

import validate_report  # noqa: E402
from health import engine, profile  # noqa: E402
from health.snapshot import UsageError  # noqa: E402

PROFILES = TOOL_SRC / "profiles"
POLICY_SCHEMA = TOOL_SRC / "schema" / "review-policy-v1.schema.json"
REQUIRED_CLASSES = {"credential", "sandbox", "store", "events", "lease", "budget", "selfdeploy"}


def _load(name: str) -> dict:
    with open(PROFILES / f"{name}.toml", "rb") as fh:
        return tomllib.load(fh)


def _base(review) -> dict:
    return {"version": "1", "review": review}


class HealthReviewProfileTests(unittest.TestCase):
    def assert_invalid(self, review, needle: str) -> None:
        with self.assertRaises(UsageError) as cm:
            profile.validate(_base(review), "t.toml")
        self.assertIn("profile_invalid", str(cm.exception))
        self.assertIn(needle, str(cm.exception))

    def test_health_review_valid_section_passes(self):
        profile.validate(_base({
            "owner_map": "OWNERS.md",
            "high_risk": [{"class": "store", "paths": ["^src/store/"]}],
            "invariants": {"events-append-only": "RULES.md"},
        }), "t.toml")
        profile.validate(_base({}), "t.toml")

    def test_health_review_unknown_subkey_is_rejected(self):
        self.assert_invalid({"owner_map": "m.md", "risk_levels": []}, "未知の項目")

    def test_health_review_owner_map_must_be_string(self):
        self.assert_invalid({"owner_map": ["m.md"]}, "owner_map")
        self.assert_invalid({"owner_map": " "}, "owner_map")

    def test_health_review_high_risk_shape_is_checked(self):
        self.assert_invalid({"high_risk": {"class": "x", "paths": ["a"]}}, "high_risk")
        self.assert_invalid({"high_risk": [{"class": "x", "paths": ["a"], "level": "high"}]}, "class・paths だけ")
        self.assert_invalid({"high_risk": [{"paths": ["a"]}]}, "class")
        self.assert_invalid({"high_risk": [{"class": "x", "paths": []}]}, "paths")
        self.assert_invalid({"high_risk": [{"class": "x", "paths": ["("]}]}, "正規表現が不正")
        self.assert_invalid({"high_risk": [{"class": "x", "paths": ["a"]}, {"class": "x", "paths": ["b"]}]}, "重複")

    def test_health_review_invariants_shape_is_checked(self):
        self.assert_invalid({"invariants": ["CLAUDE.md"]}, "invariants")
        self.assert_invalid({"invariants": {"x": 1}}, "invariants")
        self.assert_invalid({"invariants": {"x": ""}}, "invariants")

    def test_health_review_section_must_be_table(self):
        self.assert_invalid("OWNERS.md", "review は表")

    def test_health_review_celeris_profile_points_at_existing_paths(self):
        rv = _load("celeris")["review"]
        self.assertTrue((ROOT / rv["owner_map"]).is_file())
        classes = {c["class"] for c in rv["high_risk"]}
        self.assertLessEqual(REQUIRED_CLASSES, classes)
        files = subprocess.run(["git", "-C", str(ROOT), "ls-files"], capture_output=True, text=True,
                               check=True).stdout.split()
        for c in rv["high_risk"]:
            for pat in c["paths"]:
                rx = re.compile(pat)
                self.assertTrue(any(rx.search(f) for f in files), f"{c['class']}: {pat} が追跡 file に当たらない")
        for name, src in rv["invariants"].items():
            self.assertTrue(os.path.isfile(ROOT / src), f"invariants.{name}: {src} が無い")
        for name in ("task-state-in-sqlite", "events-append-only", "no-llm-in-dispatcher"):
            self.assertIn(name, rv["invariants"])

    def test_health_review_default_extends_celeris_without_copying_values(self):
        default, celeris = _load("default"), _load("celeris")
        profile.validate(default, "default.toml")
        profile.validate(celeris, "celeris.toml")
        self.assertEqual(default.get("extends"), "celeris")
        self.assertEqual(default["review"], {})
        merged = profile._merge(celeris, default)
        self.assertEqual(merged["review"], celeris["review"])



def git(cwd: Path, *args: str) -> str:
    env = {**os.environ, "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@example.invalid",
           "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@example.invalid",
           "GIT_AUTHOR_DATE": "2026-01-01T00:00:00Z", "GIT_COMMITTER_DATE": "2026-01-01T00:00:00Z"}
    env.pop("CELERIS_WU_BASE", None)
    return subprocess.run(["git", "-C", str(cwd), *args], check=True, capture_output=True, text=True,
                          env=env).stdout.strip()


def schema_violations(value) -> list[str]:
    with open(POLICY_SCHEMA, encoding="utf-8") as fh:
        schema = json.load(fh)
    out = validate_report.Violations("policy")
    validate_report.check_schema(schema, schema, value, "", out)
    return out.items


CHILD = """version = "1"
extends = "team"
[review]
"""
PARENT = """version = "1"
[review]
owner_map = "OWNERS.md"
[[review.high_risk]]
class = "store"
paths = ["^src/store/"]
[[review.high_risk]]
class = "events"
paths = ['^src/events\\.rs$']
[review.invariants]
events-append-only = "RULES.md"
"""


class HealthReviewPolicyCliTests(unittest.TestCase):
    """--print-review-policy を一時 repo で走らせる。検査器は写しを使い、検査器自身の profiles/ を決める。"""

    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp(prefix="rh-review-", dir=os.environ.get("TMPDIR")))
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.tool = self.tmp / "tool"
        shutil.copytree(TOOL_SRC, self.tool, ignore=shutil.ignore_patterns("__pycache__"))
        for f in (self.tool / "profiles").iterdir():
            f.unlink()
        self.repo = self.tmp / "repo"
        self.repo.mkdir()
        git(self.repo, "init", "-q", "-b", "main")
        self.write("README.md", "# r\n")
        self.write("scripts/dev/repo-health/profiles/default.toml", CHILD)
        self.write("scripts/dev/repo-health/profiles/team.toml", PARENT)
        self.base = self.commit("base")

    def write(self, rel: str, text: str) -> None:
        path = self.repo / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")

    def commit(self, msg: str) -> str:
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-q", "-m", msg)
        return git(self.repo, "rev-parse", "HEAD")

    def cli(self, *args: str) -> subprocess.CompletedProcess:
        env = {k: v for k, v in os.environ.items() if k != "CELERIS_WU_BASE"}
        return subprocess.run([sys.executable, "-I", str(self.tool / "repo_health.py"), "--print-review-policy",
                               "--repo", str(self.repo), *args], capture_output=True, text=True, env=env)

    def policy(self, *args: str) -> dict:
        proc = self.cli(*args)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        data = json.loads(proc.stdout)
        self.assertEqual(schema_violations(data), [])
        return data

    def test_health_review_policy_reads_base_side(self):
        # head（commit）と作業ツリーが [review] を変えても、出力は --base の commit の内容
        self.write("scripts/dev/repo-health/profiles/team.toml", PARENT.replace('"store"', '"renamed"'))
        self.commit("head changes review")
        self.write("scripts/dev/repo-health/profiles/team.toml", 'version = "1"\n')
        got = self.policy("--base", self.base)
        self.assertEqual(got["schema"], "celeris.repo-health.review-policy/1")
        self.assertEqual(got["source"], "base")
        self.assertEqual(got["owner_map"], "OWNERS.md")
        self.assertEqual(got["high_risk"], [{"class": "store", "paths": ["^src/store/"]},
                                            {"class": "events", "paths": ["^src/events\\.rs$"]}])
        self.assertEqual(got["invariants"], {"events-append-only": "RULES.md"})
        self.assertEqual(self.policy("--base", "HEAD")["high_risk"][0]["class"], "renamed")

    def test_health_review_policy_definition_sha256_covers_child_and_parent(self):
        got = self.policy("--base", self.base)
        h = hashlib.sha256()
        for rel in ("scripts/dev/repo-health/profiles/default.toml", "scripts/dev/repo-health/profiles/team.toml"):
            raw = subprocess.run(["git", "-C", str(self.repo), "show", f"{self.base}:{rel}"], capture_output=True,
                                 check=True).stdout
            h.update(rel.encode() + b"\0" + str(len(raw)).encode() + b"\0" + raw)
        self.assertEqual(got["definition_sha256"], h.hexdigest())
        # 親の内容が変われば変わる。同じ base からは同じ値（byte 単位で同じ出力）
        self.assertEqual(self.cli("--base", self.base).stdout, self.cli("--base", self.base).stdout)
        self.write("scripts/dev/repo-health/profiles/team.toml", PARENT + "# comment\n")
        self.assertNotEqual(self.policy("--base", self.commit("parent"))["definition_sha256"],
                            got["definition_sha256"])

    def test_health_review_policy_child_subkeys_win(self):
        self.write("scripts/dev/repo-health/profiles/default.toml",
                   CHILD + '[[review.high_risk]]\nclass = "only"\npaths = ["^x/"]\n')
        got = self.policy("--base", self.commit("child overrides high_risk"))
        self.assertEqual(got["high_risk"], [{"class": "only", "paths": ["^x/"]}])
        self.assertEqual(got["owner_map"], "OWNERS.md")
        self.assertEqual(got["invariants"], {"events-append-only": "RULES.md"})

    def test_health_review_policy_absent_without_review(self):
        self.write("scripts/dev/repo-health/profiles/team.toml", 'version = "1"\n')
        self.write("scripts/dev/repo-health/profiles/default.toml", 'version = "1"\nextends = "team"\n')
        got = self.policy("--base", self.commit("no review"))
        self.assertEqual((got["source"], got["owner_map"], got["high_risk"], got["invariants"]),
                         ("absent", None, [], {}))
        self.assertRegex(got["definition_sha256"], "^[0-9a-f]{64}$")

    def test_health_review_policy_base_without_profile_is_absent_or_builtin(self):
        # base に profile が無い: head にあっても読まない。--profile 無しは組み込み generic（[review] 無し）
        empty = self.tmp / "empty"
        empty.mkdir()
        git(empty, "init", "-q", "-b", "main")
        (empty / "README.md").write_text("# e\n", encoding="utf-8")
        git(empty, "add", "-A")
        git(empty, "commit", "-q", "-m", "base")
        base = git(empty, "rev-parse", "HEAD")
        prof = empty / "scripts/dev/repo-health/profiles"
        prof.mkdir(parents=True)
        (prof / "default.toml").write_text(CHILD.replace('extends = "team"\n', "") +
                                           'owner_map = "HEAD.md"\n', encoding="utf-8")
        self.repo = empty
        got = self.policy("--base", base)
        self.assertEqual((got["source"], got["owner_map"]), ("absent", None))
        # 名前の profile が base に無ければ検査器自身の profiles/ の内容（builtin）
        (self.tool / "profiles" / "team.toml").write_text(PARENT, encoding="utf-8")
        got = self.policy("--base", base, "--profile", "team")
        self.assertEqual((got["source"], got["owner_map"]), ("builtin", "OWNERS.md"))

    def test_health_review_policy_unreadable_exits_2(self):
        self.assertEqual(self.cli("--base", "no-such-rev").returncode, 2)
        self.assertEqual(self.cli("--base", self.base, "--profile", "missing").returncode, 2)
        self.write("scripts/dev/repo-health/profiles/team.toml", PARENT + "[review.extra]\n")
        bad = self.cli("--base", self.commit("bad review"))
        self.assertEqual(bad.returncode, 2)
        self.assertIn("profile_invalid", bad.stderr)
        self.assertEqual(bad.stdout, "")

    def test_health_review_policy_rejects_head_and_report_args(self):
        for extra in (["--head", "worktree"], ["--out", str(self.tmp / "r.json")], ["--policy-version", "1"],
                      ["--rules", "policy.changed"], ["--print-base"]):
            proc = self.cli("--base", self.base, *extra)
            self.assertEqual(proc.returncode, 2, extra)
            self.assertEqual(proc.stdout, "")
        self.assertEqual(self.cli().returncode, 2)  # --base が無い

    def test_health_review_policy_schema_rejects_bad_shapes(self):
        good = self.policy("--base", self.base)
        for bad in ({**good, "extra": 1}, {**good, "source": "head"}, {**good, "definition_sha256": "ab"},
                    {**good, "high_risk": [{"class": "x", "paths": []}]}, {**good, "invariants": {"x": ""}},
                    {k: v for k, v in good.items() if k != "owner_map"}):
            self.assertNotEqual(schema_violations(bad), [], bad)

    def test_health_review_policy_on_this_repo_base(self):
        proc = subprocess.run([sys.executable, "-I", str(TOOL_SRC / "repo_health.py"), "--print-review-policy",
                               "--repo", str(ROOT), "--base", "HEAD"], capture_output=True, text=True)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        got = json.loads(proc.stdout)
        self.assertEqual(schema_violations(got), [])
        self.assertEqual(got["source"], "base")
        self.assertLessEqual(REQUIRED_CLASSES, {c["class"] for c in got["high_risk"]})


def _report(rules: list[tuple[str, str]], findings: list[tuple[str, str]], files: list[str],
            changed: bool = True) -> dict:
    statuses = [s for _, s in rules]
    verdict = "fail" if "fail" in statuses else "pass"
    return {
        "summary": {"verdict": verdict},
        "rules": [{"id": rid, "required": True, "status": st} for rid, st in rules],
        "findings": [{"rule_id": rid, "severity": sev, "delta": "new", "exception": None} for rid, sev in findings],
        "policy": {"changed": changed, "files": [{"path": p} for p in files]},
    }


class HealthReviewPolicyOnlyTests(unittest.TestCase):
    """check-health.sh の D6.2 の判定（engine.policy_review_files）と、check-health.sh の exit。"""

    POLICY = ["scripts/dev/check-bad.sh", "scripts/dev/repo-health/profiles/default.toml"]

    def test_health_review_policy_only_decision(self):
        only = _report([("doc.links", "pass"), ("policy.changed", "fail")], [("policy.changed", "error")],
                       self.POLICY)
        self.assertEqual(engine.policy_review_files(only, ["scripts/dev/check-bad.sh"]), ["scripts/dev/check-bad.sh"])
        # policy の file 以外の変更が混ざる
        self.assertIsNone(engine.policy_review_files(only, ["scripts/dev/check-bad.sh", "app.py"]))
        self.assertIsNone(engine.policy_review_files(only, []))
        # ほかの必須 rule も fail・ほかの rule の fail 級 finding がある（任意の rule でも）
        both = _report([("doc.links", "fail"), ("policy.changed", "fail")],
                       [("doc.links", "error"), ("policy.changed", "error")], self.POLICY)
        self.assertIsNone(engine.policy_review_files(both, ["scripts/dev/check-bad.sh"]))
        optional = _report([("policy.changed", "fail")], [("dup.state", "error"), ("policy.changed", "error")],
                           self.POLICY)
        self.assertIsNone(engine.policy_review_files(optional, ["scripts/dev/check-bad.sh"]))
        # warning の finding は fail 級でない。verdict が fail でなければ対象外
        warn = _report([("policy.changed", "fail")], [("dup.state", "warning"), ("policy.changed", "error")],
                       self.POLICY)
        self.assertEqual(engine.policy_review_files(warn, ["scripts/dev/check-bad.sh"]), ["scripts/dev/check-bad.sh"])
        passed = _report([("policy.changed", "pass")], [], self.POLICY, changed=False)
        self.assertIsNone(engine.policy_review_files(passed, ["scripts/dev/check-bad.sh"]))

    def test_health_review_policy_only_branch_exits_zero(self):
        tmp = Path(tempfile.mkdtemp(prefix="rh-policy-only-", dir=os.environ.get("TMPDIR")))
        self.addCleanup(shutil.rmtree, tmp, True)
        repo = tmp / "repo"
        (repo / "scripts/dev").mkdir(parents=True)
        shutil.copy2(ROOT / "scripts/dev/check-health.sh", repo / "scripts/dev/")
        shutil.copytree(TOOL_SRC, repo / "scripts/dev/repo-health", ignore=shutil.ignore_patterns("__pycache__"))
        for name in ("source-size-report.py", "source-size-report.toml"):
            shutil.copy2(ROOT / "scripts/dev" / name, repo / "scripts/dev/")
        (repo / "scripts/dev/check-bad.sh").write_text('#!/bin/sh\n[ ! -e "$1/BAD" ]\n', encoding="utf-8")
        (repo / "scripts/dev/repo-health/profiles/default.toml").write_text(textwrap.dedent("""\
            version = "1"
            policy_paths = ['^scripts/dev/repo-health/', '^scripts/dev/check-(health|bad)\\.sh$']
            exclude_paths = ['^scripts/']
            [rules."doc.links"]
            required = true
            commands = [["sh", "scripts/dev/check-bad.sh", "{root}"]]
            """), encoding="utf-8")
        (repo / "app.py").write_text('print("x")\n', encoding="utf-8")
        git(repo, "init", "-q", "-b", "main")
        git(repo, "add", "-A")
        git(repo, "commit", "-q", "-m", "base")
        git(repo, "switch", "-q", "-c", "task")
        with open(repo / "scripts/dev/check-bad.sh", "a", encoding="utf-8") as fh:
            fh.write("# 注記\n")
        git(repo, "commit", "-qam", "policy only")
        out = tmp / "report.json"
        env = {k: v for k, v in os.environ.items() if k != "CELERIS_WU_BASE"}

        def check() -> subprocess.CompletedProcess:
            return subprocess.run(["bash", str(repo / "scripts/dev/check-health.sh"), "--out", str(out)],
                                  capture_output=True, text=True, env=env)

        proc = check()
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.assertIn("policy review required: scripts/dev/check-bad.sh\n", proc.stdout)
        report = json.loads(out.read_text(encoding="utf-8"))
        self.assertEqual(report["summary"]["verdict"], "fail")
        self.assertTrue(report["policy"]["changed"])
        (repo / "app.py").write_text('print("y")\n', encoding="utf-8")
        mixed = check()
        self.assertEqual(mixed.returncode, 1, mixed.stdout + mixed.stderr)
        self.assertNotIn("policy review required", mixed.stdout)


if __name__ == "__main__":
    unittest.main()
