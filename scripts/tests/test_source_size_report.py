"""Tests for scripts/dev/source-size-report.py (ADR-0083)."""
import importlib.util
import os
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

SOURCE = Path(__file__).resolve().parents[2] / "scripts/dev/source-size-report.py"
spec = importlib.util.spec_from_file_location("source_size_report", SOURCE)
ssr = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = ssr  # dataclass field-type resolution needs this in sys.modules
spec.loader.exec_module(ssr)


def run_git(root, *args):
    return subprocess.run(
        ["git", "-C", str(root), *args], capture_output=True, text=True, check=True
    )


def make_repo() -> Path:
    tmp = Path(tempfile.mkdtemp(prefix="ssr-test-"))
    run_git(tmp, "init", "-q")
    run_git(tmp, "config", "user.email", "test@example.com")
    run_git(tmp, "config", "user.name", "Test")
    return tmp


def write(root: Path, rel: str, content: str) -> Path:
    p = root / rel
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(content)
    return p


def commit_all(root: Path):
    run_git(root, "add", "-A")
    run_git(root, "commit", "-q", "-m", "snapshot")


class ClassificationTests(unittest.TestCase):
    def test_generated_types_ts_excluded_from_production(self):
        self.assertEqual(ssr.classify_path("gui/app/celeris/types.ts"), "generated")

    def test_generated_dir_pattern(self):
        self.assertEqual(ssr.classify_path("web/api/generated/schema.ts"), "generated")

    def test_schema_json(self):
        self.assertEqual(ssr.classify_path("docs/api/v1/health.schema.json"), "schema")

    def test_lockfile(self):
        self.assertEqual(ssr.classify_path("gui/pnpm-lock.yaml"), "lockfile")
        self.assertEqual(ssr.classify_path("Cargo.lock"), "lockfile")

    def test_migrations(self):
        self.assertEqual(ssr.classify_path("crates/task-core/migrations/0001_init.sql"), "migrations")

    def test_fixture(self):
        self.assertEqual(ssr.classify_path("crates/task-worker/fixtures/foo.json"), "fixture")

    def test_docs(self):
        self.assertEqual(ssr.classify_path("docs/adr/0083-foo.md"), "docs")
        self.assertEqual(ssr.classify_path("README.md"), "docs")

    def test_tooling_script(self):
        self.assertEqual(ssr.classify_path("scripts/dev/source-size-report.py"), "tooling")
        self.assertEqual(ssr.classify_path("web/scripts/gen-types.mjs"), "tooling")

    def test_plain_rust_and_ts(self):
        self.assertEqual(ssr.classify_path("crates/task-core/src/store.rs"), "rust")
        self.assertEqual(ssr.classify_path("web/features/tasks/list.tsx"), "ts")

    def test_external_rust_test_paths(self):
        self.assertTrue(ssr.is_external_rust_test("crates/task-core/tests/e2e.rs"))
        self.assertTrue(ssr.is_external_rust_test("crates/task-dispatch/src/dispatcher/tests.rs"))
        self.assertTrue(ssr.is_external_rust_test("crates/foo/src/bar_tests.rs"))
        self.assertFalse(ssr.is_external_rust_test("crates/task-core/src/store.rs"))

    def test_ts_test_paths_cover_web_and_gui(self):
        self.assertTrue(ssr.is_ts_test("gui/e2e/tasks.spec.ts"))
        self.assertTrue(ssr.is_ts_test("web/e2e/tasks.spec.ts"))
        self.assertTrue(ssr.is_ts_test("web/features/tasks/list.test.tsx"))
        self.assertFalse(ssr.is_ts_test("web/features/tasks/list.tsx"))


