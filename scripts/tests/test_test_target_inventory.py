"""Tests for scripts/dev/test-target-inventory.py (test targets vs tests/ support files)."""
import importlib.util
import io
import json
import shutil
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

HERE = Path(__file__).resolve().parent
SOURCE = HERE.parents[1] / "scripts/dev/test-target-inventory.py"
FIXTURE = HERE / "fixtures/test-target-inventory/metadata.json"
spec = importlib.util.spec_from_file_location("test_target_inventory", SOURCE)
tti = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = tti
spec.loader.exec_module(tti)

# Files on disk under alpha/tests: three are targets, the rest are support modules.
ALPHA_TESTS = {
    "api.rs": "mod common;\n#[test] fn a() {}\n",
    "store.rs": "#[path = \"support/db.rs\"] mod db;\n",
    "nested/main.rs": "mod helper;\n",
    "nested/helper.rs": "",
    "common/mod.rs": "pub fn setup() {}\n",
    "support/db.rs": "",
    "data/case.json": "{}",
}


class InventoryTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="tti-test-"))
        self.addCleanup(shutil.rmtree, self.root)
        for rel, body in ALPHA_TESTS.items():
            p = self.root / "crates/alpha/tests" / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(body)
        # A tests/ dir inside target/ must not be counted.
        stray = self.root / "crates/alpha/tests/target/debug/x.rs"
        stray.parent.mkdir(parents=True)
        stray.write_text("")
        (self.root / "crates/beta/src").mkdir(parents=True)
        self.meta_path = self.root / "metadata.json"
        self.meta_path.write_text(FIXTURE.read_text().replace("@ROOT@", str(self.root)))
        self.report = tti.build_report(json.loads(self.meta_path.read_text()), self.root)
        self.crates = {c["name"]: c for c in self.report["crates"]}

    def test_only_workspace_members(self):
        self.assertEqual(sorted(self.crates), ["alpha", "beta"])

    def test_target_kinds_counted(self):
        a = self.crates["alpha"]["targets"]
        self.assertEqual((a["lib"], a["bin"], a["test"], a["bench"], a["example"]), (1, 1, 3, 1, 1))
        b = self.crates["beta"]["targets"]
        self.assertEqual((b["proc-macro"], b["custom-build"], b["lib"]), (1, 1, 0))

    def test_support_files_are_not_targets(self):
        td = self.crates["alpha"]["tests_dir"]
        self.assertEqual(td["rs_files"], 6)
        self.assertEqual(td["target_files"], 3)
        self.assertEqual(td["support_files"], 3)
        self.assertEqual(
            sorted(td["support_file_paths"]), ["common/mod.rs", "nested/helper.rs", "support/db.rs"]
        )
        self.assertEqual(td["non_rs_files"], 1)
        self.assertEqual(td["top_level_rs_files"], 2)

    def test_nested_main_is_a_target_not_support(self):
        a = self.crates["alpha"]
        self.assertEqual(a["test_targets_by_location"], {"tests_top": 2, "tests_nested": 1})
        self.assertNotIn("nested/main.rs", a["tests_dir"]["support_file_paths"])

    def test_naive_count_difference(self):
        self.assertEqual(self.crates["alpha"]["naive_minus_targets"], 3)
        self.assertEqual(self.report["totals"]["naive_minus_targets"], 3)

    def test_expected_test_binaries_skip_examples_benches_build_scripts(self):
        # alpha: lib + bin + 3 tests; beta: proc-macro only.
        self.assertEqual(self.crates["alpha"]["expected_test_binaries"], 5)
        self.assertEqual(self.crates["beta"]["expected_test_binaries"], 1)
        self.assertEqual(self.report["totals"]["expected_test_binaries"], 6)

    def test_crate_without_tests_dir(self):
        td = self.crates["beta"]["tests_dir"]
        self.assertEqual((td["rs_files"], td["support_files"]), (0, 0))
        self.assertEqual(self.crates["beta"]["test_targets"], 0)

    def test_cli_json(self):
        buf = io.StringIO()
        with redirect_stdout(buf):
            rc = tti.main(["--json", "--metadata", str(self.meta_path)])
        self.assertEqual(rc, 0)
        out = json.loads(buf.getvalue())
        self.assertEqual(out["schema"], "celeris.test-target-inventory/1")
        self.assertEqual(out["totals"]["test_targets"], 3)
        self.assertEqual(out["crates"][0]["manifest_dir"], "crates/alpha")

    def test_cli_requires_json(self):
        with self.assertRaises(SystemExit):
            with redirect_stdout(io.StringIO()):
                tti.main(["--metadata", str(self.meta_path)])


if __name__ == "__main__":
    unittest.main()
