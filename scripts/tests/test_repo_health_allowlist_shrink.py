"""repo health: 許可の一覧から行を消すだけの変更を policy.changed に数えない（ADR 2026-11-16-health-policy-allowlist-shrink）。

守る不変条件: policy.changed から外れるのは、base 側の profile の policy_allowlists に当たる file で、base と head の
両方にあり、head の内容の行（空行と列 0 の # の行を除く）が base の内容の行の集合に含まれる変更だけ。行を足す・
書き換える・file を消す・許可の一覧でない policy の file の変更・head の profile で許可の一覧を足す変更は数える。
既存の試験（test_repo_health_core.py の policy の弱め）は blob の違いだけを見ていて、この判定の有無を区別できない。

一時 dir の git repo を作り、本物の検査器を --rules policy.changed で走らせる（rule は policy.changed だけを評価する）。
report は毎回 validate_report.py に通す（S8: report の policy.changed と rule の finding が一致する）。
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TOOL = ROOT / "scripts/dev/repo-health/repo_health.py"
VALIDATOR = ROOT / "scripts/dev/repo-health/validate_report.py"
PROFILE = "scripts/dev/repo-health/profiles/default.toml"
ALLOW = "lists/allow.txt"
OTHER = "lists/other.txt"

BASE_PROFILE = """version = "1"
description = "allowlist-shrink fixture"
policy_paths = ['^scripts/dev/repo-health/', '^lists/']
policy_allowlists = ['^lists/allow\\.txt$']
"""
BASE_ALLOW = "# 許可の一覧（rule<TAB>snippet<TAB>理由）\nalpha\tA\tr1\nbeta\tB\tr2\n\ngamma\tG\tr3\n"
BASE_OTHER = "one\ntwo\nthree\n"


def git(repo: Path, *args: str) -> str:
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    env.update(GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1",
               GIT_AUTHOR_DATE="2026-01-01T00:00:00Z", GIT_COMMITTER_DATE="2026-01-01T00:00:00Z")
    return subprocess.run(["git", "-C", str(repo), "-c", "user.name=t", "-c", "user.email=t@example.com",
                           "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", *args],
                          env=env, check=True, capture_output=True, text=True).stdout.strip()


class AllowlistShrinkTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp(prefix="rh-allow-", dir=os.environ.get("TMPDIR")))
        self.repo = self.tmp / "repo"
        self.repo.mkdir()
        git(self.repo, "init", "-q", "-b", "main")
        self.write(PROFILE, BASE_PROFILE)
        self.write(ALLOW, BASE_ALLOW)
        self.write(OTHER, BASE_OTHER)
        self.write("README.md", "# r\n")
        self.base = self.commit("base")

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def write(self, rel: str, text: str | bytes) -> None:
        path = self.repo / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        if isinstance(text, bytes):
            path.write_bytes(text)
        else:
            path.write_text(text, encoding="utf-8")

    def commit(self, msg: str) -> str:
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-q", "--allow-empty", "-m", msg)
        return git(self.repo, "rev-parse", "HEAD")

    def report(self) -> tuple[int, dict]:
        """head を commit し、--rules policy.changed で report を作って検証器に通す。(exit, report)"""
        self.commit("head")
        out = self.tmp / "report.json"
        proc = subprocess.run([sys.executable, "-I", str(TOOL), "--repo", str(self.repo), "--base", self.base,
                               "--head", "HEAD", "--policy-version", "1", "--rules", "policy.changed",
                               "--out", str(out)], capture_output=True, text=True)
        self.assertTrue(out.exists(), proc.stdout + proc.stderr)
        check = subprocess.run([sys.executable, "-I", str(VALIDATOR), str(out)], capture_output=True, text=True)
        self.assertEqual(check.returncode, 0, check.stdout + check.stderr)
        rep = json.loads(out.read_text(encoding="utf-8"))
        self.assertEqual(rep["policy"]["source"], "base")
        return proc.returncode, rep

    def assert_counted(self, rep: dict, rc: int, paths: list[str]) -> None:
        self.assertEqual(rc, 1)
        self.assertTrue(rep["policy"]["changed"])
        rule = next(r for r in rep["rules"] if r["id"] == "policy.changed")
        self.assertEqual(rule["status"], "fail")
        found = [f for f in rep["findings"] if f["rule_id"] == "policy.changed"]
        self.assertEqual(len(found), 1)
        self.assertEqual(found[0]["severity"], "error")
        self.assertEqual(sorted(e["path"] for e in found[0]["evidence"]), sorted(paths))

    def assert_not_counted(self, rep: dict, rc: int) -> None:
        self.assertEqual(rc, 0)
        self.assertFalse(rep["policy"]["changed"])
        rule = next(r for r in rep["rules"] if r["id"] == "policy.changed")
        self.assertEqual(rule["status"], "pass")
        self.assertEqual([f for f in rep["findings"] if f["rule_id"] == "policy.changed"], [])

    # ------------------------------------------------------------ 数えない
    def test_allowlist_shrink_removing_lines_is_not_policy_change(self):
        self.write(ALLOW, "# 許可の一覧（rule<TAB>snippet<TAB>理由）\nalpha\tA\tr1\n\ngamma\tG\tr3\n")
        rc, rep = self.report()
        self.assert_not_counted(rep, rc)
        # policy.files の形は変えない: 数えなかった file も base と head の blob が違う行として残る
        entry = next(f for f in rep["policy"]["files"] if f["path"] == ALLOW)
        self.assertEqual(set(entry), {"path", "base_blob", "head_blob"})
        self.assertNotEqual(entry["base_blob"], entry["head_blob"])

    def test_allowlist_shrink_comment_and_blank_lines_are_not_content(self):
        self.write(ALLOW, "# 書き換えた注釈\n\n\nalpha\tA\tr1\nbeta\tB\tr2\ngamma\tG\tr3")
        rc, rep = self.report()
        self.assert_not_counted(rep, rc)

    # ------------------------------------------------------------ 数える
    def test_allowlist_shrink_adding_a_line_is_policy_change(self):
        self.write(ALLOW, BASE_ALLOW + "delta\tD\tr4\n")
        rc, rep = self.report()
        self.assert_counted(rep, rc, [ALLOW])

    def test_allowlist_shrink_rewriting_a_line_is_policy_change(self):
        self.write(ALLOW, BASE_ALLOW.replace("beta\tB\tr2", "beta\tB2\tr2"))
        rc, rep = self.report()
        self.assert_counted(rep, rc, [ALLOW])

    def test_allowlist_shrink_indented_hash_line_is_content(self):
        # 列 0 でない # の行は内容の行として比べる（足せば数える）
        self.write(ALLOW, BASE_ALLOW + " # alpha\tA\tr1\n")
        rc, rep = self.report()
        self.assert_counted(rep, rc, [ALLOW])

    def test_allowlist_shrink_removing_lines_from_non_allowlist_policy_file_is_policy_change(self):
        self.write(PROFILE, BASE_PROFILE.replace('description = "allowlist-shrink fixture"\n', ""))
        rc, rep = self.report()
        self.assert_counted(rep, rc, [PROFILE])

    def test_allowlist_shrink_head_profile_cannot_add_allowlist(self):
        # head が profile の policy_allowlists に other.txt を足し、other.txt から行を消す。判定は base の profile で行う
        self.write(PROFILE, BASE_PROFILE.replace("policy_allowlists = ['^lists/allow\\.txt$']",
                                                 "policy_allowlists = ['^lists/allow\\.txt$', '^lists/other\\.txt$']"))
        self.write(OTHER, "one\nthree\n")
        rc, rep = self.report()
        self.assert_counted(rep, rc, [PROFILE, OTHER])

    def test_allowlist_shrink_deleting_the_allowlist_file_is_policy_change(self):
        (self.repo / ALLOW).unlink()
        rc, rep = self.report()
        self.assert_counted(rep, rc, [ALLOW])
        entry = next(f for f in rep["policy"]["files"] if f["path"] == ALLOW)
        self.assertIsNone(entry["head_blob"])

    def test_allowlist_shrink_non_utf8_content_is_policy_change(self):
        self.write(ALLOW, b"alpha\tA\tr1\n\xff\n")
        rc, rep = self.report()
        self.assert_counted(rep, rc, [ALLOW])

    def test_allowlist_shrink_other_line_breaks_are_policy_change(self):
        # 付記 A1: Python の text mode（universal newlines）で読む読み手は単独の CR も行の区切りにする。
        # '#note<CR>許可' は '\n' だけで分けると列 0 が # の注釈の 1 行だが、その読み手には新しい許可の行が入る。
        # str.splitlines が区切る '\n' 以外の文字を 1 つでも含む内容は比べずに数える
        line = "delta\tD\tr4"
        for ch in ["\r", "\x0b", "\x0c", "\x1c", "\x1d", "\x1e", "\x85", "\u2028", "\u2029"]:
            with self.subTest(ch=repr(ch)):
                self.write(ALLOW, BASE_ALLOW + "#note" + ch + line + "\n")
                rc, rep = self.report()
                self.assert_counted(rep, rc, [ALLOW])
        # 再現の前提: text mode で読むと CR の後ろが許可の行になる（check-human-text.py の allow_list の読み方）
        self.write(ALLOW, BASE_ALLOW + "#note\r" + line + "\n")
        with open(self.repo / ALLOW, encoding="utf-8") as f:
            self.assertIn(line + "\n", list(f))

    def test_allowlist_shrink_crlf_content_is_policy_change(self):
        # 改行が CRLF の一覧は、行を消すだけでも比べずに数える（読み手によって行末の CR の扱いが違う）
        self.write(ALLOW, BASE_ALLOW.replace("\n", "\r\n"))
        self.base = self.commit("crlf base")
        self.write(ALLOW, "alpha\tA\tr1\r\n")
        rc, rep = self.report()
        self.assert_counted(rep, rc, [ALLOW])

    def test_allowlist_shrink_profile_rejects_invalid_regex(self):
        self.write(PROFILE, BASE_PROFILE.replace("policy_allowlists = ['^lists/allow\\.txt$']",
                                                 "policy_allowlists = ['(']"))
        self.base = self.commit("bad profile at base")
        out = self.tmp / "report.json"
        proc = subprocess.run([sys.executable, "-I", str(TOOL), "--repo", str(self.repo), "--base", self.base,
                               "--head", "HEAD", "--policy-version", "1", "--rules", "policy.changed",
                               "--out", str(out)], capture_output=True, text=True)
        self.assertEqual(proc.returncode, 2, proc.stdout + proc.stderr)
        self.assertIn("profile_invalid", proc.stderr)
        self.assertIn("policy_allowlists", proc.stderr)
        self.assertFalse(out.exists())


if __name__ == "__main__":
    unittest.main()