class InlineTestExtentTests(unittest.TestCase):
    def test_inline_mod_block_is_subtracted_from_production(self):
        lines = (
            ["pub fn a() {", "    1;", "}", "", "#[cfg(test)]", "mod tests {", "    fn t() {}", "}"]
        )
        info = ssr.analyze_rust_file(lines)
        self.assertEqual(info.prod_lines, 4)  # everything except the 4-line cfg(test) mod block
        self.assertEqual(info.inline_test_lines, 4)
        self.assertEqual(info.inline_test_mods, [{"name": "tests", "start": 5, "end": 8, "lines": 4}])

    def test_cfg_test_helper_fn_outside_mod_block_still_counts_as_test(self):
        lines = ["pub fn a() {}", "", "#[cfg(test)]", "pub fn only_used_in_tests() {", "    1;", "}"]
        info = ssr.analyze_rust_file(lines)
        self.assertEqual(info.prod_lines, 2)
        self.assertEqual(info.inline_test_lines, 4)

    def test_external_mod_tests_declaration_is_not_inline(self):
        lines = ["pub fn a() {}", "", "#[cfg(test)]", "mod tests;"]
        info = ssr.analyze_rust_file(lines)
        self.assertEqual(info.prod_lines, 4)
        self.assertEqual(info.inline_test_lines, 0)
        self.assertEqual(info.external_mod_decls, ["tests"])

    def test_string_containing_brace_does_not_confuse_block_end(self):
        lines = [
            "#[cfg(test)]",
            "mod tests {",
            '    const S: &str = "}";',
            "    fn t() {}",
            "}",
        ]
        info = ssr.analyze_rust_file(lines)
        self.assertEqual(info.inline_test_mods[0]["end"], 5)


class ModResolutionTests(unittest.TestCase):
    def test_non_mod_rs_file_resolves_into_sibling_directory(self):
        candidates = ssr.resolve_mod_candidates("crates/task-dispatch/src/dispatcher.rs", "child_tasks")
        self.assertIn("crates/task-dispatch/src/dispatcher/child_tasks.rs", candidates)
        self.assertIn("crates/task-dispatch/src/dispatcher/child_tasks/mod.rs", candidates)

    def test_lib_rs_resolves_into_src_directory(self):
        candidates = ssr.resolve_mod_candidates("crates/celeris/src/lib.rs", "config")
        self.assertIn("crates/celeris/src/config.rs", candidates)
        self.assertIn("crates/celeris/src/config/mod.rs", candidates)


