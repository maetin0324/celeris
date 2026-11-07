"""repo health の profile の [review] 節（ADR 2026-11-07-health-review-integration D2.1・D2.2）。

検査器の profile.validate が [review] の下位項目を検査すること、Celeris profile の値が実在の path を指すこと、
default.toml の空の [review] が extends の celeris の値を変えないことを確かめる。
"""
from __future__ import annotations

import os
import re
import subprocess
import sys
import tomllib
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TOOL_SRC = ROOT / "scripts/dev/repo-health"
sys.path.insert(0, str(TOOL_SRC))

from health import profile  # noqa: E402
from health.snapshot import UsageError  # noqa: E402

PROFILES = TOOL_SRC / "profiles"
REQUIRED_CLASSES = {"credential", "sandbox", "store", "events", "lease", "budget", "selfdeploy"}


def _load(name: str) -> dict:
    with open(PROFILES / f"{name}.toml", "rb") as fh:
        return tomllib.load(fh)


def _base(review) -> dict:
    return {"version": "1", "review": review}


class HealthReviewProfileTests(unittest.TestCase):
    def assert_invalid(self, review, needle: str) -> None:
        with self.assertRaises(UsageError) as cm:
            profile.validate(_base(review), "t.toml")
        self.assertIn("profile_invalid", str(cm.exception))
        self.assertIn(needle, str(cm.exception))

    def test_health_review_valid_section_passes(self):
        profile.validate(_base({
            "owner_map": "docs/map.md",
            "high_risk": [{"class": "store", "paths": ["^src/store/"]}],
            "invariants": {"events-append-only": "RULES.md"},
        }), "t.toml")
        profile.validate(_base({}), "t.toml")

    def test_health_review_unknown_subkey_is_rejected(self):
        self.assert_invalid({"owner_map": "m.md", "risk_levels": []}, "未知の項目")

    def test_health_review_owner_map_must_be_string(self):
        self.assert_invalid({"owner_map": ["m.md"]}, "owner_map")
        self.assert_invalid({"owner_map": " "}, "owner_map")

    def test_health_review_high_risk_shape_is_checked(self):
        self.assert_invalid({"high_risk": {"class": "x", "paths": ["a"]}}, "high_risk")
        self.assert_invalid({"high_risk": [{"class": "x", "paths": ["a"], "level": "high"}]}, "class・paths だけ")
        self.assert_invalid({"high_risk": [{"paths": ["a"]}]}, "class")
        self.assert_invalid({"high_risk": [{"class": "x", "paths": []}]}, "paths")
        self.assert_invalid({"high_risk": [{"class": "x", "paths": ["("]}]}, "正規表現が不正")
        self.assert_invalid({"high_risk": [{"class": "x", "paths": ["a"]}, {"class": "x", "paths": ["b"]}]}, "重複")

    def test_health_review_invariants_shape_is_checked(self):
        self.assert_invalid({"invariants": ["CLAUDE.md"]}, "invariants")
        self.assert_invalid({"invariants": {"x": 1}}, "invariants")
        self.assert_invalid({"invariants": {"x": ""}}, "invariants")

    def test_health_review_section_must_be_table(self):
        self.assert_invalid("docs/map.md", "review は表")

    def test_health_review_celeris_profile_points_at_existing_paths(self):
        rv = _load("celeris")["review"]
        self.assertTrue((ROOT / rv["owner_map"]).is_file())
        classes = {c["class"] for c in rv["high_risk"]}
        self.assertLessEqual(REQUIRED_CLASSES, classes)
        files = subprocess.run(["git", "-C", str(ROOT), "ls-files"], capture_output=True, text=True,
                               check=True).stdout.split()
        for c in rv["high_risk"]:
            for pat in c["paths"]:
                rx = re.compile(pat)
                self.assertTrue(any(rx.search(f) for f in files), f"{c['class']}: {pat} が追跡 file に当たらない")
        for name, src in rv["invariants"].items():
            self.assertTrue(os.path.isfile(ROOT / src), f"invariants.{name}: {src} が無い")
        for name in ("task-state-in-sqlite", "events-append-only", "no-llm-in-dispatcher"):
            self.assertIn(name, rv["invariants"])

    def test_health_review_default_extends_celeris_without_copying_values(self):
        default, celeris = _load("default"), _load("celeris")
        profile.validate(default, "default.toml")
        profile.validate(celeris, "celeris.toml")
        self.assertEqual(default.get("extends"), "celeris")
        self.assertEqual(default["review"], {})
        merged = profile._merge(celeris, default)
        self.assertEqual(merged["review"], celeris["review"])


if __name__ == "__main__":
    unittest.main()
