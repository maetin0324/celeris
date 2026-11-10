#!/usr/bin/env python3
"""cases.py と mutate.sh を合成の fixture（一時 git repo）で確かめる。

    python3 agent-docs/progress/2026-11-10-health-v2-r5/test_guard.py

fixture は $TMPDIR の下に作り、試験の後に消す。cargo は PATH の先頭に置いた偽の `cargo` で置き換える
（本物の build はしない）。
"""

from __future__ import annotations

import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import textwrap
import time
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
CASES = HERE / "cases.py"
MUTATE = HERE / "mutate.sh"
CAPMAP = "agent-docs/progress/2026-11-07-health-v2-b0/capability-map.json"
REMOVALS = "agent-docs/progress/2026-11-10-health-v2-r5/removals"

GIT_ENV = {
    "GIT_AUTHOR_NAME": "fixture",
    "GIT_AUTHOR_EMAIL": "fixture@example.invalid",
    "GIT_COMMITTER_NAME": "fixture",
    "GIT_COMMITTER_EMAIL": "fixture@example.invalid",
    "GIT_CONFIG_NOSYSTEM": "1",
}

LIB_RS = """\
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

// #[test] fn in_comment() {}
pub const FIXTURE: &str = r#"
#[test]
fn in_raw_string() {}
"#;

mod guard;

#[cfg(test)]
mod inline {
    use super::*;

    #[test]
    fn adds_small() {
        assert_eq!(add(1, 2), 3);
    }

    #[tokio::test(
        flavor = "multi_thread"
    )]
    #[ignore]
    async fn adds_async() {
        let s = "#[test] fn in_string() {}";
        assert!(!s.is_empty());
    }

    #[test]
    fn table() {
        for (a, b, want) in [(1, 1, 2), (2, 2, 4), (0, 0, 0)] {
            assert_eq!(add(a, b), want);
        }
        let cases = vec![(1, 2), (3, 4)];
        for (a, b) in cases {
            assert!(add(a, b) > 0);
        }
    }
}
"""

GUARD_RS = """\
pub fn check() -> bool {
    true
}

#[cfg(test)]
#[path = "guard_tests.rs"]
mod tests;
"""

GUARD_TESTS_RS = """\
use super::check;

#[test]
fn protected_invariant() {
    assert!(check());
}
"""

API_RS = """\
mod common;

#[test]
fn api_one() {
    common::helper();
}

#[test]
fn api_two() {
    common::helper();
}

macro_rules! point_test {
    ($name:ident, $p:expr) => {
        #[test]
        fn $name() {
            assert!($p > 0);
        }
    };
}
point_test!(point_a, 1);
point_test!(point_b, 2);
"""

COMMON_RS = "pub fn helper() {}\n"


def run(cmd, cwd, env=None, check=True, **kw):
    e = dict(os.environ)
    e.update(GIT_ENV)
    if env:
        e.update(env)
    p = subprocess.run(cmd, cwd=cwd, env=e, capture_output=True, text=True, **kw)
    if check and p.returncode != 0:
        raise AssertionError(f"{cmd} → {p.returncode}\n{p.stdout}\n{p.stderr}")
    return p


def git(repo, *args):
    return run(["git", "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false", *args], cwd=repo).stdout


class Fixture:
    """crates/foo の小さい repo。base の commit を作る。"""

    def __init__(self):
        self.dir = Path(tempfile.mkdtemp(prefix="r5-guard-"))
        self.repo = self.dir / "repo"
        self.repo.mkdir()
        git(self.repo, "init", "-q", "-b", "main")
        self.write("crates/foo/Cargo.toml", '[package]\nname = "foo-pkg"\nversion = "0.1.0"\n')
        self.write("crates/foo/src/lib.rs", LIB_RS)
        self.write("crates/foo/src/guard.rs", GUARD_RS)
        self.write("crates/foo/src/guard_tests.rs", GUARD_TESTS_RS)
        self.write("crates/foo/tests/api.rs", API_RS)
        self.write("crates/foo/tests/common/mod.rs", COMMON_RS)
        capmap = {
            "capabilities": [
                {
                    "id": "foo-cap",
                    "invariants": [{"kind": "fail_closed", "text": "x", "evidence_path": "crates/foo/src/guard.rs"}],
                    "tests": ["crates/foo/tests/api.rs"],
                }
            ]
        }
        self.write(CAPMAP, json.dumps(capmap))
        self.write("mut/kill.patch", "placeholder\n")
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-q", "-m", "base")
        self.base = git(self.repo, "rev-parse", "HEAD").strip()

    def write(self, rel, text):
        p = self.repo / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text)

    def read(self, rel):
        return (self.repo / rel).read_text()

    def edit(self, rel, old, new):
        s = self.read(rel)
        assert old in s, (rel, old)
        self.write(rel, s.replace(old, new, 1))

    def cases(self, crate="foo", *extra):
        return run([sys.executable, str(CASES), "--crate", crate, "--base", self.base, "--json", *extra], cwd=self.repo, check=False)

    def removals(self, entries):
        self.write(f"{REMOVALS}/foo.json", json.dumps({"crate": "foo", "removals": entries}))

    def close(self):
        shutil.rmtree(self.dir, ignore_errors=True)


