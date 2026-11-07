"""repo health の候補 rule（dup.contract・dup.state・dup.public-surface・link.unlinked・dup.test-fixture・
migration.remnant）: 一時 dir の小さな git repo で各 rule の正例・負例・例外・existing を確かめる。

本物の検査器（scripts/dev/repo-health/）をそのまま使う。profile は一時 repo の base に置く（policy は base 側）。
"""
from __future__ import annotations

import hashlib
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
TOOL = ROOT / "scripts/dev/repo-health/repo_health.py"
VALIDATOR = ROOT / "scripts/dev/repo-health/validate_report.py"
RULES = "dup.contract,dup.state,dup.public-surface,link.unlinked,dup.test-fixture,migration.remnant"
PROFILE = "scripts/dev/repo-health/profiles/default.toml"


def git(cwd: Path, *args: str) -> str:
    env = {**os.environ, "GIT_AUTHOR_DATE": "2026-01-01T00:00:00Z", "GIT_COMMITTER_DATE": "2026-01-01T00:00:00Z"}
    return subprocess.run(["git", "-C", str(cwd), *args], check=True, capture_output=True, text=True,
                          env=env).stdout.strip()


RUST_A = """\
pub struct TaskRow {
    pub id: String,
    pub state: String,
    pub owner: String,
}
"""


class RepoHealthDupTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp(prefix="rh-dup-", dir=os.environ.get("TMPDIR")))
        self.repo = self.tmp / "repo"
        self.repo.mkdir()
        git(self.repo, "init", "-q", "-b", "main")
        git(self.repo, "config", "user.email", "t@example.com")
        git(self.repo, "config", "user.name", "t")
        git(self.repo, "config", "commit.gpgsign", "false")
        self.write("README.md", "# r\n")

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    # ------------------------------------------------------------ helpers
    def write(self, rel: str, text: str) -> None:
        path = self.repo / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(textwrap.dedent(text), encoding="utf-8")

    def commit(self, msg: str) -> str:
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-q", "--allow-empty", "-m", msg)
        return git(self.repo, "rev-parse", "HEAD")

    def base(self, profile: str = "") -> str:
        self.write(PROFILE, 'version = "1"\n[rules."policy.changed"]\nrequired = true\n' + textwrap.dedent(profile))
        return self.commit("base")

    def run_tool(self, base: str, rules: str = RULES, codes=(0, 1)) -> dict:
        out = self.tmp / "report.json"
        if out.exists():
            out.unlink()
        proc = subprocess.run(
            [sys.executable, "-I", str(TOOL), "--repo", str(self.repo), "--base", base, "--head", "HEAD",
             "--policy-version", "1", "--profile", PROFILE, "--rules", rules, "--out", str(out)],
            capture_output=True, text=True)
        self.assertIn(proc.returncode, codes, proc.stderr + proc.stdout)
        check = subprocess.run([sys.executable, "-I", str(VALIDATOR), str(out)], capture_output=True, text=True)
        self.assertEqual(check.returncode, 0, check.stdout)
        return json.loads(out.read_text(encoding="utf-8"))

    def findings(self, report: dict, rid: str, path: str | None = None) -> list[dict]:
        out = [f for f in report["findings"] if f["rule_id"] == rid]
        if path is not None:
            out = [f for f in out if any(e["path"] == path for e in f["evidence"])]
        return out

    def status(self, report: dict, rid: str) -> dict:
        return next(r for r in report["rules"] if r["id"] == rid)

    def assert_candidate(self, f: dict, delta: str = "new") -> None:
        self.assertTrue(f["candidate"])
        self.assertEqual(f["severity"], "warning")
        self.assertEqual(f["delta"], delta)
        self.assertTrue(f["rationale"])

    # ------------------------------------------------------------ dup.contract
    def test_contract_same_name_and_shape(self):
        b = self.base()
        self.write("a/src/lib.rs", RUST_A)
        self.write("b/src/lib.rs", RUST_A)
        self.write("c/src/lib.rs", "pub struct Row {\n    id: String,\n    state: String,\n    owner: String,\n}\n")
        self.write("d/src/lib.rs", "pub struct Other {\n    x: u32,\n}\n")
        self.write("web/types.ts", "export interface Job {\n  id: string;\n}\n")
        self.write("web/other.ts", "export type Job = {\n  id: string;\n};\n")
        self.commit("head")
        r = self.run_tool(b)
        fs = self.findings(r, "dup.contract")
        names = {f["scope"]["id"] for f in fs}
        self.assertIn("rust:TaskRow", names)
        self.assertIn("typescript:Job", names)
        shape = [f for f in fs if "shape" in f["scope"]["id"]]
        self.assertEqual(len(shape), 1, fs)
        self.assertEqual({e["symbol"] for e in shape[0]["evidence"]}, {"Row", "TaskRow"})
        self.assertFalse(self.findings(r, "dup.contract", "d/src/lib.rs"), "1 か所だけの型は候補にしない")
        for f in fs:
            self.assert_candidate(f)
        self.assertEqual(self.status(r, "dup.contract")["status"], "pass")
        self.assertFalse(self.status(r, "dup.contract")["required"])

    def test_contract_negative_and_existing(self):
        self.write("a/src/lib.rs", RUST_A)
        self.write("b/src/lib.rs", RUST_A)
        self.write("tests/t.rs", RUST_A)
        b = self.base()
        self.write("a/src/other.rs", "pub struct Solo {\n    a: u8,\n}\n")
        self.commit("head")
        r = self.run_tool(b)
        fs = self.findings(r, "dup.contract")
        self.assertEqual([f["scope"]["id"] for f in fs if "shape" not in f["scope"]["id"]], ["rust:TaskRow"])
        self.assertEqual(fs[0]["delta"], "existing", "base にもある重複は existing")
        self.assertNotIn("tests/t.rs", {e["path"] for f in fs for e in f["evidence"]}, "試験の型は既定で数えない")

    # ------------------------------------------------------------ dup.state
    def test_state_multiple_writers(self):
        b = self.base()
        self.write("store/tasks.rs", 'fn a() { q("INSERT INTO tasks (id) VALUES (?)"); }\n')
        self.write("api/handler.rs", 'fn b() { q("UPDATE tasks SET state = ? WHERE id = ?"); }\n')
        self.write("store/events.rs", 'fn c() { q("INSERT INTO events (k) VALUES (?)"); }\n')
        self.write("migrations/0001.rs", 'fn m() { q("DELETE FROM events"); }\n')
        self.write("docs/x.rs", "// update events set later\nfn d() {}\n")
        self.commit("head")
        r = self.run_tool(b)
        fs = self.findings(r, "dup.state")
        self.assertEqual([f["scope"]["id"] for f in fs], ["table:tasks"])
        self.assert_candidate(fs[0])
        self.assertEqual({e["path"] for e in fs[0]["evidence"]}, {"store/tasks.rs", "api/handler.rs"})

    def test_state_exception_with_reason(self):
        b = self.base("""
            [rules."dup.state"]
            required = true
            [[rules."dup.state".exceptions]]
            path = '^replay/'
            reason = "migration/replay は意図して同じ表を書き直す"
        """)
        self.write("store/tasks.rs", 'q("INSERT INTO tasks VALUES (1)");\n')
        self.write("replay/tasks.rs", 'q("DELETE FROM tasks");\n')
        self.commit("head")
        r = self.run_tool(b)
        fs = self.findings(r, "dup.state")
        self.assertEqual(len(fs), 1)
        self.assertEqual(fs[0]["exception"]["reason"], "migration/replay は意図して同じ表を書き直す")
        self.assertEqual(fs[0]["exception"]["source"]["path"], PROFILE)
        st = self.status(r, "dup.state")
        self.assertTrue(st["required"], "profile で必須化できる")
        self.assertEqual(st["status"], "pass")

    # ------------------------------------------------------------ dup.public-surface
    def test_public_surface(self):
        b = self.base()
        self.write("api/a.rs", 'Router::new().route("/tasks/{id}", get(h));\n')
        self.write("gw/b.ts", "app.get('/tasks/:id', h);\n")
        self.write("cli/main.py", "sub.add_parser('show')\n")
        self.write("tools/x.py", "sub.add_parser(\"show\")\n")
        self.write("one/lib.rs", "pub fn resolve_root() {}\npub fn main() {}\n")
        self.write("two/lib.rs", "pub fn resolve_root() {}\npub fn main() {}\n    pub fn method() {}\n")
        self.write("three/lib.rs", "    pub fn method() {}\npub fn only_here() {}\n")
        self.commit("head")
        r = self.run_tool(b)
        ids = sorted(f["scope"]["id"] for f in self.findings(r, "dup.public-surface"))
        self.assertEqual(ids, ["cli:show", "fn:resolve_root", "http:/tasks/{}"])
        for f in self.findings(r, "dup.public-surface"):
            self.assert_candidate(f)

    def test_public_surface_exception_by_fingerprint(self):
        self.write("one/lib.rs", "pub fn verify_token() {}\n")
        self.write("two/lib.rs", "pub fn verify_token() {}\n")
        # fingerprint は rule ID と鍵（種類:名前）だけで決まる（path・行番号を含めない）
        key = "dup.public-surface\nfn:verify_token"
        fp = "dup.public-surface:" + hashlib.sha256(key.encode()).hexdigest()[:16]
        b = self.base(f"""
            [[rules."dup.public-surface".exceptions]]
            fingerprint = "{fp}"
            reason = "security 境界の独立検証。共通化しない"
        """)
        self.write("three/lib.rs", "pub fn x() {}\n")
        self.commit("head")
        r = self.run_tool(b)
        f = self.findings(r, "dup.public-surface")[0]
        self.assertEqual(f["fingerprint"], fp)
        self.assertEqual(f["delta"], "existing")
        self.assertEqual(f["exception"]["reason"], "security 境界の独立検証。共通化しない")

    # ------------------------------------------------------------ link.unlinked
    def test_unlinked_items_scripts_docs(self):
        b = self.base()
        self.write("src/lib.rs", "pub fn used_fn() {}\npub fn orphan_fn() {}\nfn call() { used_fn(); }\n")
        self.write("scripts/run-all.sh", "#!/bin/sh\nsh scripts/linked-one.sh\n")
        self.write("scripts/linked-one.sh", "#!/bin/sh\necho hi\n")
        self.write("scripts/lonely-tool.sh", "#!/bin/sh\necho hi\n")
        self.write("docs/guide.md", "# g\nsee [other](other.md)\n")
        self.write("docs/other.md", "# o\n")
        self.write("docs/README.md", "# index\n")
        self.commit("head")
        r = self.run_tool(b)
        fs = self.findings(r, "link.unlinked")
        keys = {(f["scope"]["kind"], f["scope"]["id"], tuple(e.get("symbol") for e in f["evidence"])) for f in fs}
        self.assertIn(("file", "src/lib.rs", ("orphan_fn",)), keys)
        paths = {e["path"] for f in fs for e in f["evidence"]}
        self.assertIn("scripts/lonely-tool.sh", paths)
        self.assertIn("docs/guide.md", paths)
        for neg in ("scripts/linked-one.sh", "docs/other.md", "docs/README.md"):
            self.assertNotIn(neg, paths)
        self.assertNotIn("used_fn", {e.get("symbol") for f in fs for e in f["evidence"]})
        for f in fs:
            # base にある profile の file 自身も未参照の script の候補（existing）
            self.assert_candidate(f, "existing" if f["evidence"][0]["path"] == PROFILE else "new")

    def test_unlinked_exception_and_config(self):
        b = self.base("""
            [rules."link.unlinked"]
            kinds = ["script"]
            [[rules."link.unlinked".exceptions]]
            path = '^scripts/hooks/'
            reason = "hook は外部の設定から名前で呼ばれる"
        """)
        self.write("scripts/hooks/pre-commit-check.sh", "#!/bin/sh\n")
        self.write("src/lib.rs", "pub fn orphan_fn() {}\n")
        self.commit("head")
        r = self.run_tool(b)
        fs = self.findings(r, "link.unlinked", "scripts/hooks/pre-commit-check.sh")
        self.assertEqual(len(fs), 1)
        self.assertEqual(fs[0]["exception"]["reason"], "hook は外部の設定から名前で呼ばれる")
        self.assertFalse(self.findings(r, "link.unlinked", "src/lib.rs"), "kinds で item を外せる")

    # ------------------------------------------------------------ dup.test-fixture
    def test_fixture_near_duplicates(self):
        body = "".join(f"assert compute_value({i}) == expected_value_{i}\n" for i in range(12))
        b = self.base()
        self.write("tests/test_a.py", body)
        self.write("tests/test_b.py", body + "assert compute_value(99) == expected_value_99\n")
        self.write("tests/test_c.py", "".join(f"check_something_else({i}, 'zzz')\n" for i in range(12)))
        self.write("src/copy.py", body)
        self.commit("head")
        r = self.run_tool(b)
        fs = self.findings(r, "dup.test-fixture")
        self.assertEqual(len(fs), 1, fs)
        self.assertEqual([e["path"] for e in fs[0]["evidence"]], ["tests/test_a.py", "tests/test_b.py"])
        self.assert_candidate(fs[0])

    def test_fixture_exception_and_existing(self):
        body = "".join(f'{{"case": {i}, "expect": "value-{i}"}}\n' for i in range(10))
        self.write("tests/fixtures/a.json", body)
        self.write("tests/fixtures/b.json", body)
        b = self.base("""
            [[rules."dup.test-fixture".exceptions]]
            path = '^tests/fixtures/'
            reason = "provider 固有の wire の録画。provider ごとに別に持つ"
        """)
        self.write("src/x.py", "print(1)\n")
        self.commit("head")
        r = self.run_tool(b)
        fs = self.findings(r, "dup.test-fixture")
        self.assertEqual(len(fs), 1)
        self.assertEqual(fs[0]["delta"], "existing")
        self.assertIn("provider 固有", fs[0]["exception"]["reason"])

    # ------------------------------------------------------------ migration.remnant
    def test_remnant_vocabulary(self):
        b = self.base("""
            [rules."migration.remnant"]
            retired_paths = ["scripts/old-deploy.sh"]
        """)
        self.write("src/a.rs", "#[deprecated]\npub fn old() {}\n// legacy path kept\n")
        self.write("src/b.py", "if cfg.use_new_router:\n    pass\nsubprocess.run(['scripts/old-deploy.sh'])\n")
        self.write("src/clean.rs", "pub fn fresh() {}\n")
        self.write("docs/adr.md", "deprecated legacy\n")
        self.write("tests/t.rs", "// legacy\n")
        self.commit("head")
        r = self.run_tool(b)
        fs = self.findings(r, "migration.remnant")
        got = sorted((e["path"], e["symbol"]) for f in fs for e in f["evidence"][:1])
        self.assertEqual(got, [("src/a.rs", "deprecated"), ("src/a.rs", "legacy"),
                               ("src/b.py", "retired:scripts/old-deploy.sh"), ("src/b.py", "switch-flag")])
        for f in fs:
            self.assert_candidate(f)

    def test_remnant_worsened_and_exception(self):
        self.write("src/a.py", "# legacy\n")
        b = self.base("""
            [[rules."migration.remnant".exceptions]]
            path = '^src/replay/'
            reason = "replay は旧い event の形を読み続ける"
        """)
        self.write("src/a.py", "# legacy\n# legacy again\n")
        self.write("src/replay/r.py", "LEGACY_KIND = 'legacy'\n")
        self.commit("head")
        r = self.run_tool(b)
        a = self.findings(r, "migration.remnant", "src/a.py")[0]
        self.assertEqual(a["delta"], "worsened")
        self.assertEqual(a["baseline"], {"value": 1, "head_value": 2})
        rp = self.findings(r, "migration.remnant", "src/replay/r.py")[0]
        self.assertEqual(rp["exception"]["reason"], "replay は旧い event の形を読み続ける")

    # ------------------------------------------------------------ 共通
    def test_candidates_are_not_modified(self):
        self.write("a/src/lib.rs", RUST_A)
        b = self.base()
        self.write("b/src/lib.rs", RUST_A + "// legacy\n")
        self.write("scripts/lonely-tool.sh", "#!/bin/sh\n")
        self.write("x/a.rs", 'q("INSERT INTO t VALUES (1)");\n')
        self.write("y/a.rs", 'q("DELETE FROM t");\npub fn open_db() {}\n')
        self.write("z/a.rs", "pub fn open_db() {}\n")
        body = "".join(f"assert compute_value({i}) == expected_value_{i}\n" for i in range(12))
        self.write("tests/test_a.py", body)
        self.write("tests/test_b.py", body)
        head = self.commit("head")

        def digest() -> str:
            h = hashlib.sha256()
            for p in sorted(self.repo.rglob("*")):
                if ".git" in p.relative_to(self.repo).parts or not p.is_file():
                    continue
                h.update(str(p.relative_to(self.repo)).encode() + b"\0" + p.read_bytes())
            return h.hexdigest()

        before = digest()
        r = self.run_tool(b)
        self.assertEqual({f["rule_id"] for f in r["findings"]} - {"policy.changed"},
                         set(RULES.split(",")), "6 rule がすべて候補を出す")
        self.assertTrue(all(f["candidate"] and f["severity"] != "error"
                            for f in r["findings"] if f["rule_id"] != "policy.changed"))
        self.assertEqual(digest(), before, "候補の file を書き換え・削除しない")
        self.assertEqual(git(self.repo, "status", "--porcelain"), "", "作業ツリーを汚さない")
        self.assertEqual(git(self.repo, "rev-parse", "HEAD"), head)
        self.assertEqual(r["summary"]["verdict"], "pass", "必須でない候補 rule は verdict を落とさない")

    def test_bad_rule_config_is_error_not_pass(self):
        b = self.base("""
            [rules."dup.state"]
            required = true
            group_by = "crate"
        """)
        self.write("x/a.rs", 'q("INSERT INTO t VALUES (1)");\n')
        self.commit("head")
        r = self.run_tool(b, rules="dup.state", codes=(2,))
        st = self.status(r, "dup.state")
        self.assertEqual(st["status"], "error")
        self.assertEqual(st["reason"], "crashed:dup.state:ConfigError")
        self.assertEqual(r["summary"]["exit_code"], 2)

    def test_doc_only_is_not_applicable(self):
        self.write("src/lib.rs", RUST_A)
        b = self.base()
        self.write("docs/a.md", "# a\n")
        self.commit("head")
        r = self.run_tool(b)
        for rid in RULES.split(","):
            self.assertEqual(self.status(r, rid)["status"], "not_applicable", rid)


if __name__ == "__main__":
    unittest.main()
