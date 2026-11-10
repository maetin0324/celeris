#!/usr/bin/env python3
"""Unit tests of classify.py's rule 2 (r6: the target is named by docs/architecture-map.md).

Invariant: a test target whose path a table row of the map names, in any form that
scripts/dev/check-architecture-map.py collects (a brace list `a/{b,c}.rs`, a relative path in
`（…）` right after a path, a relative Markdown link), is r6 unless rule 1 keeps it. Moving such a
target makes the map checker report wrong_path (the health gate's arch-map.wrong_path), and the
map is outside the R5 write set.

Why a new test: the earlier search looked for the full path (or the path without `crates/` in
backticks) as a substring of each map line. It does not see a path that only the collector
builds, so 19 targets named through brace lists were put in bundle / adjust leaves and the
inventory's own --check agreed (it compared against the same search).
test_legacy_substring_search_misses_brace_and_paren keeps that blind spot on record.

Run from the repository root:
  python3 agent-docs/progress/2026-11-10-health-v2-r5/test_classify.py
"""
from __future__ import annotations

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import classify  # noqa: E402

HEADER = (
    "| subsystem | owner crate/module | entry point | ADR / 設計 | 関連試験 | 不変条件 |\n"
    "|---|---|---|---|---|---|\n"
)
# Brace list (the celerisctl row of the real map has this shape).
BRACE_ROW = ("| cli | celerisctl | `crates/celerisctl/src/main.rs` | - | "
             "`crates/celerisctl/tests/{no_migrate,cli_resolution,bin_smoke}.rs` | 不変 |\n")
# Relative paths in （…） after a path, one of them a brace list.
PAREN_ROW = ("| task | task-api | `crates/task-api/src/handlers.rs` | - | "
             "`crates/task-api/tests/lifecycle.rs`（`task_tree.rs`, `{reports,organization}.rs`） | 不変 |\n")
# A relative Markdown link from docs/ (resolved against the map's directory).
LINK_ROW = ("| docs | task-api | `crates/task-api/src/docs.rs` | - | "
            "[試験](../crates/task-api/tests/docs.rs), `crates/task-api/tests/lifecycle.rs` | 不変 |\n")
SYNTHETIC_MAP = HEADER + BRACE_ROW + PAREN_ROW + LINK_ROW

BRACE_TARGETS = ["crates/celerisctl/tests/no_migrate.rs", "crates/celerisctl/tests/cli_resolution.rs",
                 "crates/celerisctl/tests/bin_smoke.rs"]
PAREN_TARGETS = ["crates/task-api/tests/task_tree.rs", "crates/task-api/tests/reports.rs",
                 "crates/task-api/tests/organization.rs"]
UNNAMED = "crates/task-api/tests/approvals.rs"


class FakeTree:
    """The parts of classify.Tree that external_refs reads: the map, .md files, scripts/, nextest."""

    def __init__(self, files: dict[str, str]):
        self.files = files
        self.blobs = {p: f"sha-{i}" for i, p in enumerate(files)}

    def read(self, path: str) -> str:
        return self.files.get(path, "")

    def read_many(self, paths: list[str]) -> None:
        pass


def target(path: str) -> dict:
    crate_dir, name = path.split("/tests/")
    return {"crate": crate_dir.split("/")[-1], "crate_dir": crate_dir, "target": name[:-3], "path": path}


def record(t: dict, refs: dict, **over) -> dict:
    """A record with no rule 1 reason: rule 2 or a bundle leaf decides."""
    rec = {"path": t["path"], "crate": t["crate"], "target": t["target"], "process_global": {},
           "external_refs": refs, "digit_in_name": False, "shares_source_with": [],
           "conflicts": {k: {"conflict": False} for k in classify.R_SLICES}}
    rec.update(over)
    return rec


def legacy_hits(amap: str, path: str) -> list[int]:
    """The search classify.py used before this fix (substring of each map line)."""
    tail = path.split("/", 1)[1] if path.startswith("crates/") else path
    return [i for i, ln in enumerate(amap.splitlines(), 1) if path in ln or f"`{tail}`" in ln]