class ReportEndToEndTests(unittest.TestCase):
    def test_oversize_production_file_warns_by_default_but_exits_zero(self):
        repo = make_repo()
        big = "\n".join(f"// line {i}" for i in range(2500))
        write(repo, "crates/x/src/lib.rs", big)
        commit_all(repo)
        report = ssr.run_report(str(repo), prod_threshold=2000, inline_threshold=300, exceptions=[])
        self.assertEqual(report["warning_count"], 1)
        self.assertEqual(report["warnings"][0]["check"], ssr.CHECK_PRODUCTION_SIZE)

    def test_exception_with_reason_suppresses_warning(self):
        repo = make_repo()
        big = "\n".join(f"// line {i}" for i in range(2500))
        write(repo, "crates/x/src/lib.rs", big)
        commit_all(repo)
        exc = ssr.Exception_(path="crates/x/src/lib.rs", reason="known cohesive bag of types", checks=None)
        report = ssr.run_report(str(repo), prod_threshold=2000, inline_threshold=300, exceptions=[exc])
        self.assertEqual(report["warning_count"], 0)
        self.assertEqual(report["excepted_count"], 1)
        self.assertEqual(report["warnings"][0]["excepted"], "known cohesive bag of types")

    def test_config_rejects_exception_without_reason(self):
        repo = make_repo()
        write(repo, "scripts/dev/source-size-report.toml", textwrap.dedent(
            """
            [[exceptions]]
            path = "crates/x/src/lib.rs"
            reason = ""
            """
        ))
        with self.assertRaises(ValueError):
            ssr.load_config(None, str(repo))

    def test_untracked_gitignored_mod_target_warns(self):
        repo = make_repo()
        # foo.rs is not a crate root, so `mod local_only;` resolves into foo/local_only.rs.
        write(repo, ".gitignore", "crates/x/src/foo/local_only.rs\n")
        write(repo, "crates/x/src/foo.rs", "mod local_only;\n")
        commit_all(repo)
        # exists on disk, matches .gitignore, was never `git add`ed.
        write(repo, "crates/x/src/foo/local_only.rs", "pub fn f() {}\n")
        report = ssr.run_report(str(repo), prod_threshold=2000, inline_threshold=300, exceptions=[])
        checks = [w["check"] for w in report["warnings"]]
        self.assertIn(ssr.CHECK_UNTRACKED_MOD, checks)

    def test_size_exception_without_explicit_checks_does_not_cover_the_mod_check(self):
        repo = make_repo()
        write(repo, ".gitignore", "crates/x/src/foo/local_only.rs\n")
        write(repo, "crates/x/src/foo.rs", "mod local_only;\n")
        commit_all(repo)
        write(repo, "crates/x/src/foo/local_only.rs", "pub fn f() {}\n")
        exc = ssr.Exception_(path="crates/x/src/foo.rs", reason="big on purpose", checks=None)
        report = ssr.run_report(str(repo), prod_threshold=2000, inline_threshold=300, exceptions=[exc])
        mod_warnings = [w for w in report["warnings"] if w["check"] == ssr.CHECK_UNTRACKED_MOD]
        self.assertEqual(len(mod_warnings), 1)
        self.assertIsNone(mod_warnings[0]["excepted"])

    def test_untracked_but_not_gitignored_mod_target_does_not_warn(self):
        repo = make_repo()
        write(repo, "crates/x/src/foo.rs", "mod local_only;\n")
        commit_all(repo)
        # exists on disk, not yet `git add`ed, but nothing ignores it (ordinary WIP file).
        write(repo, "crates/x/src/foo/local_only.rs", "pub fn f() {}\n")
        report = ssr.run_report(str(repo), prod_threshold=2000, inline_threshold=300, exceptions=[])
        checks = [w["check"] for w in report["warnings"]]
        self.assertNotIn(ssr.CHECK_UNTRACKED_MOD, checks)

    def test_generated_ts_file_not_counted_toward_production_even_when_huge(self):
        repo = make_repo()
        big = "\n".join(f"// {i}" for i in range(9000))
        write(repo, "gui/app/celeris/types.ts", big)
        commit_all(repo)
        report = ssr.run_report(str(repo), prod_threshold=2000, inline_threshold=300, exceptions=[])
        self.assertEqual(report["warning_count"], 0)
        self.assertNotIn("ts_production", report["totals"])
        self.assertEqual(report["totals"]["generated"]["lines"], 9000)


class CliTests(unittest.TestCase):
    def test_default_exit_zero_with_warnings(self):
        repo = make_repo()
        big = "\n".join(f"// {i}" for i in range(2500))
        write(repo, "crates/x/src/lib.rs", big)
        commit_all(repo)
        proc = subprocess.run(
            [sys.executable, str(SOURCE), "--root", str(repo)], capture_output=True, text=True
        )
        self.assertEqual(proc.returncode, 0)
        self.assertIn("production_size", proc.stdout)

    def test_strict_exit_nonzero_with_warnings(self):
        repo = make_repo()
        big = "\n".join(f"// {i}" for i in range(2500))
        write(repo, "crates/x/src/lib.rs", big)
        commit_all(repo)
        proc = subprocess.run(
            [sys.executable, str(SOURCE), "--root", str(repo), "--strict"],
            capture_output=True,
            text=True,
        )
        self.assertEqual(proc.returncode, 1)

    def test_strict_exit_zero_with_no_warnings(self):
        repo = make_repo()
        write(repo, "crates/x/src/lib.rs", "pub fn f() {}\n")
        commit_all(repo)
        proc = subprocess.run(
            [sys.executable, str(SOURCE), "--root", str(repo), "--strict"],
            capture_output=True,
            text=True,
        )
        self.assertEqual(proc.returncode, 0)

    def test_json_format_is_valid_json(self):
        repo = make_repo()
        write(repo, "crates/x/src/lib.rs", "pub fn f() {}\n")
        commit_all(repo)
        proc = subprocess.run(
            [sys.executable, str(SOURCE), "--root", str(repo), "--format", "json"],
            capture_output=True,
            text=True,
        )
        self.assertEqual(proc.returncode, 0)
        import json

        parsed = json.loads(proc.stdout)
        self.assertIn("totals", parsed)


if __name__ == "__main__":
    unittest.main()
