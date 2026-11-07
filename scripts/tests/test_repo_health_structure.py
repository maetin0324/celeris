"""repo health の構造 rule（deps.violation・deps.cycle・size.responsibility-growth）の正例・負例。

一時 dir の小さな git repo を base と head の 2 commit で作り、検査器の写し（rules/ は本物、
source-size-report.py も写す）を実行する。report は毎回 validate_report.py に通す。
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
SOURCE_SIZE = ROOT / "scripts/dev/source-size-report.py"
RULES = "deps.violation,deps.cycle,size.responsibility-growth"

LAYERS = """
[[rules."deps.violation".layers]]
name = "core"
paths = ['^core/', '^crates/core/']

[[rules."deps.violation".layers]]
name = "api"
paths = ['^api/', '^crates/api/']
allow = ["core"]
"""


def git(cwd: Path, *args: str) -> str:
    env = {**os.environ, "GIT_AUTHOR_DATE": "2026-01-01T00:00:00Z", "GIT_COMMITTER_DATE": "2026-01-01T00:00:00Z"}
    return subprocess.run(["git", "-C", str(cwd), *args], check=True, capture_output=True, text=True,
                          env=env).stdout.strip()


def rust_impl(name: str, methods: list[str], indent: str = "") -> str:
    body = "".join(f"{indent}    pub fn {m}(&self) -> u32 {{\n{indent}        1\n{indent}    }}\n" for m in methods)
    return f"{indent}impl {name} {{\n{body}{indent}}}\n"


class RepoHealthStructureTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp(prefix="rh-struct-", dir=os.environ.get("TMPDIR")))
        self.tool = self.tmp / "dev" / "repo-health"
        shutil.copytree(TOOL_SRC, self.tool, ignore=shutil.ignore_patterns("__pycache__"))
        shutil.copy(SOURCE_SIZE, self.tmp / "dev" / "source-size-report.py")
        self.repo = self.tmp / "repo"
        self.repo.mkdir()
        git(self.repo, "init", "-q", "-b", "main")
        git(self.repo, "config", "user.email", "t@example.com")
        git(self.repo, "config", "user.name", "t")
        git(self.repo, "config", "commit.gpgsign", "false")

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    # ------------------------------------------------------------ helpers
    def write(self, rel: str, text: str) -> None:
        path = self.repo / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(textwrap.dedent(text), encoding="utf-8")

    def remove(self, rel: str) -> None:
        (self.repo / rel).unlink()

    def commit(self, msg: str) -> str:
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-q", "--allow-empty", "-m", msg)
        return git(self.repo, "rev-parse", "HEAD")

    def profile(self, extra: str = "") -> None:
        self.write("scripts/dev/repo-health/profiles/default.toml",
                   'version = "1"\n[rules."policy.changed"]\nrequired = true\n' + textwrap.dedent(extra))

    def run_tool(self, base: str, head: str = "HEAD"):
        out = self.tmp / "report.json"
        if out.exists():
            out.unlink()
        cmd = [sys.executable, "-I", str(self.tool / "repo_health.py"), "--repo", str(self.repo), "--base", base,
               "--head", head, "--policy-version", "1", "--rules", RULES, "--out", str(out)]
        proc = subprocess.run(cmd, capture_output=True, text=True)
        self.assertTrue(out.exists(), proc.stderr + proc.stdout)
        check = subprocess.run([sys.executable, "-I", str(self.tool / "validate_report.py"), str(out)],
                               capture_output=True, text=True)
        self.assertEqual(check.returncode, 0, check.stdout + check.stderr)
        return proc, json.loads(out.read_text(encoding="utf-8"))

    @staticmethod
    def rule(report: dict, rid: str) -> dict:
        return next(r for r in report["rules"] if r["id"] == rid)

    @staticmethod
    def findings(report: dict, rid: str) -> list[dict]:
        return [f for f in report["findings"] if f["rule_id"] == rid]

    # ------------------------------------------------------------ deps.cycle
    def test_new_python_cycle_fails_with_paths_and_lines(self):
        self.profile()
        self.write("pkg/a.py", "from pkg import b\n\nX = 1\n")
        self.write("pkg/b.py", "Y = 2\n")
        self.write("pkg/__init__.py", "")
        base = self.commit("base")
        self.write("pkg/b.py", '"""doc"""\nfrom pkg import a\n\nY = 2\n')
        self.commit("head")
        proc, report = self.run_tool(base)
        self.assertEqual(proc.returncode, 1, proc.stdout)
        self.assertEqual(self.rule(report, "deps.cycle")["status"], "fail")
        [f] = self.findings(report, "deps.cycle")
        self.assertEqual(f["delta"], "new")
        self.assertEqual(f["severity"], "error")
        locs = {(e["path"], e["line"]) for e in f["evidence"]}
        self.assertEqual(locs, {("pkg/a.py", 1), ("pkg/b.py", 2)})

    def test_existing_cycle_left_alone_passes(self):
        self.profile()
        self.write("pkg/a.py", "from pkg import b\n")
        self.write("pkg/b.py", "from pkg import a\n")
        self.write("pkg/c.py", "Z = 1\n")
        base = self.commit("base")
        self.write("pkg/c.py", "Z = 2\n")
        self.commit("head")
        proc, report = self.run_tool(base)
        self.assertEqual(proc.returncode, 0, proc.stdout)
        [f] = self.findings(report, "deps.cycle")
        self.assertEqual(f["delta"], "existing")
        self.assertEqual(self.rule(report, "deps.cycle")["status"], "pass")

    def test_existing_cycle_that_grows_is_worsened(self):
        self.profile()
        self.write("pkg/a.py", "from pkg import b\n")
        self.write("pkg/b.py", "from pkg import a\n")
        self.write("pkg/c.py", "Z = 1\n")
        base = self.commit("base")
        self.write("pkg/b.py", "from pkg import a\nfrom pkg import c\n")
        self.write("pkg/c.py", "from pkg import a\n")
        self.commit("head")
        proc, report = self.run_tool(base)
        self.assertEqual(proc.returncode, 1, proc.stdout)
        [f] = self.findings(report, "deps.cycle")
        self.assertEqual(f["delta"], "worsened")
        self.assertEqual(f["baseline"], {"value": 2, "head_value": 3})

    def test_rust_module_cycle_ignores_crate_root_reexports(self):
        self.profile()
        self.write("crates/core/Cargo.toml", '[package]\nname = "core"\nversion = "0.1.0"\n')
        self.write("crates/core/src/lib.rs", "pub mod store;\npub mod dispatch;\npub use store::Store;\n")
        self.write("crates/core/src/store.rs", "use crate::Thing;\npub struct Store;\n")
        self.write("crates/core/src/dispatch.rs", "use crate::store::Store;\npub struct Thing;\n")
        base = self.commit("base")
        proc, report = self.run_tool(base)
        self.assertEqual(self.findings(report, "deps.cycle"), [])  # crate::Thing は root の再 export
        self.write("crates/core/src/store.rs",
                   "use crate::Thing;\npub struct Store;\n\n#[cfg(test)]\nmod tests {\n    use crate::dispatch::X;\n}\n")
        self.commit("test only")
        proc, report = self.run_tool(base)
        self.assertEqual(self.findings(report, "deps.cycle"), [])  # #[cfg(test)] の use は数えない
        self.write("crates/core/src/store.rs", "use crate::{dispatch::Thing, Thing as T};\npub struct Store;\n")
        self.commit("head")
        proc, report = self.run_tool(base)
        self.assertEqual(proc.returncode, 1, proc.stdout)
        [f] = self.findings(report, "deps.cycle")
        self.assertEqual(f["scope"], {"kind": "module", "id": "crates/core/src/dispatch"})
        self.assertIn(("crates/core/src/store.rs", 1), {(e["path"], e["line"]) for e in f["evidence"]})

    # ------------------------------------------------------------ deps.violation
    def test_layer_violation_new_vs_existing_and_not_applicable(self):
        self.profile()
        self.write("core/model.py", "X = 1\n")
        self.write("api/http.py", "from core import model\n")
        base = self.commit("base")
        proc, report = self.run_tool(base)
        self.assertEqual(self.rule(report, "deps.violation")["status"], "not_applicable")
        self.assertTrue(self.rule(report, "deps.violation")["reason"].startswith("no_layers_declared"))

        self.profile(LAYERS)
        base = self.commit("layers")
        self.write("core/model.py", "from api import http\n\nX = 1\n")
        head = self.commit("core uses api")
        proc, report = self.run_tool(base, head)
        self.assertEqual(proc.returncode, 1, proc.stdout)
        [f] = self.findings(report, "deps.violation")
        self.assertEqual((f["delta"], f["evidence"][0]["path"], f["evidence"][0]["line"]), ("new", "core/model.py", 1))
        self.assertIn("core", f["rationale"])
        # 違反が base にもあれば existing（合格を妨げない）
        self.write("api/http.py", "from core import model\n\nY = 2\n")
        self.commit("unrelated")
        proc, report = self.run_tool(head)
        self.assertEqual(proc.returncode, 0, proc.stdout)
        self.assertEqual([f["delta"] for f in self.findings(report, "deps.violation")], ["existing"])

    def test_cargo_layer_violation(self):
        self.profile(LAYERS)
        self.write("Cargo.toml", '[workspace]\nmembers = ["crates/*"]\n')
        self.write("crates/core/Cargo.toml", '[package]\nname = "core"\nversion = "0.1.0"\n')
        self.write("crates/core/src/lib.rs", "pub fn f() {}\n")
        self.write("crates/api/Cargo.toml",
                   '[package]\nname = "api"\nversion = "0.1.0"\n\n[dependencies]\ncore = { path = "../core" }\n')
        self.write("crates/api/src/lib.rs", "pub use core::f;\n")
        base = self.commit("base")
        self.write("crates/core/Cargo.toml",
                   '[package]\nname = "core"\nversion = "0.1.0"\n\n[dev-dependencies]\napi = { path = "../api" }\n')
        dev = self.commit("dev dep")
        proc, report = self.run_tool(base, dev)
        self.assertEqual(proc.returncode, 0, proc.stdout)  # dev-dependencies は既定で見ない
        self.write("crates/core/Cargo.toml",
                   '[package]\nname = "core"\nversion = "0.1.0"\n\n[dependencies]\napi = { path = "../api" }\n')
        self.commit("head")
        proc, report = self.run_tool(base)
        self.assertEqual(proc.returncode, 1, proc.stdout)
        [f] = self.findings(report, "deps.violation")
        self.assertEqual((f["evidence"][0]["path"], f["evidence"][0]["line"]), ("crates/core/Cargo.toml", 6))
        self.assertEqual(f["scope"]["kind"], "crate")
        cyc = self.findings(report, "deps.cycle")
        self.assertEqual([(c["delta"], c["scope"]["kind"]) for c in cyc], [("new", "crate")])

    def test_invalid_layer_config_is_unknown_not_pass(self):
        self.profile('[rules."deps.violation"]\nlayers = [{ name = "a", paths = ["^a/"], allow = ["nope"] }]\n')
        self.write("a/x.py", "X = 1\n")
        base = self.commit("base")
        self.write("a/x.py", "X = 2\n")
        self.commit("head")
        proc, report = self.run_tool(base)
        self.assertEqual(proc.returncode, 2, proc.stdout)
        self.assertEqual(self.rule(report, "deps.violation")["status"], "unknown")

    # ------------------------------------------------------------ size.responsibility-growth
    def test_threshold_growth_new_worsened_existing(self):
        self.profile('[rules."size.responsibility-growth"]\nfile_lines = 10\nfn_lines = 6\n')
        self.write("app/big.py", "".join(f"A{i} = {i}\n" for i in range(12)))
        self.write("app/small.py", "def f():\n    return 1\n")
        base = self.commit("base")
        # 変えなければ existing
        self.write("app/other.py", "Z = 1\n")
        self.commit("unrelated")
        proc, report = self.run_tool(base)
        self.assertEqual(proc.returncode, 0, proc.stdout)
        self.assertEqual([f["delta"] for f in self.findings(report, "size.responsibility-growth")], ["existing"])
        # 閾値超えの file が増える → worsened、閾値以下の関数が超える → new
        self.write("app/big.py", "".join(f"A{i} = {i}\n" for i in range(14)))
        self.write("app/small.py", "def f():\n" + "".join(f"    x{i} = {i}\n" for i in range(6)) + "    return 1\n")
        self.commit("grow")
        proc, report = self.run_tool(base)
        self.assertEqual(proc.returncode, 1, proc.stdout)
        got = {(f["evidence"][0]["path"], f["delta"]) for f in self.findings(report, "size.responsibility-growth")}
        self.assertEqual(got, {("app/big.py", "worsened"), ("app/small.py", "new")})
        big = next(f for f in self.findings(report, "size.responsibility-growth") if f["delta"] == "worsened")
        self.assertEqual(big["baseline"], {"value": 12, "head_value": 14, "threshold": 10})
        fn = next(f for f in self.findings(report, "size.responsibility-growth") if f["delta"] == "new")
        self.assertEqual((fn["evidence"][0]["line"], fn["evidence"][0]["end_line"]), (1, 8))

    def test_split_only_is_not_counted_as_improvement(self):
        self.profile('[rules."size.responsibility-growth"]\ntype_methods = 3\nmodule_lines = 15\n')
        self.write("crates/core/Cargo.toml", '[package]\nname = "core"\nversion = "0.1.0"\n')
        self.write("crates/core/src/lib.rs", "pub mod foo;\n")
        self.write("crates/core/src/foo.rs",
                   "pub struct Foo;\n\n" + rust_impl("Foo", ["a", "b"]) + "\n" + rust_impl("Foo", ["c", "d"]))
        base = self.commit("base")
        proc, base_report = self.run_tool(base, base)
        base_fps = {f["fingerprint"] for f in self.findings(base_report, "size.responsibility-growth")}
        self.assertEqual(len(base_fps), 2)  # module foo の行、型 Foo の method 4
        # foo.rs を foo/mod.rs と foo/more.rs に分けるだけ: 同じ finding が existing のまま残る
        self.remove("crates/core/src/foo.rs")
        self.write("crates/core/src/foo/mod.rs", "mod more;\npub struct Foo;\n\n" + rust_impl("Foo", ["a", "b"]))
        self.write("crates/core/src/foo/more.rs", "use super::Foo;\n\n" + rust_impl("Foo", ["c", "d"]))
        split = self.commit("split")
        proc, report = self.run_tool(base, split)
        self.assertEqual(proc.returncode, 0, proc.stdout)
        fs = self.findings(report, "size.responsibility-growth")
        self.assertEqual({f["fingerprint"] for f in fs}, base_fps)
        self.assertEqual({f["delta"] for f in fs}, {"existing"})
        methods = next(f for f in fs if "method" in f["rationale"])
        self.assertEqual({e["path"] for e in methods["evidence"]},
                         {"crates/core/src/foo/mod.rs", "crates/core/src/foo/more.rs"})
        # 分けた上で method を足せば悪化（file ごとには 2→3 で閾値以下でも型の合計で数える）。module の行も増える
        self.write("crates/core/src/foo/more.rs", "use super::Foo;\n\n" + rust_impl("Foo", ["c", "d", "e"]))
        self.commit("split and grow")
        proc, report = self.run_tool(base)
        self.assertEqual(proc.returncode, 1, proc.stdout)
        methods = next(f for f in self.findings(report, "size.responsibility-growth") if "method" in f["rationale"])
        self.assertEqual((methods["delta"], methods["baseline"]["value"], methods["baseline"]["head_value"]),
                         ("worsened", 4, 5))

    def test_inline_tests_do_not_count_as_growth(self):
        self.profile('[rules."size.responsibility-growth"]\nfile_lines = 5\n')
        self.write("crates/core/Cargo.toml", '[package]\nname = "core"\nversion = "0.1.0"\n')
        self.write("crates/core/src/lib.rs", "pub fn f() -> u32 {\n    1\n}\n")
        base = self.commit("base")
        self.write("crates/core/src/lib.rs", "pub fn f() -> u32 {\n    1\n}\n\n#[cfg(test)]\nmod tests {\n"
                   + "".join(f"    #[test]\n    fn t{i}() {{}}\n" for i in range(5)) + "}\n")
        self.commit("tests")
        proc, report = self.run_tool(base)
        self.assertEqual(proc.returncode, 0, proc.stdout)
        self.assertEqual(self.findings(report, "size.responsibility-growth"), [])


if __name__ == "__main__":
    unittest.main()
