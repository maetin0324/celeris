"""Celeris profile の境界の class と class ごとの不変条件（ADR 2026-11-17-boundary-change-risk-review D2・D6）。

守る不変条件:
- 監査で class に合わなかった境界の file（task-api/knowledge.rs・cos/operations.rs・dispatcher/worker_finish.rs・
  task-worker/browser_policy.rs）と ADR D2 の表の file が、base 側の `--print-review-policy` の出力で期待の class に合う。
- web/・docs/ の追跡 file はどの class にも合わない（普通の UI・文書の変更を設計 review に回さない）。
- high_risk の項目の invariants の名前は、extends を解決した後の [review.invariants] の key に限る。未知の名前・
  形の違う値は profile の読み込みエラー（exit 2・profile_invalid）で、黙って捨てない。
- invariants を書かない class の出力は今までと同じ形（`class`・`paths` だけ）。

既存の試験で検出できない理由: test_repo_health_review_policy.py は class が 7 つ以上あることと、各 path の式が追跡 file に
当たることしか見ない。どの file がどの class に合うか（監査の抜け）、普通の path が合わないこと、invariants の名前の
参照は見ない。共有の fixture（repo_health_fixtures.py）は使わず、一時 git repo を作る。
"""
from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TOOL_SRC = ROOT / "scripts/dev/repo-health"
PROFILE_DIR = "scripts/dev/repo-health/profiles"
sys.path.insert(0, str(TOOL_SRC))

import validate_report  # noqa: E402
from health import profile  # noqa: E402
from health.snapshot import UsageError  # noqa: E402

POLICY_SCHEMA = TOOL_SRC / "schema" / "review-policy-v1.schema.json"

# ADR D2 の表: 境界の file → 合うべき class（監査の抜けの 4 file を含む）。
BOUNDARY_FILES = {
    "crates/task-api/src/knowledge.rs": {"transaction"},
    "crates/task-api/src/cos/operations.rs": {"authz"},
    "crates/task-dispatch/src/dispatcher/worker_finish.rs": {"worker-finish"},
    "crates/task-worker/src/browser_policy.rs": {"browser"},
    "crates/task-api/src/principal.rs": {"authz"},
    "crates/task-api/src/middleware.rs": {"authz"},
    "crates/task-api/src/handlers/authority.rs": {"authz"},
    "crates/task-core/src/authority.rs": {"authz"},
    "crates/task-ops/src/lifecycle.rs": {"transaction"},
    "crates/task-core/src/transition.rs": {"event-apply"},
    "crates/task-ops/src/replay.rs": {"event-apply"},
    "crates/task-worker/src/browser_credential.rs": {"browser"},
    "crates/task-worker/src/browser_cdp_credential_ipc.rs": {"browser"},
    "crates/task-worker/src/browser_launcher_run.rs": {"browser"},
    "crates/task-api/src/browser_credentials.rs": {"browser"},
    "crates/task-core/src/design_review.rs": {"review-gate"},
    "crates/task-dispatch/src/health_review/design.rs": {"review-gate"},
    "crates/task-dispatch/src/health_review/design_gate.rs": {"review-gate"},
    "crates/task-dispatch/src/health_review/design_verdict.rs": {"review-gate"},
    "crates/task-dispatch/src/health_review/policy.rs": {"review-gate"},
    "crates/task-dispatch/src/health_review/policy_review.rs": {"review-gate"},
    "crates/task-dispatch/src/health_review/gate.rs": {"review-gate"},
    "crates/celeris/src/instance.rs": {"lease", "selfdeploy"},
}

