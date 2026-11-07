"""repo health の中核（repo_health.py）: 一時 dir の小さな git repo で契約の各経路を確かめる。

検査器の写しを一時 dir に置き、試験用の rule（rules/zz_test_*.py）を足して使う。試験用の rule は
写しにだけあり、本物の rules/ には入らない。
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TOOL_SRC = ROOT / "scripts/dev/repo-health"
VALIDATOR = TOOL_SRC / "validate_report.py"
FAKE_TOOL = "rh-fake-tool"

# source-size: python の file の TODO の数が閾値（profile の max_todo、既定 1）を超えたら finding。
# behave = crash / unknown で失敗の経路を作る。deps.cycle: 外部 tool rh-fake-tool を要る rule。
TEST_RULES = '''
from health.rule import Hit, Rule, Unknown


def todo(ctx, tree):
    behave = ctx.config.get("behave", "count")
    if behave == "crash":
        raise RuntimeError("boom")
    if behave == "unknown":
        raise Unknown("baseline_unreadable: test")
    limit = ctx.config.get("max_todo", 1)
    hits = []
    for info in ctx.target_files(tree, languages=("python",), classes=("product",)):
        n = (tree.read_text(info.path) or "").count("TODO")
        if n > limit:
            hits.append(Hit(evidence=[{"path": info.path, "line": 1, "ref": tree.side}],
                            scope=("file", info.path), rationale=f"TODO {n} > {limit}", value=n, threshold=limit))
    return hits


def tool_rule(ctx, tree):
    proc = ctx.run(["rh-fake-tool"], cwd=tree.checkout_dir())
    return [] if proc.returncode == 0 else [Hit(evidence=[{"path": "x"}], scope=("repo", "x"), rationale="tool")]


def unlinked(ctx, tree):
    return [Hit(evidence=[{"path": p}], scope=("file", p), rationale="候補", severity="error")
            for p in tree.files() if p.endswith("orphan.py")]


RULES = [
    Rule(id="source-size", kind="structure", evaluate=todo, languages=("python",)),
    Rule(id="deps.cycle", kind="structure", evaluate=tool_rule, tools=("rh-fake-tool",)),
    Rule(id="link.unlinked", kind="structure", evaluate=unlinked, candidate=True, default_required=False),
    Rule(id="doc.links", kind="docs", evaluate=lambda ctx, tree: [], adapter="docs"),
]
'''


def git(cwd: Path, *args: str) -> str:
    env = {**os.environ, "GIT_AUTHOR_DATE": "2026-01-01T00:00:00Z", "GIT_COMMITTER_DATE": "2026-01-01T00:00:00Z"}
    return subprocess.run(["git", "-C", str(cwd), *args], check=True, capture_output=True, text=True,
                          env=env).stdout.strip()


class RepoHealthCoreTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp(prefix="rh-core-", dir=os.environ.get("TMPDIR")))
        self.tool = self.tmp / "tool"
        shutil.copytree(TOOL_SRC, self.tool, ignore=shutil.ignore_patterns("__pycache__"))
        # 中核の試験は試験用の rule で回す。本物の rule（rules-* WorkUnit）は同じ ID を二重登録にするので外す
        for real in (self.tool / "rules").glob("*.py"):
            if real.name != "policy_changed.py":
                real.unlink()
        (self.tool / "rules" / "zz_test_rules.py").write_text(TEST_RULES, encoding="utf-8")
        self.bin = self.tmp / "bin"
        self.bin.mkdir()
        fake = self.bin / FAKE_TOOL
        fake.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        fake.chmod(0o755)
        self.repo = self.tmp / "repo"
        self.repo.mkdir()
        git(self.repo, "init", "-q", "-b", "main")
        git(self.repo, "config", "user.email", "t@example.com")
        git(self.repo, "config", "user.name", "t")
        git(self.repo, "config", "commit.gpgsign", "false")
        self.write("src/app.py", "print('a')\n")
        self.write("README.md", "# r\n")
        self.base = self.commit("base")

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    # ------------------------------------------------------------ helpers
    def write(self, rel: str, text: str, repo: Path | None = None) -> None:
        path = (repo or self.repo) / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")

    def commit(self, msg: str, repo: Path | None = None) -> str:
        r = repo or self.repo
        git(r, "add", "-A")
        git(r, "commit", "-q", "--allow-empty", "-m", msg)
        return git(r, "rev-parse", "HEAD")

    def profile(self, body: str) -> None:
        self.write("scripts/dev/repo-health/profiles/default.toml", textwrap.dedent(body))

    def run_tool(self, *extra: str, base: str | None = None, head: str = "HEAD", repo: Path | None = None,
                 path_env: str | None = None, version: str = "1", out: str = "report.json"):
        out_path = self.tmp / out
        if out_path.exists():
            out_path.unlink()
        env = {**os.environ, "PATH": path_env if path_env is not None else f"{self.bin}:{os.environ['PATH']}"}
        cmd = [sys.executable, "-I", str(self.tool / "repo_health.py"), "--repo", str(repo or self.repo),
               "--base", base or self.base, "--head", head, "--policy-version", version, "--out", str(out_path),
               *extra]
        proc = subprocess.run(cmd, capture_output=True, text=True, env=env)
        report = json.loads(out_path.read_text(encoding="utf-8")) if out_path.exists() else None
        return proc, report, out_path

    def assert_valid(self, path: Path) -> None:
        proc = subprocess.run([sys.executable, "-I", str(VALIDATOR), str(path)], capture_output=True, text=True)
        self.assertEqual(proc.returncode, 0, proc.stdout)

    def rule(self, report: dict, rid: str) -> dict:
        return next(r for r in report["rules"] if r["id"] == rid)

    # ------------------------------------------------------------ tests
    def test_clean_commit_pass_is_deterministic(self):
        self.write("src/app.py", "print('b')\n")
        self.commit("head")
        p1, r1, out1 = self.run_tool(out="a.json")
        self.assertEqual(p1.returncode, 0, p1.stderr + p1.stdout)
        self.assert_valid(out1)
        p2, _, out2 = self.run_tool(out="b.json")
        self.assertEqual(p2.returncode, 0)
        self.assertEqual(out1.read_bytes(), out2.read_bytes(), "同じ入力で byte 単位で同じ report")
        self.assertEqual(r1["inputs"]["mode"], "full")
        self.assertEqual(r1["policy"]["source"], "builtin")
        self.assertEqual(r1["inputs"]["profile"], {"requested": None, "resolved": "generic", "source": "builtin"})
        self.assertTrue(r1["summary"]["reviewable"])
        self.assertEqual(r1["snapshot"]["base"]["sha"], self.base)
        self.assertEqual(self.rule(r1, "source-size")["status"], "pass")
        self.assertEqual(r1["metrics"]["diff"], {"changed_files": 1, "changed_lines": 1})

    def test_dirty_worktree_is_not_reviewable(self):
        self.write("src/app.py", "print('dirty')\n")
        self.write("src/new.py", "x = 1\ny = 2\n")  # 未追跡も diff に入る
        proc, rep, out = self.run_tool(head="worktree")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assert_valid(out)
        self.assertEqual(rep["snapshot"]["head"]["kind"], "worktree")
        self.assertTrue(rep["snapshot"]["head"]["dirty"])
        self.assertFalse(rep["summary"]["reviewable"])
        self.assertEqual(rep["metrics"]["diff"], {"changed_files": 2, "changed_lines": 3})
        # 変更の無い作業ツリーは dirty でない
        git(self.repo, "checkout", "-q", "--", ".")
        (self.repo / "src/new.py").unlink()
        _, rep2, _ = self.run_tool(head="worktree")
        self.assertFalse(rep2["snapshot"]["head"]["dirty"])

    def test_shallow_clone_base_unreachable_is_error(self):
        self.write("src/app.py", "print('c')\n")
        self.commit("head")
        clone = self.tmp / "clone"
        subprocess.run(["git", "clone", "-q", "--depth", "1", f"file://{self.repo}", str(clone)], check=True,
                       capture_output=True)
        proc, rep, out = self.run_tool(repo=clone)
        self.assertEqual(proc.returncode, 2, proc.stderr)
        self.assert_valid(out)
        self.assertTrue(rep["snapshot"]["shallow"])
        self.assertIsNone(rep["snapshot"]["base"]["sha"])
        self.assertFalse(rep["snapshot"]["base"]["reachable"])
        self.assertEqual(rep["policy"]["source"], "unavailable")
        self.assertEqual(rep["summary"]["verdict"], "error")
        self.assertTrue(all(r["status"] == "unsupported" and r["reason"] == "base_unreachable"
                            for r in rep["rules"]))

    def test_doc_only_skips_structure_rules(self):
        self.write("README.md", "# r\n\nmore\n")
        self.write("manual/guide.md", "# g\n")
        self.commit("docs")
        proc, rep, out = self.run_tool(path_env=f"{self.git_only_bin()}")  # doc-only は外部 tool を要らない
        self.assertEqual(proc.returncode, 0, proc.stderr + proc.stdout)
        self.assert_valid(out)
        self.assertEqual(rep["inputs"]["mode"], "doc-only")
        for rid in ("source-size", "deps.cycle", "link.unlinked"):
            self.assertEqual(self.rule(rep, rid)["status"], "not_applicable", rid)
        self.assertEqual(self.rule(rep, "doc.links")["status"], "pass")
        self.assertEqual(self.rule(rep, "policy.changed")["status"], "pass")

    def test_policy_weakening_is_evaluated_on_base_and_fails(self):
        self.profile('''
            version = "1"
            [rules."source-size"]
            max_todo = 1
        ''')
        self.write("src/app.py", "# TODO TODO\n")  # 既存負債（2 > 1）
        self.base = self.commit("policy at base")
        # 候補は TODO を増やし、閾値を上げ、さらに rule を無効にして自分を合格にしようとする
        self.write("src/app.py", "# TODO TODO TODO\n")
        self.profile('''
            version = "1"
            [rules."source-size"]
            enabled = false
            max_todo = 10
        ''')
        self.commit("weaken")
        proc, rep, out = self.run_tool()
        self.assertEqual(proc.returncode, 1, proc.stdout + proc.stderr)
        self.assert_valid(out)
        self.assertEqual(rep["policy"]["source"], "base")
        self.assertTrue(rep["policy"]["changed"])
        pol = self.rule(rep, "policy.changed")
        self.assertEqual(pol["status"], "fail")
        size = self.rule(rep, "source-size")
        self.assertEqual(size["status"], "fail", "base 側の閾値 1 と enabled（既定）で評価する")
        f = next(x for x in rep["findings"] if x["rule_id"] == "source-size")
        self.assertEqual(f["delta"], "worsened")
        self.assertEqual(f["baseline"], {"value": 2, "head_value": 3, "threshold": 1})
        pf = next(x for x in rep["findings"] if x["rule_id"] == "policy.changed")
        self.assertEqual(pf["severity"], "error")
        self.assertEqual(pf["evidence"][0]["path"], "scripts/dev/repo-health/profiles/default.toml")

    def test_bootstrap_profile_is_warning_only(self):
        self.profile('''
            version = "1"
        ''')
        self.commit("add profile")
        proc, rep, out = self.run_tool()
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.assert_valid(out)
        self.assertEqual(rep["policy"]["source"], "bootstrap")
        self.assertEqual(rep["inputs"]["profile"]["source"], "repo")
        pf = next(x for x in rep["findings"] if x["rule_id"] == "policy.changed")
        self.assertEqual(pf["severity"], "warning")

    def git_only_bin(self) -> Path:
        d = self.tmp / "gitbin"
        if not d.exists():
            d.mkdir()
            (d / "git").symlink_to(shutil.which("git"))
        return d

    def test_missing_tool_is_unsupported_not_pass(self):
        self.write("src/app.py", "print('t')\n")
        self.commit("head")
        proc, rep, out = self.run_tool(path_env=str(self.git_only_bin()))
        self.assertEqual(proc.returncode, 2, proc.stdout + proc.stderr)
        self.assert_valid(out)
        r = self.rule(rep, "deps.cycle")
        self.assertEqual((r["status"], r["reason"]), ("unsupported", f"tool_missing:{FAKE_TOOL}"))
        self.assertEqual(rep["summary"]["verdict"], "error")
        # tool があれば同じ rule は pass
        proc2, rep2, _ = self.run_tool()
        self.assertEqual(proc2.returncode, 0, proc2.stdout)
        self.assertEqual(self.rule(rep2, "deps.cycle")["status"], "pass")

    def test_missing_git_writes_no_report(self):
        empty = self.tmp / "empty"
        empty.mkdir()
        proc, rep, _ = self.run_tool(path_env=str(empty))
        self.assertEqual(proc.returncode, 2)
        self.assertIsNone(rep)
        self.assertIn("tool_missing:git", proc.stderr)

    def test_non_git_and_version_mismatch_write_no_report(self):
        plain = self.tmp / "plain"
        plain.mkdir()
        proc, rep, _ = self.run_tool(repo=plain)
        self.assertEqual(proc.returncode, 2)
        self.assertIsNone(rep)
        self.assertIn("not_a_git_repo", proc.stderr)
        proc, rep, _ = self.run_tool(version="2")
        self.assertEqual(proc.returncode, 2)
        self.assertIsNone(rep)
        self.assertIn("policy_version_mismatch", proc.stderr)
        proc, rep, _ = self.run_tool(base="no-such-ref")
        self.assertEqual(proc.returncode, 2)
        self.assertIn("base_not_found", proc.stderr)

    def test_existing_new_and_exception(self):
        self.profile('''
            version = "1"
            [[rules."source-size".exceptions]]
            path = '^src/legacy\\.py$'
            reason = "試験: 既知の例外"
        ''')
        self.write("src/old.py", "# TODO TODO\n")
        self.base = self.commit("debt")
        self.write("src/legacy.py", "# TODO TODO\n")
        self.write("src/README.md", "x\n")
        self.commit("head")
        proc, rep, out = self.run_tool()
        self.assertEqual(proc.returncode, 0, proc.stdout)
        self.assert_valid(out)
        by_path = {f["evidence"][0]["path"]: f for f in rep["findings"] if f["rule_id"] == "source-size"}
        self.assertEqual(by_path["src/old.py"]["delta"], "existing")
        self.assertEqual(by_path["src/legacy.py"]["delta"], "new")
        exc = by_path["src/legacy.py"]["exception"]
        self.assertEqual(exc["reason"], "試験: 既知の例外")
        self.assertEqual(exc["source"]["path"], "scripts/dev/repo-health/profiles/default.toml")
        self.assertEqual(exc["source"]["line"], 3)
        self.assertEqual(rep["summary"]["counts"]["fail_grade"], 0)

    def test_candidate_finding_never_blocks(self):
        self.write("src/orphan.py", "x = 1\n")
        self.commit("head")
        proc, rep, out = self.run_tool()
        self.assertEqual(proc.returncode, 0, proc.stdout)
        self.assert_valid(out)
        f = next(x for x in rep["findings"] if x["rule_id"] == "link.unlinked")
        self.assertTrue(f["candidate"])
        self.assertEqual(f["severity"], "warning", "候補の severity は error から warning に下げる")

    def test_adapterless_language_is_unsupported(self):
        self.write("cmd/main.go", "package main\n")
        self.commit("go")
        proc, rep, out = self.run_tool()
        self.assertEqual(proc.returncode, 2, proc.stdout)
        self.assert_valid(out)
        r = self.rule(rep, "source-size")
        self.assertEqual((r["status"], r["reason"]), ("unsupported", "adapter_missing:go"))
        self.assertEqual(self.rule(rep, "doc.links")["status"], "pass", "文書 rule は adapter に依らない")

    def test_rule_crash_and_unknown_are_not_pass(self):
        for behave, status, prefix in (("crash", "error", "crashed:source-size:RuntimeError"),
                                       ("unknown", "unknown", "baseline_unreadable")):
            with self.subTest(behave=behave):
                self.profile(f'''
                    version = "1"
                    [rules."source-size"]
                    behave = "{behave}"
                ''')
                self.base = self.commit(f"profile {behave}")
                self.write("src/app.py", f"print('{behave}')\n")
                self.commit("head")
                proc, rep, out = self.run_tool()
                self.assertEqual(proc.returncode, 2, proc.stdout)
                self.assert_valid(out)
                r = self.rule(rep, "source-size")
                self.assertEqual(r["status"], status)
                self.assertTrue(r["reason"].startswith(prefix), r["reason"])

    def test_profile_rule_without_implementation_is_unsupported(self):
        self.profile('''
            version = "1"
            [rules."deps.violation"]
            layers = ["a", "b"]
        ''')
        self.base = self.commit("profile")
        self.write("src/app.py", "print('z')\n")
        self.commit("head")
        proc, rep, out = self.run_tool()
        self.assertEqual(proc.returncode, 2, proc.stdout)
        self.assert_valid(out)
        r = self.rule(rep, "deps.violation")
        self.assertEqual(r["status"], "unsupported")
        self.assertTrue(r["reason"].startswith("rule_missing:deps.violation"))

    def test_unknown_profile_key_is_rejected(self):
        self.profile('''
            version = "1"
            max_lines = 10
        ''')
        self.base = self.commit("bad profile")
        proc, rep, _ = self.run_tool()
        self.assertEqual(proc.returncode, 2)
        self.assertIsNone(rep)
        self.assertIn("profile_invalid", proc.stderr)

    def test_rules_filter_keeps_policy_changed(self):
        self.commit("head")
        proc, rep, out = self.run_tool("--rules", "doc.links")
        self.assertEqual(proc.returncode, 0, proc.stdout)
        self.assert_valid(out)
        self.assertEqual([r["id"] for r in rep["rules"]], ["doc.links", "policy.changed"])
        proc, _, _ = self.run_tool("--rules", "nope.rule")
        self.assertEqual(proc.returncode, 2)

    def test_submodule_is_recorded_not_entered(self):
        sub = self.tmp / "sub"
        sub.mkdir()
        git(sub, "init", "-q", "-b", "main")
        git(sub, "config", "user.email", "t@example.com")
        git(sub, "config", "user.name", "t")
        self.write("lib.go", "package lib\n", repo=sub)  # adapter の無い言語でも中には入らない
        sub_sha = self.commit("sub", repo=sub)
        git(self.repo, "-c", "protocol.file.allow=always", "submodule", "add", "-q", f"file://{sub}", "vendor/sub")
        self.commit("add submodule")
        proc, rep, out = self.run_tool()
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.assert_valid(out)
        self.assertEqual(rep["snapshot"]["submodules"],
                         [{"path": "vendor/sub", "base_sha": None, "head_sha": sub_sha}])
        self.assertEqual(rep["metrics"]["diff"]["changed_files"], 1, ".gitmodules だけ（submodule の中は数えない）")


class AdapterTests(unittest.TestCase):
    def test_classification(self):
        sys.path.insert(0, str(TOOL_SRC))
        try:
            from health.adapters import Classifier
        finally:
            sys.path.pop(0)
        c = Classifier({"test": [r"^qa/"]})
        cases = {
            "crates/a/src/lib.rs": ("rust", "product"),
            "crates/a/tests/it.rs": ("rust", "test"),
            "scripts/tests/test_x.py": ("python", "test"),
            "web/src/a.test.ts": ("typescript", "test"),
            "web/src/a.tsx": ("typescript", "product"),
            "tool/run.sh": ("shell", "product"),
            "manual/x.md": ("markdown", "docs"),
            "doc/notes.txt": ("data", "docs"),
            "crates/a/migrations/0001.sql": ("data", "migration"),
            "scripts/tests/fixtures/a.py": ("python", "fixture"),
            "qa/check.py": ("python", "test"),
            "cmd/main.go": ("go", "product"),
        }
        for path, want in cases.items():
            info = c.classify(path, b"")
            self.assertEqual((info.language, info.cls), want, path)
        self.assertEqual(c.classify("bin/tool", b"#!/usr/bin/env python3\n").language, "python")
        self.assertEqual(c.classify("bin/run", b"#!/bin/sh\n").language, "shell")
        self.assertEqual(c.classify("src/api.ts", b"// @generated by x\n").cls, "generated")
        self.assertFalse(c.classify("manual/x.md", b"").is_code)


if __name__ == "__main__":
    unittest.main()
