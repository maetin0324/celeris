#!/usr/bin/env python3
"""ops-r2-seed.sh make の試験（ADR 2026-10-31-ops-r2-preflight-fixes 付記 D-seed2）。合成の公開 tar・case.json・asset store で、
submodule 込みの種・照合・拒否と、漏洩検査が種の submodule の中身を読むことを確かめる。network には出ない。

    cd scripts/campaign && python3 -m unittest test_ops_r2_seed
"""
import hashlib
import io
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SEED = os.path.join(HERE, "ops-r2-seed.sh")
LEAK = os.path.join(HERE, "check-ops-r1-leak.py")

SUB_SECRET = "SUBMODULE-HIDDEN-NEEDLE-5c1e"
C_OUTER = "a" * 40
C_INNER = "b" * 40


def sha_file(p):
    with open(p, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()


def write_tar(path, files, dirs=()):
    """files: {member: bytes}、dirs: dir の member。git archive と同じく非圧縮。"""
    with tarfile.open(path, "w") as t:
        for d in dirs:
            info = tarfile.TarInfo(d)
            info.type = tarfile.DIRTYPE
            info.mode = 0o775
            t.addfile(info)
        for member, data in files.items():
            info = tarfile.TarInfo(member)
            info.size = len(data)
            info.mode = 0o755 if member.endswith(".sh") else 0o644
            info.uid = 1234
            t.addfile(info, io.BytesIO(data))
    return sha_file(path)


class SeedTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="seed-test-")
        self.assets = os.path.join(self.tmp, "assets")
        os.makedirs(os.path.join(self.assets, "submodules"))
        # 公開の初期 tar: submodule の path は空の dir（git archive の gitlink と同じ）
        self.archive = os.path.join(self.tmp, "k2-initial.tar")
        self.archive_sha = write_tar(self.archive, {"Cargo.toml": b"[workspace]\n", "build.sh": b"#!/bin/sh\n"},
                                     dirs=("lib/", "lib/outer/"))
        outer = os.path.join(self.assets, "submodules", f"outer-{C_OUTER[:8]}.tar")
        inner = os.path.join(self.assets, "submodules", f"inner-{C_INNER[:8]}.tar")
        sums = {
            f"submodules/outer-{C_OUTER[:8]}.tar":
                write_tar(outer, {"src/lib.rs": b"pub fn f() {}\n"}, dirs=("src/", "vendor/", "vendor/inner/")),
            f"submodules/inner-{C_INNER[:8]}.tar":
                write_tar(inner, {"inner.c": f"/* {SUB_SECRET} */\n".encode()}),
        }
        self.write_sums(sums)
        self.case = os.path.join(self.tmp, "case.json")
        self.write_case()

    def tearDown(self):
        shutil.rmtree(self.tmp)

    def write_sums(self, sums):
        with open(os.path.join(self.assets, "SHA256SUMS"), "w", encoding="utf-8") as f:
            for k, v in sums.items():
                f.write(f"{v}  {k}\n")

    def write_case(self, archive_sha=None, submodules=None):
        with open(self.case, "w", encoding="utf-8") as f:
            json.dump({"initial": {
                "archive_sha256": archive_sha or self.archive_sha,
                # 入れ子を先に書いても浅い path から入る
                "submodules": submodules or [f"lib/outer/vendor/inner@{C_INNER}", f"lib/outer@{C_OUTER}"],
            }}, f)

    def seed(self, *args):
        return subprocess.run(["sh", SEED, *args], capture_output=True, text=True)

    def make(self, out=None):
        out = out or os.path.join(self.tmp, "seed.tar")
        return self.seed("make", "--archive", self.archive, "--case", self.case, "--assets", self.assets,
                         "--out", out), out

    def test_seed_contains_submodules_and_is_deterministic(self):
        p, out = self.make()
        self.assertEqual(p.returncode, 0, p.stderr)
        sha = p.stdout.strip()
        self.assertEqual(sha, sha_file(out))
        with tarfile.open(out) as t:
            names = {m.name: m for m in t.getmembers()}
        self.assertIn("Cargo.toml", names)
        self.assertIn("lib/outer/src/lib.rs", names)
        self.assertIn("lib/outer/vendor/inner/inner.c", names)
        self.assertEqual(oct(names["build.sh"].mode), oct(0o755))
        self.assertEqual(names["lib/outer/src/lib.rs"].uid, 0)
        # 公開 tar は変えない
        self.assertEqual(sha_file(self.archive), self.archive_sha)
        p2, out2 = self.make(os.path.join(self.tmp, "seed2.tar"))
        self.assertEqual(p2.returncode, 0, p2.stderr)
        self.assertEqual(p2.stdout.strip(), sha, "same inputs give the same bytes")

    def test_a_and_b_get_same_bytes_and_unpack_builds_the_full_tree(self):
        p, out = self.make()
        sha = p.stdout.strip()
        placed = []
        for arm in ("a", "b"):
            state = os.path.join(self.tmp, arm)
            q = self.seed("place", "--seed", out, "--sha256", sha, "--state", state)
            self.assertEqual(q.returncode, 0, q.stderr)
            with open(os.path.join(state, "seed", "benchfs.tar"), "rb") as f:
                placed.append(f.read())
        self.assertEqual(placed[0], placed[1])
        into = os.path.join(self.tmp, "a", "work")
        q = self.seed("unpack", "--seed", os.path.join(self.tmp, "a", "seed", "benchfs.tar"), "--sha256", sha,
                      "--into", into)
        self.assertEqual(q.returncode, 0, q.stderr)
        self.assertTrue(os.path.isfile(os.path.join(into, "lib/outer/vendor/inner/inner.c")))
        self.assertFalse(os.path.exists(os.path.join(into, ".git")))

    def test_public_archive_sha_must_match_case_json(self):
        self.write_case(archive_sha="0" * 64)
        p, out = self.make()
        self.assertEqual(p.returncode, 1)
        self.assertIn("initial.archive_sha256", p.stderr)
        self.assertFalse(os.path.exists(out))
        self.assertFalse(os.path.exists(out + ".tmp"))

    def test_submodule_tar_must_match_sha256sums(self):
        with open(os.path.join(self.assets, "submodules", f"inner-{C_INNER[:8]}.tar"), "ab") as f:
            f.write(b"\0" * 512)
        p, _ = self.make()
        self.assertEqual(p.returncode, 1)
        self.assertIn("differs from SHA256SUMS", p.stderr)

    def test_unlisted_or_unpinned_submodule_is_refused(self):
        self.write_case(submodules=[f"lib/outer@{C_OUTER}", f"lib/other@{'c' * 40}"])
        p, _ = self.make()
        self.assertEqual(p.returncode, 1)
        self.assertIn("not listed in SHA256SUMS", p.stderr)
        self.write_case(submodules=["lib/outer@abc123"])
        p, _ = self.make()
        self.assertEqual(p.returncode, 1)
        self.assertIn("40-hex", p.stderr)

    def test_file_in_both_archive_and_submodule_is_refused(self):
        self.archive_sha = write_tar(self.archive, {"lib/outer/src/lib.rs": b"x\n"}, dirs=("lib/", "lib/outer/"))
        self.write_case()
        p, _ = self.make()
        self.assertEqual(p.returncode, 1)
        self.assertIn("appears twice", p.stderr)

    def test_old_repo_rev_arguments_are_usage_errors(self):
        p = self.seed("make", "--repo", self.tmp, "--rev", "HEAD", "--out", os.path.join(self.tmp, "x.tar"))
        self.assertEqual(p.returncode, 2)

    def test_leak_check_reads_submodule_content_in_the_placed_seed(self):
        p, out = self.make()
        state = os.path.join(self.tmp, "a")
        q = self.seed("place", "--seed", out, "--sha256", p.stdout.strip(), "--state", state)
        self.assertEqual(q.returncode, 0, q.stderr)
        private = os.path.join(self.tmp, "private.json")
        with open(private, "w", encoding="utf-8") as f:
            json.dump({"schema": "x", "hidden": SUB_SECRET}, f)
        res = os.path.join(self.tmp, "leak.json")
        r = subprocess.run([sys.executable, "-I", LEAK, "--private", private, "--canary", "CANARY-not-there-77",
                            "--scan", state, "--out", res], capture_output=True, text=True)
        self.assertEqual(r.returncode, 1, r.stderr)
        with open(res, encoding="utf-8") as f:
            doc = json.load(f)
        members = {h.get("member") for h in doc["hits"] if h.get("needle_kind") == "private_string"}
        self.assertIn("lib/outer/vendor/inner/inner.c", members)


if __name__ == "__main__":
    unittest.main()
