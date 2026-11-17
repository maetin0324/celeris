"""deps.cycle の循環の内部の新しい組と、Rust の論理 module（#[path]・super::）の対照試験。

ADR 2026-11-17-repo-health-scc-edges-logical-modules の D6。守る不変条件（brief の I1〜I4）:
- I1 base で既知の循環の内部の組・論理 module の負債は existing のまま（fail にしない）。
- I2 既存の循環の内部の新しい組は、節点の数が変わらなくても fail（`super::` で書いても同じ）。
- I3 組の削除・循環の外の単位との型の import は pass。
- I4 `#[path]` で別の dir に移した file も宣言した module に数え、module の行と循環の内部の組が減らない。
既存の試験（test_repo_health_structure.py など）は循環の節点の数と物理の path の単位しか見ないので、
節点の数を変えない新しい組と、`#[path]`・`super::` による単位のすり抜けを検出できない。

fixture は試験ごとの一時 dir の git repo（汎用の Rust package `demo`。base は a→b→c→a の 3 単位の循環と、
何にも依存しない types）。本物の scripts/dev/repo-health/repo_health.py を base と head の 2 commit に当て、
report を validate_report.py に通す。git の道具は repo_health_fixtures.py の関数を使う（file は変えない）。
実行: python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health_scc_edges.py' -v
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

sys.path.insert(0, str(Path(__file__).resolve().parent))
from repo_health_fixtures import PROFILE, commit, git, write  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
TOOL = ROOT / "scripts/dev/repo-health"

LIB = "pub mod a;\npub mod b;\npub mod c;\npub mod types;\n"
BASE_FILES = {
    "Cargo.toml": '[package]\nname = "demo"\nversion = "0.1.0"\nedition = "2021"\n',
    "src/lib.rs": LIB,
    "src/a.rs": "use crate::b::B;\n\npub struct A;\n\npub fn a(_: B) {}\n",
    "src/b.rs": "use crate::c::C;\n\npub struct B;\npub struct B2;\n\npub fn b(_: C) {}\n",
    "src/c.rs": "use crate::a::A;\n\npub struct C;\n\npub fn c(_: A) {}\n",
    "src/types.rs": "pub struct Id;\n",
}
SMALL_MODULES = '[rules."size.responsibility-growth"]\nmodule_lines = 40\n'


def unit(name: str) -> str:
    return f"./src/{name}"


def fns(prefix: str, n: int) -> str:
    """n 個の関数（1 個 3 行の本体の行）。"""
    return "".join(f"pub fn {prefix}{i}() -> u32 {{\n    {i}\n}}\n\n" for i in range(n))


class SccEdgeTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp(prefix="rh-scc-", dir=os.environ.get("TMPDIR")))
        self.repo = self.tmp / "repo"
        self.repo.mkdir()
        git(self.repo, "init", "-q", "-b", "main")
        for key, value in (("user.name", "Fixture"), ("user.email", "fixture@example.invalid"),
                           ("commit.gpgsign", "false")):
            git(self.repo, "config", key, value)

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    # ------------------------------------------------------------ helpers
    def base(self, extra: dict[str, str] | None = None, profile: str = "") -> str:
        write(self.repo, PROFILE, 'version = "1"\n' + profile)
        for path, body in {**BASE_FILES, **(extra or {})}.items():
            write(self.repo, path, body)
        return commit(self.repo, "base")

    def head(self, files: dict[str, str | None], subject: str = "head") -> str:
        for path, body in files.items():
            if body is None:
                (self.repo / path).unlink()
            else:
                write(self.repo, path, body)
        return commit(self.repo, subject)

    def run_tool(self, base: str, head: str) -> tuple[int, dict]:
        out = self.tmp / f"report-{head[:8]}.json"
        proc = subprocess.run([sys.executable, "-I", str(TOOL / "repo_health.py"), "--repo", str(self.repo),
                               "--base", base, "--head", head, "--policy-version", "1", "--out", str(out)],
                              capture_output=True, text=True)
        self.assertTrue(out.exists(), proc.stderr + proc.stdout)
        check = subprocess.run([sys.executable, "-I", str(TOOL / "validate_report.py"), str(out)],
                               capture_output=True, text=True)
        self.assertEqual(check.returncode, 0, check.stdout + check.stderr)
        return proc.returncode, json.loads(out.read_text(encoding="utf-8"))

    @staticmethod
    def findings(report: dict, rid: str, prefix: str = "") -> list[dict]:
        return [f for f in report["findings"] if f["rule_id"] == rid and f["rationale"].startswith(prefix)]

    def edge_findings(self, report: dict) -> list[dict]:
        return [f for f in self.findings(report, "deps.cycle") if "新しい組" in f["rationale"]]

    def cycle_findings(self, report: dict) -> list[dict]:
        return [f for f in self.findings(report, "deps.cycle") if "新しい組" not in f["rationale"]]

    def module_lines(self, report: dict, name: str) -> list[dict]:
        return [f for f in self.findings(report, "size.responsibility-growth")
                if f["scope"]["id"] == unit(name) and f["rationale"].startswith("module の本体の行")]

    def assert_new_edge(self, report: dict, src: str, dst: str) -> None:
        [f] = self.edge_findings(report)
        self.assertEqual((f["delta"], f["severity"], f["scope"]["id"]), ("new", "error", unit(src)))
        self.assertIn(f"{unit(src)} -> {unit(dst)}", f["rationale"])
        self.assertEqual({e["symbol"] for e in f["evidence"]}, {f"{unit(src)} -> {unit(dst)}"})

    def assert_cycle_existing(self, report: dict, size: int = 3) -> None:
        [f] = self.cycle_findings(report)
        self.assertEqual((f["delta"], f["baseline"]["head_value"]), ("existing", size))

    # ------------------------------------------------------------ deps.cycle の組
    def test_scc_edges_new_internal_edge_fails(self):
        base = self.base()
        head = self.head({"src/a.rs": "use crate::b::B;\nuse crate::c::C;\n\npub struct A;\n\npub fn a(_: B, _: C) {}\n"})
        code, report = self.run_tool(base, head)
        self.assertEqual(code, 1)
        self.assert_new_edge(report, "a", "c")
        self.assertEqual(self.edge_findings(report)[0]["evidence"][0]["path"], "src/a.rs")
        self.assert_cycle_existing(report)  # 節点は 3 のまま

    def test_scc_edges_existing_internal_edge_kept_passes(self):
        base = self.base()
        head = self.head({"src/a.rs": "use crate::b::{B, B2};\n\npub struct A;\n\npub fn a(_: B, _: B2) {}\n"})
        code, report = self.run_tool(base, head)
        self.assertEqual(code, 0)
        self.assertEqual(self.edge_findings(report), [])
        self.assert_cycle_existing(report)

    def test_scc_edges_removed_edge_passes(self):
        # base は組 b→a を足した 4 組。b→a を消しても循環 a→b→c→a は残り、節点は 3 のまま。
        b_with_a = "use crate::a::A;\nuse crate::c::C;\n\npub struct B;\npub struct B2;\n\npub fn b(_: C, _: A) {}\n"
        base = self.base({"src/b.rs": b_with_a})
        head = self.head({"src/b.rs": BASE_FILES["src/b.rs"]})
        code, report = self.run_tool(base, head)
        self.assertEqual(code, 0)
        self.assertEqual(self.edge_findings(report), [])
        self.assert_cycle_existing(report)
        # 続けて c→a も消すと循環が無くなる。これも pass。
        head2 = self.head({"src/c.rs": "pub struct C;\n\npub fn c() {}\n"}, "drop c->a")
        code, report = self.run_tool(base, head2)
        self.assertEqual(code, 0)
        self.assertEqual(self.findings(report, "deps.cycle"), [])

    def test_scc_edges_legal_type_import_passes(self):
        base = self.base()
        cases = {
            # 既存の内部の組 a→b に沿った型の import
            "along existing pair": {"src/a.rs": "use crate::b::{B, B2};\n\npub struct A;\n\npub fn a(_: B, _: B2) {}\n"},
            # 循環の中の a が循環の外の types の型を import
            "to outside unit": {"src/a.rs": "use crate::b::B;\nuse crate::types::Id;\n\npub struct A;\n\n"
                                            "pub fn a(_: B, _: Id) {}\n"},
            # 循環の外の新しい module d が循環の中の a の型を import
            "from outside unit": {"src/lib.rs": LIB + "pub mod d;\n",
                                  "src/d.rs": "use crate::a::A;\n\npub fn d(_: A) {}\n"},
        }
        for name, files in cases.items():
            with self.subTest(name):
                git(self.repo, "reset", "-q", "--hard", base)
                head = self.head(files, name)
                code, report = self.run_tool(base, head)
                self.assertEqual(code, 0)
                self.assertEqual(self.edge_findings(report), [])
                self.assert_cycle_existing(report)

    def test_scc_edges_super_path_new_internal_edge_fails(self):
        base = self.base()
        head = self.head({
            "src/a.rs": None,
            "src/a/mod.rs": "use crate::b::B;\n\npub mod inner;\n\npub struct A;\n\npub fn a(_: B) {}\n",
            # super::A は自分の単位 a。super::super::c は crate root を経て c（新しい組 a→c）
            "src/a/inner.rs": "use super::super::c::C;\nuse super::A;\n\npub fn inner(_: C, _: A) {}\n",
        })
        code, report = self.run_tool(base, head)
        self.assertEqual(code, 1)
        self.assert_new_edge(report, "a", "c")
        self.assertEqual(self.edge_findings(report)[0]["evidence"][0]["path"], "src/a/inner.rs")
        self.assert_cycle_existing(report)

    # ------------------------------------------------------------ #[path] の論理 module
    def test_logical_module_path_move_size_fails(self):
        base = self.base({"src/b.rs": BASE_FILES["src/b.rs"] + fns("f", 10)}, SMALL_MODULES)
        _, report = self.run_tool(base, base)
        self.assertEqual(self.module_lines(report, "b"), [])  # base の b は閾値の下
        head = self.head({
            "src/b.rs": BASE_FILES["src/b.rs"] + '\n#[path = "extra/b_impl.rs"]\nmod imp;\n\n' + fns("f", 9),
            "src/extra/b_impl.rs": fns("g", 8),
        })
        code, report = self.run_tool(base, head)
        self.assertEqual(code, 1)
        [f] = self.module_lines(report, "b")
        self.assertEqual((f["delta"], f["severity"]), ("new", "error"))
        self.assertIn("55 が閾値 40 を超えた", f["rationale"])  # b.rs 31 行 + b_impl.rs 24 行（物理ではどちらも 40 以下）
        self.assertEqual({e["path"] for e in f["evidence"]}, {"src/b.rs", "src/extra/b_impl.rs"})
        self.assertEqual(self.module_lines(report, "extra"), [])  # 物理の単位 extra には数えない

    def test_logical_module_path_move_deps_fails(self):
        base = self.base()
        head = self.head({
            "src/a.rs": BASE_FILES["src/a.rs"] + '\n#[path = "misc/a_part.rs"]\nmod part;\n',
            "src/misc/a_part.rs": "use crate::c::C;\n\npub fn part(_: C) {}\n",
        })
        code, report = self.run_tool(base, head)
        self.assertEqual(code, 1)
        self.assert_new_edge(report, "a", "c")  # 物理の単位なら misc→c で循環の外
        self.assertEqual(self.edge_findings(report)[0]["evidence"][0]["path"], "src/misc/a_part.rs")
        self.assert_cycle_existing(report)

    def test_logical_module_base_debt_is_existing(self):
        debt = {
            "src/a.rs": BASE_FILES["src/a.rs"] + '\n#[path = "misc/a_part.rs"]\nmod part;\n',
            "src/misc/a_part.rs": "use crate::c::C;\n\npub fn part(_: C) {}\n",
            "src/b.rs": BASE_FILES["src/b.rs"] + '\n#[path = "extra/b_impl.rs"]\nmod imp;\n\n' + fns("f", 9),
            "src/extra/b_impl.rs": fns("g", 8),
        }
        base = self.base(debt, SMALL_MODULES)
        head = self.head({"src/types.rs": "pub struct Id;\n\npub struct Name;\n"})
        code, report = self.run_tool(base, head)
        self.assertEqual(code, 0)
        [f] = self.module_lines(report, "b")
        self.assertEqual(f["delta"], "existing")
        self.assertEqual(self.edge_findings(report), [])
        self.assert_cycle_existing(report)


if __name__ == "__main__":
    unittest.main()
