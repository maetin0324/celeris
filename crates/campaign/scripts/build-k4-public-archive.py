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

manifest.json: 旧/新 tgz の sha256・size、残した entry（path・type・sha256）、除いた entry
（path・type・sha256・reason）、除外リストの sha256。時刻は書かない（再生成で同じ内容になる）。
exit: 0 = 成功 / 1 = 入力の不整合 / 2 = 使い方の誤り。
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
        "source": {"path": os.path.abspath(src), "sha256": src_sha_before, "size": os.path.getsize(src),
                   "entries": len(entries)},
        "output": {"path": os.path.abspath(out), "sha256": sha256_file(out), "size": os.path.getsize(out),
                   "entries": len(keep)},
        "exclude_list": {"path": os.path.abspath(exclude), "sha256": sha256_file(exclude), "entries": len(excl)},
        "normalization": {"order": "name", "mtime": 0, "uid": 0, "gid": 0, "uname": "", "gname": "",
                          "mode": "source", "tar_format": "pax", "gzip_mtime": 0, "gzip_level": 9},
        "kept": [row(n) for n in keep],
        "excluded": [row(n, {"reason": reasons[n]}) for n in sorted(excl)],
    }
    tmp = manifest + ".partial"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(doc, f, ensure_ascii=False, indent=1, sort_keys=False)
        f.write("\n")
    os.replace(tmp, manifest)
    return doc


def main(argv):
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
                      "kept": len(doc["kept"]), "excluded": len(doc["excluded"])}))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
