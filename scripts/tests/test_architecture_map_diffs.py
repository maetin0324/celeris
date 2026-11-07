"""check-architecture-map.py の find_diffs: 6 種類の食い違いの正例（出る）と負例（出ない）。"""
from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
_spec = importlib.util.spec_from_file_location("check_architecture_map", ROOT / "scripts/dev/check-architecture-map.py")
cam = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = cam
_spec.loader.exec_module(cam)

DOC = "docs/architecture-map.md"
HEADER = ["| subsystem | owner crate/module | entry point | ADR / 設計 | 関連試験 | 不変条件 |", "|---|---|---|---|---|---|"]
FILES = {
    "src/store.rs", "src/store/tests.rs", "src/api.rs", "tests/api.rs", "tests/extra.rs",
    "adr/0001-ok.md", "adr/0002-old.md", "src/gone_but_listed.rs",
}
TEXTS = {"adr/0001-ok.md": "# ok\n- 状態: 採用\n", "adr/0002-old.md": "# old\n- 状態: 廃止（0003 に置き換え）\n"}


def row(name="store", entry="`src/store.rs`", adr="[ADR-0001](../adr/0001-ok.md)", tests="`src/store/tests.rs`",
        inv="書き込みは transaction で行う"):
    return f"| {name} | core | {entry} | {adr} | {tests} | {inv} |"


def exists(rel: str) -> bool:
    rel = rel.rstrip("/")
    return rel in FILES or any(f.startswith(rel + "/") for f in FILES)


def diffs(rows: list[str], reference: dict | None = None, kind: str | None = None):
    out = cam.find_diffs(HEADER + rows, DOC, exists, TEXTS.get, reference)
    return [d for d in out if kind is None or d.kind == kind]


def ref(**cap) -> dict:
    base = {"id": "store", "owners": [{"path": "src/store.rs"}], "tests": ["src/store/tests.rs"],
            "invariants": [{"evidence_path": "adr/0001-ok.md"}]}
    return {"capabilities": [{**base, **cap}]}


class FindDiffsTests(unittest.TestCase):
    def test_clean_map_and_reference_have_no_diffs(self):
        self.assertEqual(diffs([row()], ref()), [])

    def test_wrong_path(self):
        got = diffs([row(entry="`src/missing.rs`")], kind="wrong_path")
        self.assertEqual([(d.line, d.key) for d in got], [(3, "src/missing.rs")])
        got = diffs([row(adr="[x](../adr/9999-none.md)")], kind="wrong_path")
        self.assertEqual([d.key for d in got], ["adr/9999-none.md"], "相対リンクは doc の dir から解く")
        self.assertEqual(diffs([row()], kind="wrong_path"), [])

    def test_missing_test(self):
        got = diffs([row(tests="なし")], kind="missing_test")
        self.assertEqual([d.key for d in got], ["row:store"])
        got = diffs([row()], ref(tests=["src/store/tests.rs", "tests/extra.rs"]), kind="missing_test")
        self.assertEqual([d.key for d in got], ["store|tests/extra.rs"])
        # 地図が dir を挙げれば dir の下の試験は載っている
        self.assertEqual(diffs([row(tests="`src/store/`")], ref(), kind="missing_test"), [])

    def test_missing_invariant(self):
        got = diffs([row(inv="")], kind="missing_invariant")
        self.assertEqual([d.key for d in got], ["row:store"])
        got = diffs([row(adr="-")], ref(), kind="missing_invariant")
        self.assertEqual([d.key for d in got], ["store|adr/0001-ok.md"])
        self.assertEqual(diffs([row()], ref(), kind="missing_invariant"), [])

    def test_missing_row(self):
        got = diffs([row()], {"capabilities": [{"id": "api", "owners": [{"path": "src/api.rs"}]}]}, kind="missing_row")
        self.assertEqual([(d.line, d.key, d.paths) for d in got], [(0, "api", ("src/api.rs",))])
        self.assertEqual(diffs([row()], ref(), kind="missing_row"), [])
        # 子 module（store/tests.rs）を挙げた行は親 module store.rs の行でもある
        self.assertEqual(diffs([row(entry="`src/store/tests.rs`")], ref(), kind="missing_row"), [])

    def test_wrong_owner(self):
        two = ref(owners=[{"path": "src/store.rs"}, {"path": "src/api.rs"}])
        got = diffs([row()], two, kind="wrong_owner")
        self.assertEqual([(d.line, d.key, d.paths) for d in got], [(3, "store", ("src/api.rs",))])
        # 別の行が owner を挙げていれば食い違いではない
        self.assertEqual(diffs([row(), row(name="api", entry="`src/api.rs`")], two, kind="wrong_owner"), [])

    def test_stale(self):
        got = diffs([row(adr="[old](../adr/0002-old.md)")], kind="stale")
        self.assertEqual([d.key for d in got], ["store|adr/0002-old.md"])
        self.assertEqual(diffs([row()], kind="stale"), [])
        # 状態の語は呼び手が替えられる
        got = cam.find_diffs(HEADER + [row()], DOC, exists, TEXTS.get, None, stale_status="採用")
        self.assertEqual([d.kind for d in got], ["stale"])

    def test_keys_do_not_depend_on_line_numbers(self):
        a = diffs([row(entry="`src/missing.rs`")])
        b = diffs([row(name="other", entry="`src/api.rs`", tests="`tests/api.rs`"), row(entry="`src/missing.rs`")])
        self.assertEqual({(d.kind, d.key) for d in a}, {(d.kind, d.key) for d in b})
        self.assertNotEqual([d.line for d in a], [d.line for d in b])

    def test_repo_map_has_no_shape_or_path_diffs(self):
        """今の地図では既定の検査（実在・列の形）と同じ判定の種類は 0 件。"""
        lines = (ROOT / DOC).read_text(encoding="utf-8").splitlines()
        got = cam.find_diffs(lines, DOC, lambda rel: cam.path_exists(rel, ROOT), lambda _p: None)
        self.assertEqual([d for d in got if d.kind in ("wrong_path", "missing_test", "missing_invariant")], [])


if __name__ == "__main__":
    unittest.main()
