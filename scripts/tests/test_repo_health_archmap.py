"""repo health の rule arch-map.*（rules/arch_map.py）: 一時 dir の git repo で report を確かめる。

検査器（scripts/dev/repo-health/）と地図の検査器（scripts/dev/check-architecture-map.py）を一時 dir に
同じ相対配置で写して使う。判定は check-architecture-map.py の find_diffs が正本で、ここでは profile からの
読み込み・not_applicable/unknown・base との差分（existing/new）を確かめる。
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
CHECKER_SRC = ROOT / "scripts/dev/check-architecture-map.py"
KINDS = ("missing_row", "missing_test", "wrong_owner", "missing_invariant", "stale", "wrong_path")
IDS = [f"arch-map.{k}" for k in KINDS]
HEADER = ("| subsystem | owner crate/module | entry point | ADR / 設計 | 関連試験 | 不変条件 |\n"
          "|---|---|---|---|---|---|\n")
PROFILE = '''
    version = "1"
    [arch_map]
    path = "manual/map.md"
    {reference}
'''


def git(cwd: Path, *args: str) -> str:
    env = {**os.environ, "GIT_AUTHOR_DATE": "2026-01-01T00:00:00Z", "GIT_COMMITTER_DATE": "2026-01-01T00:00:00Z"}
    return subprocess.run(["git", "-C", str(cwd), *args], check=True, capture_output=True, text=True,
                          env=env).stdout.strip()


def row(name: str, entry: str, tests: str = "`src/store_tests.rs`", inv: str = "書き込みは transaction") -> str:
    return f"| {name} | core | `{entry}` | [ADR](../adr/0001-ok.md) | {tests} | {inv} |\n"


class RepoHealthArchMapTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp(prefix="rh-archmap-", dir=os.environ.get("TMPDIR")))
        self.tool = self.tmp / "tool/scripts/dev/repo-health"
        shutil.copytree(TOOL_SRC, self.tool, ignore=shutil.ignore_patterns("__pycache__"))
        shutil.copy(CHECKER_SRC, self.tool.parent / "check-architecture-map.py")
        self.repo = self.tmp / "repo"
        self.repo.mkdir()
        git(self.repo, "init", "-q", "-b", "main")
        git(self.repo, "config", "user.email", "t@example.com")
        git(self.repo, "config", "user.name", "t")
        git(self.repo, "config", "commit.gpgsign", "false")
        for rel in ("src/store.rs", "src/store_tests.rs", "src/api.rs", "src/api_tests.rs"):
            self.write(rel, "// x\n")
        self.write("adr/0001-ok.md", "# ok\n- 状態: 採用\n")
        self.write("cap.json", json.dumps({"capabilities": [
            {"id": "store", "owners": [{"path": "src/store.rs"}], "tests": ["src/store_tests.rs"],
             "invariants": [{"evidence_path": "adr/0001-ok.md"}]},
            {"id": "api", "owners": [{"path": "src/api.rs"}], "tests": ["src/api_tests.rs"]},
        ]}))

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def write(self, rel: str, text: str) -> None:
        path = self.repo / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")

    def commit(self, msg: str) -> str:
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-q", "--allow-empty", "-m", msg)
        return git(self.repo, "rev-parse", "HEAD")

    def profile(self, reference: bool = True) -> None:
        ref = 'reference = "cap.json"' if reference else ""
        self.write("scripts/dev/repo-health/profiles/default.toml", textwrap.dedent(PROFILE.format(reference=ref)))

    def run_tool(self, base: str, head: str = "HEAD", out: str = "report.json"):
        out_path = self.tmp / out
        cmd = [sys.executable, "-I", str(self.tool / "repo_health.py"), "--repo", str(self.repo), "--base", base,
               "--head", head, "--policy-version", "1", "--out", str(out_path)]
        proc = subprocess.run(cmd, capture_output=True, text=True)
        report = json.loads(out_path.read_text(encoding="utf-8")) if out_path.exists() else None
        if report is not None:
            check = subprocess.run([sys.executable, "-I", str(self.tool / "validate_report.py"), str(out_path)],
                                   capture_output=True, text=True)
            self.assertEqual(check.returncode, 0, check.stdout)
        return proc, report

    def rules(self, report: dict) -> dict:
        return {r["id"]: (r["status"], r["reason"]) for r in report["rules"] if r["id"].startswith("arch-map.")}

    def findings(self, report: dict, rid: str) -> list[dict]:
        return [f for f in report["findings"] if f["rule_id"] == rid]

    def test_generic_profile_without_map_is_not_applicable(self):
        base = self.commit("base")
        self.write("src/store.rs", "// y\n")
        self.commit("head")
        proc, rep = self.run_tool(base)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(rep["inputs"]["profile"]["resolved"], "generic")
        for rid in IDS:
            self.assertEqual(rep["rules"][[r["id"] for r in rep["rules"]].index(rid)]["status"], "not_applicable", rid)
            self.assertTrue(self.rules(rep)[rid][1].startswith("no_arch_map"), rid)

    def test_existing_debt_passes_and_new_diffs_fail_per_kind(self):
        self.profile()
        # base の負債: api の行が無い（missing_row）
        self.write("manual/map.md", HEADER + row("store", "src/store.rs"))
        base = self.commit("base")
        # head: 上に行を足して行番号をずらし、存在しない path と不変条件の空の行を足す
        self.write("manual/map.md", HEADER + row("misc", "src/gone.rs", inv="") + row("store", "src/store.rs"))
        self.commit("head")
        proc, rep = self.run_tool(base)
        self.assertEqual(proc.returncode, 1, proc.stderr)
        st = self.rules(rep)
        self.assertEqual(st["arch-map.missing_row"][0], "pass")
        self.assertEqual([f["delta"] for f in self.findings(rep, "arch-map.missing_row")], ["existing"],
                         "行番号がずれても base にある食い違いは existing")
        self.assertEqual(st["arch-map.wrong_path"][0], "fail")
        wp = self.findings(rep, "arch-map.wrong_path")
        self.assertEqual([(f["delta"], f["severity"], f["candidate"]) for f in wp], [("new", "error", False)])
        self.assertEqual(wp[0]["evidence"][0]["path"], "manual/map.md")
        self.assertEqual(wp[0]["evidence"][0]["line"], 3)
        self.assertIn("src/gone.rs", wp[0]["rationale"])
        self.assertEqual(st["arch-map.missing_invariant"][0], "fail")
        for kind in ("missing_test", "wrong_owner", "stale"):
            self.assertEqual(st[f"arch-map.{kind}"], ("pass", ""), kind)
        # 同じ入力で byte 単位で同じ report
        _, rep2 = self.run_tool(base, out="again.json")
        self.assertEqual(json.dumps(rep, sort_keys=True), json.dumps(rep2, sort_keys=True))

    def test_missing_row_is_new_when_head_drops_the_owner(self):
        self.profile()
        full = HEADER + row("store", "src/store.rs") + row("api", "src/api.rs", tests="`src/api_tests.rs`")
        self.write("manual/map.md", full)
        base = self.commit("base")
        self.write("manual/map.md", HEADER + row("store", "src/store.rs") + row("api", "src/api_tests.rs"))
        self.commit("head")
        proc, rep = self.run_tool(base)
        self.assertEqual(proc.returncode, 1, proc.stderr)
        st = self.rules(rep)
        # api の行の entry が試験 file に替わった: owner の api.rs はどの行の owner・entry にも無い
        self.assertEqual(st["arch-map.missing_row"][0], "fail")
        self.assertEqual([f["delta"] for f in self.findings(rep, "arch-map.missing_row")], ["new"])

    def test_wrong_owner_and_missing_test_from_reference(self):
        self.profile()
        self.write("manual/map.md", HEADER + row("store", "src/store.rs") + row("api", "src/api.rs", tests="`src/api_tests.rs`"))
        base = self.commit("base")
        caps = json.loads((self.repo / "cap.json").read_text())
        self.write("manual/map.md", HEADER + row("store", "src/store.rs", tests="`src/api_tests.rs`")
                   + row("api", "src/api_tests.rs", tests="`src/api_tests.rs`"))
        self.commit("head")
        proc, rep = self.run_tool(base)
        st = self.rules(rep)
        self.assertEqual(st["arch-map.missing_test"][0], "fail")
        self.assertIn("src/store_tests.rs", self.findings(rep, "arch-map.missing_test")[0]["rationale"])
        self.assertEqual(caps["capabilities"][1]["id"], "api")
        # wrong_owner: 行はあるが owner の一部がどの行にも無い
        caps["capabilities"][0]["owners"].append({"path": "src/api.rs"})
        self.write("cap.json", json.dumps(caps))
        self.write("manual/map.md", HEADER + row("store", "src/store.rs"))
        base2 = self.commit("reference with two owners")
        _, rep2 = self.run_tool(base2, out="r2.json")
        wo = self.findings(rep2, "arch-map.wrong_owner")
        self.assertEqual([(f["delta"], f["evidence"][0]["line"]) for f in wo], [("existing", 3)])
        self.assertIn("src/api.rs", wo[0]["rationale"])

    def test_without_reference_reference_kinds_are_not_applicable(self):
        self.profile(reference=False)
        self.write("manual/map.md", HEADER + row("store", "src/store.rs"))
        base = self.commit("base")
        self.write("manual/map.md", HEADER + row("store", "src/store.rs", tests="なし"))
        self.commit("head")
        proc, rep = self.run_tool(base)
        st = self.rules(rep)
        self.assertEqual(st["arch-map.missing_row"][0], "not_applicable")
        self.assertEqual(st["arch-map.wrong_owner"][0], "not_applicable")
        self.assertEqual(st["arch-map.missing_test"][0], "fail", "地図だけの判定は対照なしでも働く")
        self.assertEqual(proc.returncode, 1)

    def test_missing_map_reference_or_broken_table_is_unknown_not_pass(self):
        self.profile()
        self.write("manual/map.md", HEADER + row("store", "src/store.rs"))
        base = self.commit("base")
        cases = {
            "map": lambda: (self.repo / "manual/map.md").unlink(),
            "table": lambda: self.write("manual/map.md", HEADER + "| store | core | `src/store.rs` |\n"),
        }
        for name, change in cases.items():
            git(self.repo, "reset", "-q", "--hard", base)
            change()
            self.commit(name)
            proc, rep = self.run_tool(base, out=f"{name}.json")
            self.assertEqual(proc.returncode, 2, name)
            self.assertEqual(rep["summary"]["verdict"], "error", name)
            prefix = {"map": "arch_map_missing", "table": "arch_map_malformed"}[name]
            self.assertEqual(self.rules(rep)["arch-map.wrong_path"][0], "unknown", name)
            self.assertTrue(self.rules(rep)["arch-map.wrong_path"][1].startswith(prefix), name)
        # 対照は policy の側（base）で読む。base に無ければ unknown
        git(self.repo, "reset", "-q", "--hard", base)
        (self.repo / "cap.json").unlink()
        base2 = self.commit("no reference")
        self.write("README.md", "x\n")
        self.commit("head")
        proc, rep = self.run_tool(base2, out="noref.json")
        self.assertEqual(proc.returncode, 2)
        self.assertTrue(self.rules(rep)["arch-map.missing_row"][1].startswith("arch_map_reference_missing"))

    def test_doc_only_change_still_evaluates_map(self):
        self.profile()
        self.write("manual/map.md", HEADER + row("store", "src/store.rs") + row("api", "src/api.rs", tests="`src/api_tests.rs`"))
        base = self.commit("base")
        self.write("manual/map.md", HEADER + row("store", "src/nope.rs") + row("api", "src/api.rs", tests="`src/api_tests.rs`"))
        self.commit("docs only")
        proc, rep = self.run_tool(base)
        self.assertEqual(rep["inputs"]["mode"], "doc-only")
        self.assertEqual(self.rules(rep)["arch-map.wrong_path"][0], "fail")
        self.assertEqual(proc.returncode, 1)


if __name__ == "__main__":
    unittest.main()
