#!/usr/bin/env python3
"""K4 の公開入力 tgz から派生物だけを除いた決定的な新 tgz を作る（標準ライブラリのみ・LLM なし）。

使い方:
  python3 -I crates/campaign/scripts/build-k4-public-archive.py \
      --src <旧 tgz> --exclude <除外リスト> --out <新 tgz> --manifest <manifest.json>

除外リストは 1 行 1 entry path（空行と # で始まる行は無視）。監査
（agent-docs/progress/2026-10-25-k4-public-input-audit/audit.md）の除外案をそのまま書く。

- 除外リストの path は旧 tgz に実在しなければならない。dir を除くならその下の entry もすべて書く。
- 除外理由は EXCLUDE_REASONS の path 規則で決める。どの規則にも当たらない path は失敗にする
  （理由なしで除かない）。
- 残す entry は名前順に並べ、bytes と mode は旧版のまま、mtime=0・uid/gid=0・uname/gname 空にする。
  gzip header も mtime=0・file 名なし。同じ入力から 2 回作ると同じ sha256 になる。
- 書いた後に新 tgz を読み直し、残した各 entry の sha256 が旧版と同じことを確かめる。
- 旧 tgz と同じ file には書かない。新 tgz は一時名に書いてから rename する。
- symlink・hardlink・device 等の regular file/dir 以外の entry は失敗にする。

manifest.json: old_sha256・new_sha256（source/output に path・size も）、残した entry kept
（path・type・sha256）、除いた entry removed（path・type・sha256・reason）、除外リストの sha256。
時刻は書かない（再生成で同じ内容になる）。
exit: 0 = 成功 / 1 = 入力の不整合 / 2 = 使い方の誤り。

自己試験（合成 tgz）: python3 -I crates/campaign/scripts/build-k4-public-archive.py --self-test
"""
import argparse
import gzip
import hashlib
import io
import json
import os
import re
import sys
import tarfile

SCHEMA = "celeris.k4-public-archive/1"

# 除外理由。上から順に最初に当たった規則を使う（監査 audit.json の rationale の要約）。
EXCLUDE_REASONS = [
    (r"(^|/)__pycache__(/|$)", "Python の bytecode cache（実行の副産物）"),
    (r"^review28/(review|log-review)-?[^/]*\.txt$|^review28/log-review\.txt$",
     "log 審査の出力（pass/fail 件数と note）。旧判定の集計"),
    (r"^review28/cells-[0-9]+\.csv$",
     "旧判定 CSV（セルごとの valid/invalid・理由・数値列）。解答表に相当する派生物"),
    (r"^review28/(nic-audit\.txt|log-survey\.txt)$",
     "job の log・NIC 審査から作った抜粋・判定。派生"),
    (r"^review28/(manifest-[0-9]+\.json|archive-[0-9]+\.sha256)$",
     "collect.py が作った file 目録・別 tgz の sha256。派生"),
    (r"^review28/(accounting\.json|v3-released\.json)$",
     "collect.py が qstat から計算した vnode 時間の会計。派生の集計"),
    (r"^review28/[^/]+\.py$|^runs/[^/]+/runner/(summarize|audit-nic)\.py$",
     "集計・判定 script（実行すると旧判定を再現する生成器とその付属物）"),
    (r"^runs/[^/]+/fs/.+/logs-audit\.json$",
     "review-v3.py が job 後に書いた log 審査の判定（pass・reasons・note）。旧判定"),
    (r"^runs/[^/]+/cells/.+/nic-audit\.json$",
     "audit-nic.py が job 後に書いた NIC 選択の判定（pass）。旧判定"),
]


class Fail(Exception):
    pass


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def reason_for(path):
    for pat, reason in EXCLUDE_REASONS:
        if re.search(pat, path):
            return reason
    return None


def read_exclude(path):
    out = []
    with open(path, encoding="utf-8") as f:
        for line in f:
            p = line.strip()
            if not p or p.startswith("#"):
                continue
            out.append(p)
    if len(set(out)) != len(out):
        raise Fail(f"{path}: duplicate path in exclude list")
    return out


def entry_type(m):
    if m.isfile():
        return "file"
    if m.isdir():
        return "dir"
    raise Fail(f"{m.name}: unsupported entry type {m.type!r} (only regular files and dirs)")


def load_src(src):
    """旧 tgz の entry を (member, bytes, sha256) で読む。"""
    entries = {}
    with tarfile.open(src, "r:gz") as t:
        for m in t.getmembers():
            kind = entry_type(m)
            name = m.name.rstrip("/")
            if name in entries:
                raise Fail(f"{src}: duplicate member {name}")
            if name.startswith("/") or ".." in name.split("/"):
                raise Fail(f"{src}: unsafe member name {name}")
            data = b""
            if kind == "file":
                f = t.extractfile(m)
                if f is None:
                    raise Fail(f"{src}: cannot read {name}")
                data = f.read()
            entries[name] = (m, kind, data, hashlib.sha256(data).hexdigest() if kind == "file" else None)
    return entries