# ADR D2 の class ごとの invariants（既存の 7 class にも付ける）。
CLASS_INVARIANTS = {
    "credential": ["credential-stays-in-broker"],
    "sandbox": ["worker-db-read-only", "worker-no-subagents"],
    "store": ["task-state-in-sqlite", "no-data-loss"],
    "events": ["events-append-only"],
    "lease": ["lease-single-holder"],
    "budget": ["budget-unknown-cost-stops"],
    "selfdeploy": ["unpromoted-release-no-takeover"],
    "authz": ["api-principal-from-token", "cos-audit-same-tx"],
    "transaction": ["task-state-in-sqlite", "lifecycle-human-epoch-same-tx"],
    "event-apply": ["events-append-only", "task-state-in-sqlite"],
    "browser": ["credential-stays-in-broker"],
    "worker-finish": ["completion-not-self-reported", "lease-single-holder", "budget-unknown-cost-stops"],
    "review-gate": ["completion-not-self-reported", "policy-change-needs-human", "no-llm-in-dispatcher"],
}

# 境界の近くにあるが class に合わない普通の file（同じ crate の UI 向け handler・画面・文書）。
PLAIN_FILES = [
    "web/api/client.ts",
    "docs/SPEC.md",
    "docs/architecture-map.md",
    "crates/task-api/src/handlers/tasks.rs",
    "crates/task-dispatch/src/dispatcher/work_units.rs",
]


def git(cwd: Path, *args: str) -> str:
    env = {**os.environ, "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@example.invalid",
           "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@example.invalid",
           "GIT_AUTHOR_DATE": "2026-01-01T00:00:00Z", "GIT_COMMITTER_DATE": "2026-01-01T00:00:00Z"}
    env.pop("CELERIS_WU_BASE", None)
    return subprocess.run(["git", "-C", str(cwd), *args], check=True, capture_output=True, text=True,
                          env=env).stdout.strip()


def classes_of(policy: dict, path: str) -> set[str]:
    return {c["class"] for c in policy["high_risk"] if any(re.search(p, path) for p in c["paths"])}


class _TempRepo(unittest.TestCase):
    """検査器の写しと profile を commit した一時 git repo で `--print-review-policy` を走らせる。"""

    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp(prefix="rh-boundary-", dir=os.environ.get("TMPDIR")))
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.tool = self.tmp / "tool"
        shutil.copytree(TOOL_SRC, self.tool, ignore=shutil.ignore_patterns("__pycache__"))
        for f in (self.tool / "profiles").iterdir():
            f.unlink()
        self.repo = self.tmp / "repo"
        self.repo.mkdir()
        git(self.repo, "init", "-q", "-b", "main")
        self.write("README.md", "# r\n")

    def write(self, rel: str, text: str) -> None:
        path = self.repo / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")

    def commit(self, msg: str) -> str:
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-q", "-m", msg)
        return git(self.repo, "rev-parse", "HEAD")

    def cli(self, base: str) -> subprocess.CompletedProcess:
        env = {k: v for k, v in os.environ.items() if k != "CELERIS_WU_BASE"}
        return subprocess.run([sys.executable, "-I", str(self.tool / "repo_health.py"), "--print-review-policy",
                               "--repo", str(self.repo), "--base", base], capture_output=True, text=True, env=env)

    def policy(self, base: str) -> dict:
        proc = self.cli(base)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        data = json.loads(proc.stdout)
        with open(POLICY_SCHEMA, encoding="utf-8") as fh:
            schema = json.load(fh)
        out = validate_report.Violations("policy")
        validate_report.check_schema(schema, schema, data, "", out)
        self.assertEqual(out.items, [])
        return data

    def assert_load_error(self, base: str, *needles: str) -> None:
        proc = self.cli(base)
        self.assertEqual(proc.returncode, 2, proc.stdout + proc.stderr)
        self.assertEqual(proc.stdout, "")
        self.assertIn("profile_invalid", proc.stderr)
        for needle in needles:
            self.assertIn(needle, proc.stderr)


