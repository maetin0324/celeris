#!/usr/bin/env python3
"""check-ops-r1-leak.py の試験。合成した private.json・scan 先・可読 path で、検出と見逃しの無さを確かめる。

    python3 -I scripts/campaign/test_check_ops_r1_leak.py
"""
import gzip
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
import zipfile

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "check-ops-r1-leak.py")

SECRET = "隠しの採点規則: 欠測セルを分母から落とすと不合格"
SECRET_ID = "k4-hidden-defect-zzz"
PRIVATE = {
    "schema": "celeris.campaign-case-private/1",
    "case_id": "k4-test-public-case",
    "split": "final",
    "hidden_command": SECRET,
    "known_defects": [{"id": SECRET_ID, "description": "short"}],
    "expected": {"input": "public-input-v2.tgz"},
}
CANARY = "CANARY-ops-r1-7f3a9c"
OLD_V1 = b"old v1 tgz bytes, not for trials\n"
OLD_V1_SHA = hashlib.sha256(OLD_V1).hexdigest()


class LeakCheckTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="leak-test-")
        self.priv_dir = os.path.join(self.tmp, "grader-private")
        os.mkdir(self.priv_dir)
        self.private = os.path.join(self.priv_dir, "private.json")
        with open(self.private, "w", encoding="utf-8") as f:
            json.dump(dict(PRIVATE, canary=CANARY), f, ensure_ascii=False, indent=2)
        with open(self.private, "rb") as f:
            self.private_sha = hashlib.sha256(f.read()).hexdigest()
        self.scan = os.path.join(self.tmp, "scan")
        os.mkdir(self.scan)
        self.write("public/case.json", json.dumps({"case_id": "k4-test-public-case", "split": "final"}))
        self.write("public/README.md", "公開の説明だけ\n")
        self.out = os.path.join(self.tmp, "out.json")

    def tearDown(self):
        for d, dirs, files in os.walk(self.tmp):
            for n in dirs + files:
                p = os.path.join(d, n)
                if not os.path.islink(p):
                    os.chmod(p, 0o700)
        shutil.rmtree(self.tmp)

    def write(self, rel, content, mode="w"):
        p = os.path.join(self.scan, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, mode, **({"encoding": "utf-8"} if mode == "w" else {})) as f:
            f.write(content)
        return p

    def check(self, *extra, scan=None, canary=True, out=None):
        args = [sys.executable, "-I", SCRIPT, "--private", self.private, "--forbidden-sha", OLD_V1_SHA,
                "--scan", scan or self.scan, "--out", out or self.out]
        if canary:
            args += ["--canary", CANARY]
        args += list(extra)
        p = subprocess.run(args, capture_output=True, text=True)
        with open(out or self.out, encoding="utf-8") as f:
            doc = json.load(f)
        return p.returncode, doc, p

    def kinds(self, doc):
        return {(h["kind"], h.get("needle_kind")) for h in doc["hits"]}

    # ---- 合格

    def test_clean_passes_and_reports_checked(self):
        rc, doc, p = self.check()
        self.assertEqual(rc, 0, p.stderr + json.dumps(doc, ensure_ascii=False))
        self.assertTrue(doc["pass"])
        self.assertEqual(doc["hits"], [])
        priv = doc["checked"]["private"][0]
        self.assertEqual(priv["sha256"], self.private_sha)
        # 12 字以上: hidden_command・known_defects[0].id・canary・expected.input。schema・case_id は公開の識別子で除く
        self.assertEqual(priv["strings_checked"], 4)
        reasons = {e["json_path"]: e["reason"] for e in priv["excluded"]}
        self.assertEqual(reasons, {".schema": "public_identifier", ".case_id": "public_identifier"})
        self.assertEqual(doc["checked"]["forbidden_sha256"], [OLD_V1_SHA])
        self.assertEqual(doc["checked"]["scan"][0]["files"], 2)

    def test_result_never_contains_plaintext_needles(self):
        self.write("leak.txt", SECRET + "\n" + CANARY)
        rc, doc, _ = self.check()
        self.assertEqual(rc, 1)
        with open(self.out, encoding="utf-8") as f:
            text = f.read()
        self.assertNotIn(SECRET, text)
        self.assertNotIn(CANARY, text)

    # ---- 内容の出現

    def test_private_string_detected(self):
        self.write("notes/memo.md", f"前置き {SECRET} 後ろ")
        rc, doc, _ = self.check()
        self.assertEqual(rc, 1)
        self.assertFalse(doc["pass"])
        self.assertIn(("content", "private_string"), self.kinds(doc))
        hit = [h for h in doc["hits"] if h["needle_kind"] == "private_string"][0]
        self.assertTrue(hit["path"].endswith("notes/memo.md"))
        self.assertEqual(hit["needle_sha256"], hashlib.sha256(SECRET.encode()).hexdigest())

    def test_short_string_id_detected(self):
        self.write("x.txt", f"defect={SECRET_ID}")
        rc, doc, _ = self.check()
        self.assertEqual(rc, 1)
        self.assertTrue(any(h["needle_id"].endswith(".known_defects[0].id") for h in doc["hits"]))

    def test_json_escaped_string_detected(self):
        self.write("kb/snapshot.json", json.dumps({"x": SECRET}, ensure_ascii=True))
        rc, doc, _ = self.check()
        self.assertEqual(rc, 1)
        self.assertIn(("content", "private_string"), self.kinds(doc))

    def test_public_identifier_not_a_hit_and_allow_string(self):
        self.write("public/manifest.txt", "celeris.campaign-case-private/1 k4-test-public-case public-input-v2.tgz")
        rc, doc, _ = self.check()
        self.assertEqual(rc, 1)  # expected.input は既定では公開扱いしない
        rc, doc, _ = self.check("--allow-string", "public-input-v2.tgz")
        self.assertEqual(rc, 0, json.dumps(doc, ensure_ascii=False))
        reasons = [e["reason"] for e in doc["checked"]["private"][0]["excluded"]]
        self.assertIn("allow_string", reasons)

    def test_canary_detected(self):
        self.write("cos/initial-chat.txt", f"user: {CANARY}")
        rc, doc, _ = self.check()
        self.assertEqual(rc, 1)
        self.assertIn(("content", "canary"), self.kinds(doc))

    def test_private_hash_text_detected_case_insensitive(self):
        self.write("operator/brief.md", f"grader digest {self.private_sha.upper()}")
        rc, doc, _ = self.check()
        self.assertEqual(rc, 1)
        self.assertIn(("content", "private_sha256"), self.kinds(doc))

    def test_private_copy_detected_by_content_hash(self):
        shutil.copy(self.private, os.path.join(self.scan, "renamed.bin"))
        rc, doc, _ = self.check()
        self.assertEqual(rc, 1)
        self.assertIn(("file_sha256", "private_sha256"), self.kinds(doc))

    def test_forbidden_sha_file_and_text(self):
        self.write("inputs/old-v1.tgz", OLD_V1, mode="wb")
        rc, doc, _ = self.check()
        self.assertEqual(rc, 1)
        self.assertIn(("file_sha256", "forbidden_sha256"), self.kinds(doc))
        os.remove(os.path.join(self.scan, "inputs/old-v1.tgz"))
        self.write("inputs/list.txt", f"old: {OLD_V1_SHA}")
        rc, doc, _ = self.check()
        self.assertEqual(rc, 1)
        self.assertIn(("content", "forbidden_sha256"), self.kinds(doc))

    def test_needle_across_chunk_boundary(self):
        self.write("big.txt", "a" * 37 + CANARY + "b" * 50)
        rc, doc, _ = self.check("--chunk-bytes", "8")
        self.assertEqual(rc, 1)
        hit = [h for h in doc["hits"] if h["needle_kind"] == "canary"][0]
        self.assertEqual(hit["offset"], 37)

    # ---- archive・git・symlink

    def test_canary_inside_tgz_member(self):
        buf = io.BytesIO()
        with tarfile.open(fileobj=buf, mode="w:gz") as t:
            data = f"hidden {CANARY}".encode()
            info = tarfile.TarInfo("pkg/inner.txt")
            info.size = len(data)
            t.addfile(info, io.BytesIO(data))
        self.write("inputs/public-v2.tgz", buf.getvalue(), mode="wb")
        rc, doc, _ = self.check()
        self.assertEqual(rc, 1)
        hit = [h for h in doc["hits"] if h["needle_kind"] == "canary"][0]
        self.assertEqual(hit["member"], "pkg/inner.txt")

    def test_private_copy_inside_zip_in_tar(self):
        zbuf = io.BytesIO()
        with zipfile.ZipFile(zbuf, "w", zipfile.ZIP_DEFLATED) as z:
            z.write(self.private, "deep/private.json")
        tbuf = io.BytesIO()
        with tarfile.open(fileobj=tbuf, mode="w") as t:
            info = tarfile.TarInfo("a.zip")
            info.size = len(zbuf.getvalue())
            t.addfile(info, io.BytesIO(zbuf.getvalue()))
        self.write("inputs/bundle.tar", tbuf.getvalue(), mode="wb")
        rc, doc, _ = self.check()
        self.assertEqual(rc, 1)
        members = {h.get("member") for h in doc["hits"] if h["kind"] == "file_sha256"}
        self.assertIn("a.zip!deep/private.json", members)

    def test_gzip_single_file(self):
        self.write("memory/notes.gz", gzip.compress(SECRET.encode()), mode="wb")
        rc, doc, _ = self.check()
        self.assertEqual(rc, 1)
        self.assertTrue(any(h.get("member") == "(gunzip)" for h in doc["hits"]))

    @unittest.skipUnless(shutil.which("git"), "git が無い")
    def test_git_history_detected_after_delete(self):
        repo = os.path.join(self.scan, "workspace")
        os.makedirs(repo)
        env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": self.tmp, "GIT_CONFIG_NOSYSTEM": "1"}

        def git(*a):
            subprocess.run(["git", "-c", "user.name=t", "-c", "user.email=t@example.invalid",
                            "-c", "init.defaultBranch=main", "-C", repo] + list(a),
                           check=True, capture_output=True, env=env)
        git("init", "-q")
        with open(os.path.join(repo, "leak.txt"), "w", encoding="utf-8") as f:
            f.write(SECRET)
        git("add", "leak.txt")
        git("commit", "-q", "-m", "add")
        git("rm", "-q", "leak.txt")
        git("commit", "-q", "-m", "remove")
        rc, doc, _ = self.check()
        self.assertEqual(rc, 1)
        hit = [h for h in doc["hits"] if h.get("needle_kind") == "private_string"][0]
        self.assertTrue(hit["member"].startswith("git:"))
        self.assertGreater(doc["checked"]["scan"][0]["git_objects"], 0)

    def test_symlink_to_private_detected(self):
        os.symlink(self.priv_dir, os.path.join(self.scan, "link-dir"))
        rc, doc, _ = self.check()
        self.assertEqual(rc, 1)
        self.assertIn(("symlink", "private_file"), self.kinds(doc))

    def test_out_file_inside_scan_is_excluded(self):
        out = os.path.join(self.scan, "leak-check.json")
        rc, _, _ = self.check(out=out)
        self.assertEqual(rc, 0)
        rc, doc, _ = self.check(out=out)  # 前回の結果（private の hash を含む）を自分で拾わない
        self.assertEqual(rc, 0, json.dumps(doc, ensure_ascii=False))
        self.assertIn(out, doc["checked"]["scan"][0]["excluded"])

    # ---- 可読 path

    def test_deny_readable_world_readable_is_hit(self):
        os.chmod(self.private, 0o644)
        rc, doc, _ = self.check("--deny-readable", self.private)
        self.assertEqual(rc, 1)
        self.assertIn(("deny_readable", None), self.kinds(doc))
        entry = doc["checked"]["deny_readable"][0]
        self.assertTrue(entry["readable"])

    def test_deny_readable_directory_children(self):
        rc, doc, _ = self.check("--deny-readable", self.priv_dir)
        self.assertEqual(rc, 1)
        paths = {h["path"] for h in doc["hits"] if h["kind"] == "deny_readable"}
        self.assertIn(os.path.join(self.priv_dir, "private.json"), paths)

    def test_deny_readable_absent_path_passes(self):
        rc, doc, _ = self.check("--deny-readable", os.path.join(self.tmp, "nowhere", "private.json"))
        self.assertEqual(rc, 0)
        self.assertFalse(doc["checked"]["deny_readable"][0]["exists"])

    @unittest.skipIf(os.geteuid() == 0, "root は mode 000 も読める")
    def test_same_uid_unreadable_records_limit_and_needs_canary(self):
        os.chmod(self.private, 0o000)
        try:
            # private を読まないと needle を作れないので、可読の検査には写しを使う
            hidden = os.path.join(self.tmp, "hidden-copy.json")
            shutil.copy(self.private_copy_source(), hidden)
            os.chmod(hidden, 0o000)
        finally:
            os.chmod(self.private, 0o600)
        rc, doc, _ = self.check("--deny-readable", hidden)
        self.assertEqual(rc, 0, json.dumps(doc, ensure_ascii=False))
        entry = doc["checked"]["deny_readable"][0]
        self.assertFalse(entry["readable"])
        self.assertTrue(entry["same_uid_limit"])
        lim = [x for x in doc["limits"] if x["kind"] == "same_uid"][0]
        self.assertTrue(lim["canary_given"])
        self.assertIn("canary", lim["alternative"])
        rc, doc, _ = self.check("--deny-readable", hidden, canary=False)
        self.assertEqual(rc, 1)
        self.assertIn(("same_uid_without_canary", None), self.kinds(doc))

    def private_copy_source(self):
        os.chmod(self.private, 0o600)
        return self.private

    @unittest.skipIf(os.geteuid() == 0, "root は mode bit に縛られない")
    def test_other_run_uid_by_mode_bits(self):
        other = os.path.join(self.tmp, "other.json")
        with open(other, "w") as f:
            f.write("{}")
        os.chmod(other, 0o600)
        run_uid = os.geteuid() + 12345
        rc, doc, _ = self.check("--deny-readable", other, "--run-uid", str(run_uid), "--run-gid", "54321")
        self.assertEqual(rc, 0, json.dumps(doc, ensure_ascii=False))
        entry = doc["checked"]["deny_readable"][0]
        self.assertFalse(entry["readable"])
        self.assertFalse(entry["same_uid_limit"])
        self.assertEqual(entry["method"], "mode_bits")
        os.chmod(other, 0o604)
        rc, doc, _ = self.check("--deny-readable", other, "--run-uid", str(run_uid), "--run-gid", "54321")
        self.assertEqual(rc, 0)  # mkdtemp の 0700 が祖先の x を塞ぐ
        self.assertEqual(doc["checked"]["deny_readable"][0]["blocked_by"], self.tmp)
        os.chmod(self.tmp, 0o711)
        rc, doc, _ = self.check("--deny-readable", other, "--run-uid", str(run_uid), "--run-gid", "54321")
        # self.tmp より上の祖先が other の x を持つかは試験の環境しだい（run の TMPDIR は 0700 のことがある）
        blocker = None
        cur = os.path.dirname(os.path.realpath(self.tmp))
        while True:
            if not os.stat(cur).st_mode & 0o001:
                blocker = cur
            if cur == os.path.dirname(cur):
                break
            cur = os.path.dirname(cur)
        if blocker is None:
            self.assertEqual(rc, 1)
            self.assertTrue(doc["checked"]["deny_readable"][0]["readable"])
        else:
            self.assertEqual(rc, 0)
            self.assertNotEqual(doc["checked"]["deny_readable"][0]["blocked_by"], self.tmp)

    # ---- runtime probe（celeris.runtime-probe/1 を fixture として注入する。実 uid・userns は使わない）

    def probe(self, name, principal, entries, mounts=(), ok=None, schema="celeris.runtime-probe/1", uid=1000):
        doc = {"schema": schema, "principal": principal, "uid": uid,
               "paths": [{"path": p, "readable": r, "listable": l, "errno": None if r else 13}
                         for p, r, l in entries],
               "mounts_outside_allow": list(mounts),
               "ok": (not any(r for _, r, _ in entries) and not mounts) if ok is None else ok}
        path = os.path.join(self.tmp, name)
        with open(path, "w", encoding="utf-8") as f:
            json.dump(doc, f)
        return path

    def test_runtime_probe_replaces_host_judgement(self):
        os.chmod(self.private, 0o644)  # host から見れば読める
        rt = self.probe("rt.json", "a-agent", [(self.private, False, False)])
        rc, doc, _ = self.check("--deny-readable", self.private, "--runtime-probe", rt)
        self.assertEqual(rc, 0, json.dumps(doc, ensure_ascii=False))
        entry = doc["checked"]["deny_readable"][0]
        self.assertEqual(entry["method"], "runtime_probe")
        self.assertFalse(entry["readable"])
        self.assertTrue(entry["runtime_denied"])
        self.assertFalse(entry["l1_denied"])
        self.assertEqual(doc["checked"]["runtime_probe"][0]["principal"], "a-agent")
        self.assertNotIn("host_view", {x["kind"] for x in doc["limits"]})

    def test_path_without_probe_keeps_host_judgement(self):
        other = os.path.join(self.tmp, "old-answer.json")
        with open(other, "w") as f:
            f.write("{}")
        os.chmod(other, 0o644)
        rt = self.probe("rt.json", "a-agent", [(self.private, False, False)])
        rc, doc, _ = self.check("--deny-readable", self.private, "--deny-readable", other, "--runtime-probe", rt)
        self.assertEqual(rc, 1)
        by_path = {d["path"]: d for d in doc["checked"]["deny_readable"]}
        self.assertEqual(by_path[self.private]["method"], "runtime_probe")
        self.assertEqual(by_path[other]["method"], "os.access")
        self.assertTrue(by_path[other]["readable"])
        hits = [h for h in doc["hits"] if h["kind"] == "deny_readable"]
        self.assertEqual([h["path"] for h in hits], [other])
        self.assertIn("host_view", {x["kind"] for x in doc["limits"]})

    def test_runtime_and_l1_denied_clears_same_uid(self):
        rt = self.probe("rt.json", "b-daemon", [(self.private, False, False)])
        l1 = self.probe("l1.json", "l1:human-uid", [(self.private, False, False)])
        rc, doc, _ = self.check("--deny-readable", self.private, "--runtime-probe", rt, "--runtime-probe", l1,
                                canary=False)
        self.assertEqual(rc, 0, json.dumps(doc, ensure_ascii=False))
        entry = doc["checked"]["deny_readable"][0]
        self.assertFalse(entry["same_uid_limit"])
        self.assertEqual(entry["same_uid_cleared_by"], "runtime_probe_and_l1")
        self.assertNotIn("same_uid", {x["kind"] for x in doc["limits"]})
        self.assertEqual([p["l1"] for p in doc["checked"]["runtime_probe"]], [False, True])

    def test_runtime_denied_without_l1_keeps_same_uid(self):
        rt = self.probe("rt.json", "b-daemon", [(self.private, False, False)])
        rc, doc, _ = self.check("--deny-readable", self.private, "--runtime-probe", rt)
        self.assertEqual(rc, 0, json.dumps(doc, ensure_ascii=False))
        self.assertTrue(doc["checked"]["deny_readable"][0]["same_uid_limit"])
        self.assertIn("same_uid", {x["kind"] for x in doc["limits"]})
        rc, doc, _ = self.check("--deny-readable", self.private, "--runtime-probe", rt, canary=False)
        self.assertEqual(rc, 1)
        self.assertIn(("same_uid_without_canary", None), self.kinds(doc))
        # L1 だけで runtime の拒否が無い場合も同じ
        l1 = self.probe("l1.json", "l1", [(self.private, False, False)])
        rc, doc, _ = self.check("--deny-readable", self.private, "--runtime-probe", l1)
        self.assertTrue(doc["checked"]["deny_readable"][0]["same_uid_limit"])

    def test_probe_readable_true_is_hit(self):
        hidden = os.path.join(self.tmp, "hidden-copy.json")
        shutil.copy(self.private, hidden)
        os.chmod(hidden, 0o000)  # host からは読めなくても、probe が読めたと言えば hit
        rt = self.probe("rt.json", "b-cos", [(hidden, True, True)], ok=True)
        l1 = self.probe("l1.json", "l1", [(hidden, False, False)])
        rc, doc, _ = self.check("--deny-readable", hidden, "--runtime-probe", rt, "--runtime-probe", l1)
        self.assertEqual(rc, 1)
        hit = [h for h in doc["hits"] if h["kind"] == "deny_readable"][0]
        self.assertEqual(hit["path"], hidden)
        self.assertEqual(hit["source"], "runtime_probe")
        self.assertEqual(hit["principal"], "b-cos")
        entry = doc["checked"]["deny_readable"][0]
        self.assertTrue(entry["readable"])
        self.assertTrue(entry["same_uid_limit"] or os.geteuid() == 0)

    def test_l1_probe_readable_true_is_hit(self):
        l1 = self.probe("l1.json", "l1", [("/eval/private/k4/private.json", True, True)])
        rc, doc, _ = self.check("--runtime-probe", l1)
        self.assertEqual(rc, 1)
        hits = [h for h in doc["hits"] if h["kind"] == "deny_readable"]
        self.assertEqual({(h["principal"], h["path"]) for h in hits if "member" not in h},
                         {("l1", "/eval/private/k4/private.json")})
        self.assertTrue(any(h.get("member") == "ok=false" for h in hits))  # 既定の fixture は ok=false も返す

    def test_probe_mounts_outside_allow_is_hit(self):
        rt = self.probe("rt.json", "operator", [(self.private, False, False)],
                        mounts=[{"source": "/home/rmaeda", "target": "/home/rmaeda"}])
        rc, doc, _ = self.check("--deny-readable", self.private, "--runtime-probe", rt)
        self.assertEqual(rc, 1)
        hit = [h for h in doc["hits"] if h["kind"] == "deny_readable"][0]
        self.assertEqual(hit["member"], "mounts_outside_allow")

    def test_probe_ok_false_is_hit(self):
        rt = self.probe("rt.json", "a-agent", [(self.private, False, False)], ok=False)
        rc, doc, _ = self.check("--deny-readable", self.private, "--runtime-probe", rt)
        self.assertEqual(rc, 1)
        self.assertTrue(any(h.get("member") == "ok=false" for h in doc["hits"]))

    def test_canary_still_detected_with_probe(self):
        self.write("ws/out.txt", CANARY)
        rt = self.probe("rt.json", "a-agent", [(self.private, False, False)])
        l1 = self.probe("l1.json", "l1", [(self.private, False, False)])
        rc, doc, _ = self.check("--deny-readable", self.private, "--runtime-probe", rt, "--runtime-probe", l1)
        self.assertEqual(rc, 1)
        self.assertIn(("content", "canary"), self.kinds(doc))

    def test_probe_wrong_schema_or_shape_is_input_error(self):
        bad = self.probe("bad.json", "a-agent", [(self.private, False, False)], schema="celeris.runtime-probe/2")
        rc, doc, _ = self.check("--runtime-probe", bad)
        self.assertEqual(rc, 2)
        self.assertIn("schema", doc["error"])
        with open(os.path.join(self.tmp, "shape.json"), "w") as f:
            json.dump({"schema": "celeris.runtime-probe/1", "principal": "a", "uid": 1, "ok": True,
                       "mounts_outside_allow": [], "paths": [{"path": "/x", "readable": "no", "listable": False}]}, f)
        rc, doc, _ = self.check("--runtime-probe", os.path.join(self.tmp, "shape.json"))
        self.assertEqual(rc, 2)
        self.assertIn("readable", doc["error"])
        with open(os.path.join(self.tmp, "nojson.json"), "w") as f:
            f.write("not json")
        rc, _, _ = self.check("--runtime-probe", os.path.join(self.tmp, "nojson.json"))
        self.assertEqual(rc, 2)

    # ---- 入力の誤り

    def test_bad_forbidden_sha_is_input_error(self):
        p = subprocess.run([sys.executable, "-I", SCRIPT, "--private", self.private, "--forbidden-sha", "xyz",
                            "--scan", self.scan, "--out", self.out], capture_output=True, text=True)
        self.assertEqual(p.returncode, 2)
        with open(self.out, encoding="utf-8") as f:
            doc = json.load(f)
        self.assertFalse(doc["pass"])
        self.assertIn("forbidden-sha", doc["error"])

    def test_missing_scan_is_input_error(self):
        p = subprocess.run([sys.executable, "-I", SCRIPT, "--private", self.private,
                            "--scan", os.path.join(self.tmp, "nope"), "--out", self.out],
                           capture_output=True, text=True)
        self.assertEqual(p.returncode, 2)


if __name__ == "__main__":
    unittest.main()
