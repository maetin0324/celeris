"""build-k4-public-archive.py の unit test（標準ライブラリのみ・合成 tgz）。
実行: python3 -m unittest crates/campaign/scripts/test_build_k4_public_archive.py
"""
import hashlib
import io
import json
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "build-k4-public-archive.py"

FILES = {
    "review28/qstat-43706.txt": b"Job Id: 43706\n",
    "review28/cells-43706.csv": b"fs,valid\n",
    "review28/__pycache__/x.cpython-39.pyc": b"\x00pyc",
    "runs/43706-x/planned-cells.json": b"[]\n",
    "runs/43706-x/cells/benchfs/rep1/w1a/cell.json": b"{}\n",
    "runs/43706-x/cells/benchfs/rep1/w1a/nic-audit.json": b"{\"pass\":true}\n",
    "runs/43706-x/fs/benchfs/rep1/w1a/logs-audit.json": b"{\"pass\":true}\n",
}
DIRS = ["review28", "review28/__pycache__"]
EXCLUDE = [
    "review28/__pycache__",
    "review28/__pycache__/x.cpython-39.pyc",
    "review28/cells-43706.csv",
    "runs/43706-x/cells/benchfs/rep1/w1a/nic-audit.json",
    "runs/43706-x/fs/benchfs/rep1/w1a/logs-audit.json",
]


def sha(b):
    return hashlib.sha256(b).hexdigest()


class Build(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = Path(self.tmp.name)
        self.src = self.dir / "src.tgz"
        with tarfile.open(self.src, "w:gz") as t:
            for i, d in enumerate(DIRS):
                ti = tarfile.TarInfo(d)
                ti.type, ti.mode, ti.mtime, ti.uid = tarfile.DIRTYPE, 0o755, 1000 + i, 31305
                t.addfile(ti)
            # 名前順でない順に入れ、mtime・uid も entry ごとに変える。
            for i, (name, data) in enumerate(reversed(list(FILES.items()))):
                ti = tarfile.TarInfo(name)
                ti.size, ti.mode, ti.mtime, ti.uid, ti.uname = len(data), 0o644, 2000 + i, 31305, "rmaeda"
                t.addfile(ti, io.BytesIO(data))
        self.excl = self.dir / "exclude.txt"
        self.excl.write_text("".join(p + "\n" for p in EXCLUDE))

    def run_build(self, out, manifest, excl=None):
        return subprocess.run([sys.executable, "-I", str(SCRIPT), "--src", str(self.src),
                               "--exclude", str(excl or self.excl), "--out", str(out),
                               "--manifest", str(manifest)], capture_output=True, text=True)

    def test_deterministic_and_keeps_raw_bytes(self):
        src_sha = sha(self.src.read_bytes())
        r1 = self.run_build(self.dir / "a.tgz", self.dir / "a.json")
        r2 = self.run_build(self.dir / "b.tgz", self.dir / "b.json")
        self.assertEqual(r1.returncode, 0, r1.stderr)
        self.assertEqual(r2.returncode, 0, r2.stderr)
        self.assertEqual((self.dir / "a.tgz").read_bytes(), (self.dir / "b.tgz").read_bytes())
        self.assertEqual(src_sha, sha(self.src.read_bytes()))
        with tarfile.open(self.dir / "a.tgz") as t:
            ms = t.getmembers()
            names = [m.name for m in ms]
            self.assertEqual(names, sorted(names))
            self.assertTrue(all(m.mtime == 0 and m.uid == 0 and m.gid == 0 and m.uname == "" for m in ms))
            for m in ms:
                if m.isfile():
                    self.assertEqual(t.extractfile(m).read(), FILES[m.name])
        self.assertEqual(set(names), (set(FILES) | set(DIRS)) - set(EXCLUDE))
        self.assertEqual((self.dir / "a.tgz").read_bytes()[4:8], b"\x00\x00\x00\x00")  # gzip mtime
        m = json.loads((self.dir / "a.json").read_text())
        self.assertTrue(m["changed"])
        self.assertEqual(m["source"]["sha256"], src_sha)
        self.assertEqual(m["output"]["sha256"], sha((self.dir / "a.tgz").read_bytes()))
        self.assertEqual(sorted(e["path"] for e in m["excluded"]), sorted(EXCLUDE))
        self.assertTrue(all(e["reason"] for e in m["excluded"]))
        kept = {e["path"]: e for e in m["kept"]}
        self.assertEqual(kept["review28/qstat-43706.txt"]["sha256"], sha(FILES["review28/qstat-43706.txt"]))
        # out の path 以外は同じ内容（時刻を書かない）。
        m2 = json.loads((self.dir / "b.json").read_text())
        m["output"].pop("path")
        m2["output"].pop("path")
        self.assertEqual(m, m2)

    def expect_fail(self, lines, needle):
        excl = self.dir / "bad.txt"
        excl.write_text("".join(p + "\n" for p in lines))
        r = self.run_build(self.dir / "o.tgz", self.dir / "o.json", excl)
        self.assertEqual(r.returncode, 1, r.stdout)
        self.assertIn(needle, r.stderr)
        self.assertFalse((self.dir / "o.tgz").exists())

    def test_rejects_path_missing_from_source(self):
        self.expect_fail(EXCLUDE + ["review28/nope.txt"], "not in the source archive")

    def test_rejects_raw_file_without_reason_rule(self):
        self.expect_fail(["review28/qstat-43706.txt"], "no exclusion reason rule")

    def test_rejects_dir_with_unlisted_children(self):
        self.expect_fail(["review28/__pycache__"], "has entries not in the exclude list")

    def test_rejects_writing_over_source(self):
        r = self.run_build(self.src, self.dir / "o.json")
        self.assertEqual(r.returncode, 1)
        self.assertIn("must not be the source", r.stderr)


if __name__ == "__main__":
    unittest.main()