def entry(name, **kw):
    e = {
        "name": name,
        "reason": "同じ契約を api_one が見る",
        "replacement": ["api_one"],
        "mutation": {"patch": "mut/kill.patch", "filter": "test(api_one)", "result": "killed"},
    }
    e.update(kw)
    return e


class CasesTest(unittest.TestCase):
    def setUp(self):
        self.fx = Fixture()

    def tearDown(self):
        self.fx.close()

    def result(self, p):
        self.assertIn(p.returncode, (0, 1), p.stderr)
        return json.loads(p.stdout)

    def test_unchanged_worktree_is_zero(self):
        p = self.fx.cases()
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        r = json.loads(p.stdout)
        # adds_small・adds_async・table・protected_invariant・api_one・api_two・point_a・point_b
        self.assertEqual(r["base_functions"], 8)
        self.assertEqual(r["worktree_functions"], 8)
        # table: [..] の 3 行 + vec![..] の 2 行
        self.assertEqual(r["base_case_rows"], 5)
        # guard.rs（evidence）と、それが #[cfg(test)] #[path] mod tests; で読む guard_tests.rs、tests/api.rs（tests）
        self.assertEqual(sorted(r["protected_paths"]), ["crates/foo/src/guard.rs", "crates/foo/src/guard_tests.rs", "crates/foo/tests/api.rs"])
        self.assertEqual(r["protected_functions"], 5)

    def test_package_name_resolves_to_dir(self):
        p = self.fx.cases("foo-pkg")
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertEqual(json.loads(p.stdout)["crate"], "foo")

    def test_bundling_move_is_not_a_removal(self):
        # tests/api.rs を tests/bundle/main.rs + mod api; に移す（module path だけが変わる）
        (self.fx.repo / "crates/foo/tests/api.rs").unlink()
        self.fx.write("crates/foo/tests/bundle/main.rs", "mod api;\n")
        self.fx.write("crates/foo/tests/bundle/api.rs", API_RS.replace("mod common;", "use super::common;"))
        p = self.fx.cases()
        r = self.result(p)
        self.assertEqual(p.returncode, 0, r["errors"])
        self.assertGreaterEqual(r["moved_by_name"], 4)

    def test_removed_function_without_entry_fails(self):
        self.fx.edit("crates/foo/src/lib.rs", "    #[test]\n    fn adds_small() {", "    fn adds_small() {")
        p = self.fx.cases()
        r = self.result(p)
        self.assertEqual(p.returncode, 1)
        self.assertEqual(r["removed_functions"], {"adds_small": 1})
        self.assertTrue(any("adds_small" in e for e in r["errors"]), r["errors"])

    def test_removed_function_with_valid_entry_passes(self):
        self.fx.edit("crates/foo/src/lib.rs", "    #[test]\n    fn adds_small() {", "    fn adds_small() {")
        self.fx.removals([entry("adds_small")])
        p = self.fx.cases()
        self.assertEqual(p.returncode, 0, p.stdout)

    def test_invalid_entries_fail(self):
        self.fx.edit("crates/foo/src/lib.rs", "    #[test]\n    fn adds_small() {", "    fn adds_small() {")
        bad = {
            "no reason": entry("adds_small", reason=""),
            "missing replacement": entry("adds_small", replacement=["no_such_test"]),
            "survived": entry("adds_small", mutation={"patch": "mut/kill.patch", "filter": "test(x)", "result": "survived"}),
            "no patch": entry("adds_small", mutation={"patch": "mut/none.patch", "filter": "test(x)", "result": "killed"}),
            "no filter": entry("adds_small", mutation={"patch": "mut/kill.patch", "result": "killed"}),
        }
        for label, e in bad.items():
            with self.subTest(label):
                self.fx.removals([e])
                p = self.fx.cases()
                self.assertEqual(p.returncode, 1, (label, p.stdout))

    def test_removed_table_rows_need_cases_entry(self):
        self.fx.edit("crates/foo/src/lib.rs", "[(1, 1, 2), (2, 2, 4), (0, 0, 0)]", "[(1, 1, 2)]")
        p = self.fx.cases()
        r = self.result(p)
        self.assertEqual(p.returncode, 1)
        self.assertEqual(r["removed_case_rows"], {"table": 2})
        self.fx.removals([entry("table", cases=1)])
        self.assertEqual(self.fx.cases().returncode, 1)
        self.fx.removals([entry("table", cases=2)])
        p = self.fx.cases()
        self.assertEqual(p.returncode, 0, p.stdout)

    def test_protected_function_cannot_be_removed_even_with_entry(self):
        # capability-map の evidence（guard.rs）が #[cfg(test)] #[path] mod tests; で読む file の試験
        self.fx.edit("crates/foo/src/guard_tests.rs", "#[test]\nfn protected_invariant", "fn protected_invariant")
        self.fx.removals([entry("protected_invariant")])
        p = self.fx.cases()
        r = self.result(p)
        self.assertEqual(p.returncode, 1)
        self.assertEqual([x["name"] for x in r["protected_lost"]], ["protected_invariant"])
        # capability-map の tests の file の試験（macro で作る試験も含む）
        self.fx.write("crates/foo/src/guard_tests.rs", GUARD_TESTS_RS)
        self.fx.edit("crates/foo/tests/api.rs", "point_test!(point_b, 2);\n", "")
        self.fx.removals([entry("point_b")])
        p = self.fx.cases()
        r = self.result(p)
        self.assertEqual(p.returncode, 1)
        self.assertEqual([x["name"] for x in r["protected_lost"]], ["point_b"])

    def test_protected_file_moved_by_name_passes(self):
        (self.fx.repo / "crates/foo/tests/api.rs").unlink()
        self.fx.write("crates/foo/tests/all.rs", "mod common;\nmod moved {\n" + API_RS.replace("mod common;", "use super::common;") + "}\n")
        p = self.fx.cases()
        self.assertEqual(p.returncode, 0, p.stdout)

    def test_bad_base_is_usage_error(self):
        p = run([sys.executable, str(CASES), "--crate", "foo", "--base", "no-such-rev"], cwd=self.fx.repo, check=False)
        self.assertEqual(p.returncode, 2)
        p = run([sys.executable, str(CASES), "--crate", "nope", "--base", self.fx.base], cwd=self.fx.repo, check=False)
        self.assertEqual(p.returncode, 2)


