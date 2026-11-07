#!/usr/bin/env python3
"""Count Cargo targets per crate and separate test targets from tests/ support files.

`cargo metadata --no-deps --format-version 1` lists every target cargo builds
(lib / bin / test / bench / example / proc-macro / custom-build). A `.rs` file
under `<crate>/tests/` is a test target only when its path is the `src_path`
of a `test` target; everything else there (`tests/common/mod.rs`,
`tests/support/*.rs`, `tests/<dir>/*.rs` pulled in with `mod`) is a support
module that cargo never builds on its own. Counting "all `.rs` under tests/"
mixes the two; this report keeps them apart and shows the difference.

Usage:
  python3 scripts/dev/test-target-inventory.py --json
      [--metadata FILE]   # use a saved metadata JSON instead of running cargo
      [--root DIR]        # workspace root for relative paths (default: metadata's)
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from collections import Counter
from pathlib import Path

KINDS = ("lib", "bin", "test", "bench", "example", "proc-macro", "custom-build")
# Kinds nextest builds a test binary for when the target's `test` flag is true
# (`cargo nextest run --no-run` without --benches/--examples).
TEST_BINARY_KINDS = {"lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro", "bin", "test"}
SKIP_DIRS = {"target", "node_modules", ".git"}


def load_metadata(path: str | None) -> dict:
    if path:
        with open(path, encoding="utf-8") as f:
            return json.load(f)
    out = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(out.stdout)


def target_kind(target: dict) -> str:
    kinds = target.get("kind", [])
    for k in KINDS:
        if k in kinds:
            return k
    # lib crate-types such as rlib/cdylib appear as kind names too.
    if any(k in TEST_BINARY_KINDS for k in kinds):
        return "lib"
    return kinds[0] if kinds else "unknown"


def walk_tests_dir(tests_dir: Path) -> tuple[list[Path], int]:
    """Return (.rs files under tests_dir, count of non-.rs files)."""
    rs: list[Path] = []
    other = 0
    if not tests_dir.is_dir():
        return rs, other
    for dirpath, dirnames, filenames in os.walk(tests_dir):
        dirnames[:] = sorted(d for d in dirnames if d not in SKIP_DIRS)
        for name in sorted(filenames):
            p = Path(dirpath) / name
            if name.endswith(".rs"):
                rs.append(p)
            else:
                other += 1
    return rs, other


def rel(path: Path, root: Path) -> str:
    try:
        return path.resolve().relative_to(root.resolve()).as_posix()
    except ValueError:
        return path.as_posix()


def inventory_package(pkg: dict, root: Path) -> dict:
    manifest_dir = Path(pkg["manifest_path"]).parent
    tests_dir = manifest_dir / "tests"
    kinds: Counter[str] = Counter()
    test_binaries = 0
    test_targets: list[dict] = []
    target_srcs: set[str] = set()
    for t in pkg.get("targets", []):
        k = target_kind(t)
        kinds[k] += 1
        src = Path(t["src_path"])
        target_srcs.add(src.resolve().as_posix())
        if t.get("test", False) and any(x in TEST_BINARY_KINDS for x in t.get("kind", [])):
            test_binaries += 1
        if k == "test":
            try:
                inner = src.resolve().relative_to(tests_dir.resolve())
                where = "tests_top" if len(inner.parts) == 1 else "tests_nested"
            except ValueError:
                where = "outside_tests"
            test_targets.append({"name": t["name"], "src_path": rel(src, root), "location": where})

    rs_files, non_rs = walk_tests_dir(tests_dir)
    support: list[str] = []
    top_level_rs = 0
    for p in rs_files:
        if p.parent.resolve() == tests_dir.resolve():
            top_level_rs += 1
        if p.resolve().as_posix() not in target_srcs:
            support.append(rel(p, tests_dir))
    in_tests = sum(1 for t in test_targets if t["location"] != "outside_tests")
    return {
        "name": pkg["name"],
        "manifest_dir": rel(manifest_dir, root),
        "targets": {k: kinds.get(k, 0) for k in KINDS},
        "test_targets": len(test_targets),
        "test_targets_by_location": dict(Counter(t["location"] for t in test_targets)),
        "expected_test_binaries": test_binaries,
        "tests_dir": {
            "rs_files": len(rs_files),
            "top_level_rs_files": top_level_rs,
            "target_files": in_tests,
            "support_files": len(support),
            "support_file_paths": support,
            "non_rs_files": non_rs,
        },
        # What a "count every .rs under tests/" tally reports minus the real targets.
        "naive_minus_targets": len(rs_files) - in_tests,
        "test_target_names": sorted(t["name"] for t in test_targets),
    }


def build_report(meta: dict, root: Path) -> dict:
    members = set(meta.get("workspace_members", []))
    pkgs = [p for p in meta.get("packages", []) if not members or p["id"] in members]
    crates = sorted((inventory_package(p, root) for p in pkgs), key=lambda c: c["name"])
    totals: Counter[str] = Counter()
    for c in crates:
        for k, v in c["targets"].items():
            totals[k] += v
        totals["test_targets"] += c["test_targets"]
        totals["expected_test_binaries"] += c["expected_test_binaries"]
        totals["tests_dir_rs_files"] += c["tests_dir"]["rs_files"]
        totals["tests_dir_support_files"] += c["tests_dir"]["support_files"]
        totals["naive_minus_targets"] += c["naive_minus_targets"]
    return {
        "schema": "celeris.test-target-inventory/1",
        "source": "cargo metadata --no-deps --format-version 1",
        "crates": crates,
        "totals": dict(totals),
    }


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--json", action="store_true", help="print the report as JSON (the only format)")
    ap.add_argument("--metadata", help="read cargo metadata JSON from this file")
    ap.add_argument("--root", help="workspace root for relative paths")
    args = ap.parse_args(argv)
    if not args.json:
        ap.error("--json is required")
    meta = load_metadata(args.metadata)
    root = Path(args.root or meta.get("workspace_root") or ".")
    json.dump(build_report(meta, root), sys.stdout, ensure_ascii=False, indent=2)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