class BoundaryClassesOnRealProfileTests(_TempRepo):
    """repo の default.toml・celeris.toml をそのまま一時 repo に commit し、base 側の出力で class を当てる。"""

    def setUp(self) -> None:
        super().setUp()
        for name in ("default.toml", "celeris.toml"):
            self.write(f"{PROFILE_DIR}/{name}", (ROOT / PROFILE_DIR / name).read_text(encoding="utf-8"))
        self.got = self.policy(self.commit("real profiles"))

    def test_boundary_audit_gaps_and_adr_files_match_their_class(self):
        for path, want in BOUNDARY_FILES.items():
            self.assertTrue((ROOT / path).is_file(), f"{path} が repo に無い")
            self.assertEqual(classes_of(self.got, path), want, path)

    def test_boundary_plain_paths_match_no_class(self):
        tracked = subprocess.run(["git", "-C", str(ROOT), "ls-files", "--", "web", "docs"], capture_output=True,
                                 text=True, check=True).stdout.split()
        self.assertGreater(len(tracked), 100)
        for f in PLAIN_FILES:
            self.assertTrue((ROOT / f).is_file(), f"{f} が repo に無い")
        hits = {f: classes_of(self.got, f) for f in tracked + PLAIN_FILES}
        self.assertEqual({f: c for f, c in hits.items() if c}, {})

    def test_boundary_class_invariants_name_known_entries(self):
        got = {c["class"]: c.get("invariants") for c in self.got["high_risk"]}
        self.assertEqual(got, CLASS_INVARIANTS)
        for names in got.values():
            for name in names:
                self.assertIn(name, self.got["invariants"])
                self.assertTrue((ROOT / self.got["invariants"][name]).is_file(), name)


PARENT = """version = "1"
[review]
owner_map = "OWNERS.md"
[[review.high_risk]]
class = "store"
paths = ["^src/store/"]
invariants = ["state-in-db"]
[[review.high_risk]]
class = "ui"
paths = ["^web/"]
[review.invariants]
state-in-db = "RULES.md"
events-append-only = "RULES.md"
"""


class BoundaryInvariantNamesTests(_TempRepo):
    """invariants の名前の参照と形の検査（extends を解決した後の値で見る）。"""

    def test_boundary_unknown_invariant_name_is_load_error(self):
        self.write(f"{PROFILE_DIR}/default.toml", PARENT.replace('["state-in-db"]', '["state-in-db", "typo"]'))
        self.assert_load_error(self.commit("unknown name"), "typo", "store")

    def test_boundary_invariant_names_resolve_after_extends(self):
        # 子の high_risk は親の [review.invariants] の名前を使える
        self.write(f"{PROFILE_DIR}/team.toml", PARENT)
        self.write(f"{PROFILE_DIR}/default.toml", textwrap.dedent("""\
            version = "1"
            extends = "team"
            [[review.high_risk]]
            class = "events"
            paths = ['^src/events\\.rs$']
            invariants = ["events-append-only"]
            """))
        got = self.policy(self.commit("child uses parent names"))
        self.assertEqual(got["high_risk"], [{"class": "events", "paths": ["^src/events\\.rs$"],
                                             "invariants": ["events-append-only"]}])
        # 子が [review.invariants] を差し替えて親の class が使う名前を消すと読み込みエラー
        self.write(f"{PROFILE_DIR}/default.toml", 'version = "1"\nextends = "team"\n'
                                                  '[review.invariants]\nother = "RULES.md"\n')
        self.assert_load_error(self.commit("child drops a name"), "state-in-db")

    def test_boundary_invariants_omitted_keeps_old_shape(self):
        self.write(f"{PROFILE_DIR}/default.toml", PARENT)
        got = self.policy(self.commit("mixed"))
        self.assertEqual(got["high_risk"], [{"class": "store", "paths": ["^src/store/"], "invariants": ["state-in-db"]},
                                            {"class": "ui", "paths": ["^web/"]}])

    def test_boundary_invariants_shape_is_checked(self):
        for bad in ("state-in-db", [], [""], ["state-in-db", "state-in-db"], [1]):
            with self.subTest(bad=bad), self.assertRaises(UsageError) as cm:
                profile.validate({"version": "1", "review": {
                    "high_risk": [{"class": "store", "paths": ["^src/"], "invariants": bad}],
                    "invariants": {"state-in-db": "RULES.md"}}}, "t.toml")
            self.assertIn("review.high_risk[0].invariants", str(cm.exception))


if __name__ == "__main__":
    unittest.main()