class ArchMapRule(unittest.TestCase):
    def setUp(self):
        self.targets = [target(p) for p in BRACE_TARGETS + PAREN_TARGETS
                        + ["crates/task-api/tests/docs.rs", "crates/task-api/tests/lifecycle.rs", UNNAMED]]
        tree = FakeTree({classify.ARCHMAP: SYNTHETIC_MAP, classify.NEXTEST: ""})
        self.refs = classify.external_refs(tree, self.targets)
        self.by_path = {t["path"]: t for t in self.targets}

    def test_brace_list_targets_are_r6(self):
        for p in BRACE_TARGETS:
            with self.subTest(p):
                self.assertEqual(self.refs[p]["architecture_map"], [f"{classify.ARCHMAP}:3"])
                leaf, reasons = classify.assign(record(self.by_path[p], self.refs[p]))
                self.assertEqual((leaf, reasons), ("r6", ["architecture_map"]))

    def test_paren_relative_targets_are_r6(self):
        for p in PAREN_TARGETS:
            with self.subTest(p):
                self.assertEqual(self.refs[p]["architecture_map"], [f"{classify.ARCHMAP}:4"])
                self.assertEqual(classify.assign(record(self.by_path[p], self.refs[p]))[0], "r6")

    def test_link_and_plain_paths_keep_every_line(self):
        self.assertEqual(self.refs["crates/task-api/tests/docs.rs"]["architecture_map"], [f"{classify.ARCHMAP}:5"])
        self.assertEqual(self.refs["crates/task-api/tests/lifecycle.rs"]["architecture_map"],
                         [f"{classify.ARCHMAP}:4", f"{classify.ARCHMAP}:5"])

    def test_unnamed_target_goes_to_its_bundle_leaf(self):
        self.assertEqual(self.refs[UNNAMED]["architecture_map"], [])
        self.assertEqual(classify.assign(record(self.by_path[UNNAMED], self.refs[UNNAMED]))[0], "bundle-api-b")

    def test_rule_one_still_wins_over_the_map(self):
        p = BRACE_TARGETS[0]
        leaf, reasons = classify.assign(record(self.by_path[p], self.refs[p], digit_in_name=True))
        self.assertEqual((leaf, reasons), ("keep", ["digit_in_name_outside_write_set"]))

    def test_legacy_substring_search_misses_brace_and_paren(self):
        # The old search finds none of these, so its --check could not flag them.
        for p in BRACE_TARGETS + PAREN_TARGETS:
            with self.subTest(p):
                self.assertEqual(legacy_hits(SYNTHETIC_MAP, p), [])
                self.assertNotEqual(self.refs[p]["architecture_map"], [])


class CheckMapRule(unittest.TestCase):
    """--check's own rule: re-collect the map and require r6 for named targets rule 1 does not keep."""

    def setUp(self):
        self.named = classify.archmap_paths(SYNTHETIC_MAP)
        self.refs = {"architecture_map": [], "md_links": [], "scripts": [], "nextest": []}

    def rec(self, path: str, leaf: str, reasons: list[str], **over) -> dict:
        return {**record(target(path), self.refs, **over), "leaf": leaf, "leaf_reasons": reasons}

    def test_bundle_leaf_for_a_named_target_is_an_error(self):
        errs = classify.map_rule_errors([self.rec(BRACE_TARGETS[1], "bundle-ctl", ["crate_bundle"])], self.named)
        self.assertEqual(len(errs), 1)
        self.assertIn("want r6", errs[0])

    def test_singleton_keep_for_a_named_target_is_an_error(self):
        # singleton_bundle_group is decided after rule 2, so it cannot stand for a named target.
        errs = classify.map_rule_errors(
            [self.rec(PAREN_TARGETS[0], "keep", ["singleton_bundle_group:bundle-api-b"])], self.named)
        self.assertEqual(len(errs), 1)

    def test_r6_and_rule_one_keep_pass(self):
        recs = [self.rec(BRACE_TARGETS[0], "r6", ["architecture_map"]),
                self.rec(PAREN_TARGETS[1], "keep", ["process_global:current_exe"],
                         process_global={"current_exe": ["x.rs:1"]}),
                self.rec(UNNAMED, "bundle-api-b", ["task_api_other"])]
        self.assertEqual(classify.map_rule_errors(recs, self.named), [])

    def test_keep_without_a_rule_one_reason_is_an_error(self):
        errs = classify.map_rule_errors([self.rec(PAREN_TARGETS[2], "keep", ["named_suite"])], self.named)
        self.assertEqual(len(errs), 1)
        self.assertIn("want r6", errs[0])


class Collector(unittest.TestCase):
    def test_paths_come_from_the_map_checker(self):
        cam = classify.archmap_checker()
        self.assertIs(sys.modules["check_architecture_map"], cam)
        self.assertEqual(os.path.realpath(cam.__file__),
                         os.path.realpath(os.path.join(classify.REPO_ROOT, classify.ARCHMAP_CHECKER)))
        # Same set as the checker collects for the row (classify.py does not expand braces itself).
        self.assertEqual({p for p, lines in classify.archmap_paths(SYNTHETIC_MAP).items() if 3 in lines},
                         cam.collect_backtick_paths(BRACE_ROW.rstrip("\n")))

    def test_rows_outside_a_table_are_not_collected(self):
        # check-architecture-map.py checks table rows only; prose mentions do not pin a file.
        self.assertEqual(classify.archmap_paths("see `crates/celerisctl/tests/{pipe,cos_ops}.rs`\n"), {})


if __name__ == "__main__":
    unittest.main(verbosity=2)