def write_tgz(path, entries, keep):
    tmp = path + ".partial"
    with open(tmp, "wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0, compresslevel=9) as gz:
            with tarfile.open(fileobj=gz, mode="w", format=tarfile.PAX_FORMAT) as t:
                for name in keep:
                    m, kind, data, _ = entries[name]
                    ti = tarfile.TarInfo(name)
                    ti.mode = m.mode & 0o7777
                    ti.mtime = 0
                    ti.uid = ti.gid = 0
                    ti.uname = ti.gname = ""
                    if kind == "dir":
                        ti.type = tarfile.DIRTYPE
                        t.addfile(ti)
                    else:
                        ti.type = tarfile.REGTYPE
                        ti.size = len(data)
                        t.addfile(ti, io.BytesIO(data))
    os.replace(tmp, path)


def build(src, exclude, out, manifest):
    if not os.path.isfile(src):
        raise Fail(f"{src}: source archive is missing")
    if os.path.exists(out) and os.path.samefile(src, out):
        raise Fail("--out must not be the source archive")
    if os.path.abspath(manifest) in (os.path.abspath(src), os.path.abspath(out)):
        raise Fail("--manifest must differ from --src and --out")
    src_sha_before = sha256_file(src)
    entries = load_src(src)
    excl = read_exclude(exclude)

    missing = [p for p in excl if p not in entries]
    if missing:
        raise Fail(f"exclude list has paths not in the source archive: {missing}")
    excl_set = set(excl)
    for p in excl:
        if entries[p][1] == "dir":
            orphans = sorted(n for n in entries if n.startswith(p + "/") and n not in excl_set)
            if orphans:
                raise Fail(f"excluded dir {p} has entries not in the exclude list: {orphans}")
    reasons = {}
    for p in excl:
        r = reason_for(p)
        if r is None:
            raise Fail(f"{p}: no exclusion reason rule matches (only derived files may be excluded)")
        reasons[p] = r

    keep = sorted(n for n in entries if n not in excl_set)
    os.makedirs(os.path.dirname(os.path.abspath(out)), exist_ok=True)
    write_tgz(out, entries, keep)

    # 読み直して、残した entry の bytes が旧版と同じことを確かめる。
    written = load_src(out)
    if sorted(written) != keep:
        raise Fail(f"{out}: written entry set differs from the kept set")
    for name in keep:
        if written[name][1] != entries[name][1] or written[name][3] != entries[name][3]:
            raise Fail(f"{out}: {name} differs from the source entry")
    if sha256_file(src) != src_sha_before:
        raise Fail(f"{src}: source archive changed during the build")

    new_sha = sha256_file(out)

    def row(name, extra=None):
        _, kind, data, sha = entries[name]
        r = {"path": name, "type": kind}
        if kind == "file":
            r["size"] = len(data)
            r["sha256"] = sha
        if extra:
            r.update(extra)
        return r

    doc = {
        "schema": SCHEMA,
        "changed": True,
        "old_sha256": src_sha_before,
        "new_sha256": new_sha,
        "source": {"path": os.path.abspath(src), "sha256": src_sha_before, "size": os.path.getsize(src),
                   "entries": len(entries)},
        "output": {"path": os.path.abspath(out), "sha256": new_sha, "size": os.path.getsize(out),
                   "entries": len(keep)},
        "exclude_list": {"path": os.path.abspath(exclude), "sha256": sha256_file(exclude), "entries": len(excl)},
        "normalization": {"order": "name", "mtime": 0, "uid": 0, "gid": 0, "uname": "", "gname": "",
                          "mode": "source", "tar_format": "pax", "gzip_mtime": 0, "gzip_level": 9},
        "kept": [row(n) for n in keep],
        "removed": [row(n, {"reason": reasons[n]}) for n in sorted(excl)],
    }
    tmp = manifest + ".partial"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(doc, f, ensure_ascii=False, indent=1, sort_keys=False)
        f.write("\n")
    os.replace(tmp, manifest)
    return doc


# ---- 自己試験（--self-test。標準ライブラリのみ・合成 tgz） ----

def self_test():
    import subprocess
    import tempfile
    import unittest
    from pathlib import Path
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
            return subprocess.run([sys.executable, "-I", os.path.abspath(__file__), "--src", str(self.src),
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
            self.assertEqual(m["old_sha256"], src_sha)
            self.assertEqual(m["new_sha256"], m["output"]["sha256"])
            self.assertEqual(m["output"]["sha256"], sha((self.dir / "a.tgz").read_bytes()))
            self.assertEqual(sorted(e["path"] for e in m["removed"]), sorted(EXCLUDE))
            self.assertTrue(all(e["reason"] for e in m["removed"]))
            kept = {e["path"]: e for e in m["kept"]}
            self.assertEqual(kept["review28/qstat-43706.txt"]["sha256"], sha(FILES["review28/qstat-43706.txt"]))
            # out の path 以外は同じ内容（時刻を書かない）。
            m2 = json.loads((self.dir / "b.json").read_text())
            for d in (m, m2):
                d["output"].pop("path")
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

    suite = unittest.defaultTestLoader.loadTestsFromTestCase(Build)
    return 0 if unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful() else 1


def main(argv):
    if argv == ["--self-test"]:
        return self_test()
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--src", required=True)
    ap.add_argument("--exclude", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--manifest", required=True)
    try:
        a = ap.parse_args(argv)
    except SystemExit as e:
        return 2 if e.code else 0
    try:
        doc = build(a.src, a.exclude, a.out, a.manifest)
    except (Fail, OSError, tarfile.TarError) as e:
        print(f"build-k4-public-archive: {e}", file=sys.stderr)
        return 1
    print(json.dumps({"source_sha256": doc["source"]["sha256"], "output_sha256": doc["output"]["sha256"],
                      "kept": len(doc["kept"]), "removed": len(doc["removed"])}))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