FAKE_CARGO = """\
#!/bin/sh
# 偽の cargo: 引数と、その時点の対象 file の内容を記録し、FAKE_EXIT で終わる
printf '%s\\n' "$*" > "$FAKE_LOG.args"
cat crates/foo/src/lib.rs > "$FAKE_LOG.seen"
if [ -n "${FAKE_SLEEP:-}" ]; then
    echo $$ > "$FAKE_LOG.pid"
    exec sleep 60
fi
exit "${FAKE_EXIT:-0}"
"""


class MutateTest(unittest.TestCase):
    def setUp(self):
        self.fx = Fixture()
        self.bin = self.fx.dir / "bin"
        self.bin.mkdir()
        (self.bin / "cargo").write_text(FAKE_CARGO)
        (self.bin / "cargo").chmod(0o755)
        self.log = str(self.fx.dir / "fake")
        self.orig = self.fx.read("crates/foo/src/lib.rs")
        self.fx.edit("crates/foo/src/lib.rs", "a + b", "a - b")
        self.fx.edit("crates/foo/src/lib.rs", "pub fn add", "pub fn add_new_name_unused() {}\npub fn add")
        self.patch = self.fx.dir / "m.patch"
        self.patch.write_text(git(self.fx.repo, "diff"))
        git(self.fx.repo, "checkout", "--", ".")
        self.assertEqual(self.fx.read("crates/foo/src/lib.rs"), self.orig)

    def tearDown(self):
        self.fx.close()

    def env(self, **kw):
        e = {"PATH": f"{self.bin}:{os.environ['PATH']}", "FAKE_LOG": self.log}
        e.update(kw)
        return e

    def mutate(self, **env):
        return run(["sh", str(MUTATE), str(self.patch), "foo-pkg", "test(adds_small)"], cwd=self.fx.repo / "crates/foo", env=self.env(**env), check=False, timeout=60)

    def assert_restored(self):
        self.assertEqual(self.fx.read("crates/foo/src/lib.rs"), self.orig)
        self.assertEqual(git(self.fx.repo, "status", "--porcelain", "--untracked-files=no"), "")

    def test_failing_tests_mean_killed_and_patch_is_reverted(self):
        p = self.mutate(FAKE_EXIT="100")
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertIn("KILLED", p.stdout)
        self.assertEqual(Path(self.log + ".args").read_text().strip(), "nextest run -p foo-pkg -E test(adds_small)")
        self.assertIn("a - b", Path(self.log + ".seen").read_text())  # 試験は patch を当てた木で走った
        self.assert_restored()

    def test_passing_tests_mean_survived_and_patch_is_reverted(self):
        p = self.mutate(FAKE_EXIT="0")
        self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
        self.assertIn("SURVIVED", p.stdout)
        self.assert_restored()

    def test_build_failure_is_error_not_killed(self):
        for code in ("101", "4", "94"):
            with self.subTest(code):
                p = self.mutate(FAKE_EXIT=code)
                self.assertEqual(p.returncode, 2, p.stdout + p.stderr)
                self.assertNotIn("KILLED", p.stdout)
                self.assert_restored()

    def test_patch_that_does_not_apply_changes_nothing(self):
        self.fx.edit("crates/foo/src/lib.rs", "a + b", "b + a")
        before = self.fx.read("crates/foo/src/lib.rs")
        p = self.mutate(FAKE_EXIT="100")
        self.assertEqual(p.returncode, 2, p.stdout + p.stderr)
        self.assertEqual(self.fx.read("crates/foo/src/lib.rs"), before)
        self.assertFalse(Path(self.log + ".args").exists())

    def test_interrupt_reverts_patch(self):
        for sig, want in ((signal.SIGTERM, 143), (signal.SIGINT, 130), (signal.SIGHUP, 129)):
            with self.subTest(sig.name):
                pid_file = Path(self.log + ".pid")
                pid_file.unlink(missing_ok=True)
                e = dict(os.environ)
                e.update(GIT_ENV)
                e.update(self.env(FAKE_SLEEP="1"))
                proc = subprocess.Popen(
                    ["sh", str(MUTATE), str(self.patch), "foo-pkg", "test(adds_small)"],
                    cwd=self.fx.repo, env=e, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                )
                try:
                    deadline = time.monotonic() + 30
                    while not (pid_file.exists() and pid_file.read_text().strip()):  # 偽の cargo が走り出すのを待つ
                        self.assertIsNone(proc.poll(), "mutate.sh が先に終わった")
                        self.assertLess(time.monotonic(), deadline)
                        time.sleep(0.05)
                    self.assertIn("a - b", self.fx.read("crates/foo/src/lib.rs"))  # 中断の前は当たっている
                    child = int(pid_file.read_text())
                    proc.send_signal(sig)
                    out, err = proc.communicate(timeout=30)
                finally:
                    if proc.poll() is None:
                        proc.kill()
                        proc.wait()
                self.assertEqual(proc.returncode, want, out + err)
                self.assert_restored()
                with self.assertRaises(ProcessLookupError):  # 試験の process も止めた
                    os.kill(child, 0)

    def test_usage_errors(self):
        p = run(["sh", str(MUTATE), str(self.patch), "foo-pkg"], cwd=self.fx.repo, env=self.env(), check=False)
        self.assertEqual(p.returncode, 2)
        p = run(["sh", str(MUTATE), "no-such.patch", "foo-pkg", "test(x)"], cwd=self.fx.repo, env=self.env(), check=False)
        self.assertEqual(p.returncode, 2)


if __name__ == "__main__":
    unittest.main()
