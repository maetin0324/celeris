#!/usr/bin/env python3
"""ops-r1 の漏洩検査: grader-private・旧判定・旧 v1 入力が試行の入力と可読 path に無いことを確かめる。

    python3 -I scripts/campaign/check-ops-r1-leak.py --private <private.json> [--private ...] \
        [--forbidden-sha <sha256> ...] [--canary <文字列> ...] --scan <dir|file> [--scan ...] \
        [--deny-readable <path> ...] --out <json>

検出するもの（どれか 1 つでも見つかれば exit 1。入力の誤りは exit 2。見つからなければ exit 0）:

- scan 先の file（tar/tgz・zip・gzip の member と git の object を含む）に、次のどれかが現れること。
  - private.json の sha256（16 進。file の内容が一致する写しも）
  - private.json の 12 字以上の文字列値（UTF-8 のままと JSON の \\uXXXX 表記の両方）。ただし公開入力にも載る
    識別子（既定は JSON の .schema・.case_id・.split）と --allow-string の文字列は除く。除いたものは結果に書く
  - --canary の文字列
  - --forbidden-sha の sha256（16 進の記載と、内容がその hash になる file の両方。旧 v1 tgz・旧判定の写し等）
- scan 先の symlink が private.json・--deny-readable の path・禁止 hash の file を指すこと。
- --deny-readable の path が測定の run の uid/gid（既定はこの process の uid/gid。--run-uid・--run-gid で指定）
  から読めること。ディレクトリなら中の file も 1 つずつ見る。

同じ uid の限界: run の uid が path か祖先ディレクトリの持ち主なら、run は chmod・rename で読めるようにできる。
この検査はその path を「今読めない」と判定しても漏洩を防げないので、limits に same_uid を書き、代わりの検出として
--canary の出現検査を求める。canary が 1 つも無ければ same_uid_without_canary の hit にして exit 1 にする。
mode bit の判定は host から見た今の状態で、run の sandbox（bwrap の mount 等）が見える範囲を狭めることは数えない
（多めに読めると判定する側に倒れる）。

結果 JSON（--out）には pass・hits・checked・limits を書く。hit には文字列そのものを書かず、needle の id・
長さ・sha256 だけを書く（結果 file 自体が漏洩源にならないように）。ただし checked.private[].sha256 は
private.json の hash なので、結果 file を scan 先に置かない（--out の path は scan から外す）。
"""
import argparse
import gzip
import hashlib
import io
import json
import os
import stat
import subprocess
import sys
import tarfile
import zipfile

SCHEMA = "celeris.ops-r1-leak-check/1"
DEFAULT_MIN_LEN = 12
DEFAULT_PUBLIC_PATHS = (".schema", ".case_id", ".split")
DEFAULT_CHUNK = 1 << 20
MAX_ARCHIVE_DEPTH = 3
HIT_LIST_CAP = 1000


class InputError(Exception):
    pass


def sha256_hex(data):
    return hashlib.sha256(data).hexdigest()


def is_hex64(s):
    return len(s) == 64 and all(c in "0123456789abcdefABCDEF" for c in s)


# ---------------------------------------------------------------- needles


class Needle:
    def __init__(self, nid, kind, text):
        self.id = nid
        self.kind = kind
        self.length = len(text)
        self.sha256 = sha256_hex(text.encode("utf-8"))
        variants = {text.encode("utf-8")}
        escaped = json.dumps(text, ensure_ascii=True)[1:-1]
        variants.add(escaped.encode("ascii"))
        variants.add(json.dumps(text, ensure_ascii=False)[1:-1].encode("utf-8"))
        if kind in ("private_sha256", "forbidden_sha256"):
            variants.add(text.lower().encode("ascii"))
            variants.add(text.upper().encode("ascii"))
        self.variants = sorted(v for v in variants if v)

    def describe(self):
        return {"needle_id": self.id, "needle_kind": self.kind, "needle_len": self.length,
                "needle_sha256": self.sha256}


def collect_strings(value, path, out):
    if isinstance(value, dict):
        for k, v in value.items():
            collect_strings(v, f"{path}.{k}", out)
    elif isinstance(value, list):
        for i, v in enumerate(value):
            collect_strings(v, f"{path}[{i}]", out)
    elif isinstance(value, str):
        out.append((path or ".", value))


def load_private(path, min_len, public_paths, allow):
    try:
        with open(path, "rb") as f:
            raw = f.read()
    except OSError as e:
        raise InputError(f"--private {path}: {e}")
    try:
        doc = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as e:
        raise InputError(f"--private {path}: JSON でない: {e}")
    strings = []
    collect_strings(doc, "", strings)
    used, excluded, seen = [], [], set()
    for jpath, s in strings:
        if len(s) < min_len:
            continue
        if jpath in public_paths:
            excluded.append({"json_path": jpath, "len": len(s), "sha256": sha256_hex(s.encode("utf-8")),
                             "reason": "public_identifier"})
            continue
        if s in allow:
            excluded.append({"json_path": jpath, "len": len(s), "sha256": sha256_hex(s.encode("utf-8")),
                             "reason": "allow_string"})
            continue
        if s in seen:
            continue
        seen.add(s)
        used.append((jpath, s))
    return raw, used, excluded, len(strings)


# ---------------------------------------------------------------- scanning


class Scanner:
    def __init__(self, needles, content_hashes, link_targets, chunk, exclude_real):
        self.needles = needles
        self.max_len = max((len(v) for n in needles for v in n.variants), default=1)
        self.content_hashes = content_hashes  # sha256 -> (kind, id)
        self.link_targets = link_targets  # realpath -> (kind, id)
        self.chunk = chunk
        self.exclude_real = exclude_real
        self.hits = []
        self.hit_count = 0
        self.seen_hits = set()

    def add_hit(self, hit):
        key = (hit.get("kind"), hit.get("path"), hit.get("member"), hit.get("needle_id"))
        if key in self.seen_hits:
            return
        self.seen_hits.add(key)
        self.hit_count += 1
        if len(self.hits) < HIT_LIST_CAP:
            self.hits.append(hit)

    def scan_stream(self, stream, path, member, stats):
        """stream を chunk ごとに読み、needle を探し、内容の sha256 を返す。"""
        h = hashlib.sha256()
        tail = b""
        offset = 0  # tail の先頭の、stream 内の位置
        found = set()
        head = b""
        while True:
            block = stream.read(self.chunk)
            if not block:
                break
            if len(head) < 1024:
                head += block[:1024 - len(head)]
            h.update(block)
            stats["bytes"] += len(block)
            buf = tail + block
            for n in self.needles:
                if n.id in found:
                    continue
                for v in n.variants:
                    pos = buf.find(v)
                    if pos >= 0:
                        found.add(n.id)
                        hit = {"kind": "content", "path": path, "offset": offset + pos}
                        if member is not None:
                            hit["member"] = member
                        hit.update(n.describe())
                        self.add_hit(hit)
                        break
            keep = self.max_len - 1
            if keep > 0:
                new_tail = buf[-keep:] if len(buf) > keep else buf
            else:
                new_tail = b""
            offset += len(buf) - len(new_tail)
            tail = new_tail
        digest = h.hexdigest()
        if digest in self.content_hashes:
            kind, nid = self.content_hashes[digest]
            hit = {"kind": "file_sha256", "path": path, "needle_id": nid, "needle_kind": kind,
                   "needle_sha256": digest}
            if member is not None:
                hit["member"] = member
            self.add_hit(hit)
        return digest, head

    def scan_bytes_member(self, data, path, member, stats, depth):
        stats["archive_members"] += 1
        _, head = self.scan_stream(io.BytesIO(data), path, member, stats)
        self.maybe_archive(lambda: io.BytesIO(data), head, path, member, stats, depth)

    def maybe_archive(self, opener, head, path, member, stats, depth):
        if depth >= MAX_ARCHIVE_DEPTH:
            return
        prefix = f"{member}!" if member is not None else ""
        try:
            if head[:4] == b"PK\x03\x04":
                with zipfile.ZipFile(opener()) as z:
                    for info in z.infolist():
                        if info.is_dir():
                            continue
                        with z.open(info) as m:
                            self.scan_member_stream(m, path, prefix + info.filename, stats, depth + 1)
                return
            if is_tar_head(head) or head[:2] == b"\x1f\x8b" or head[:3] == b"BZh" or head[:6] == b"\xfd7zXZ\x00":
                f = opener()
                try:
                    with tarfile.open(fileobj=f, mode="r:*") as t:
                        for info in t:
                            if not info.isfile():
                                continue
                            m = t.extractfile(info)
                            if m is None:
                                continue
                            self.scan_member_stream(m, path, prefix + info.name, stats, depth + 1)
                    return
                except tarfile.TarError:
                    pass
                if head[:2] == b"\x1f\x8b":
                    with gzip.GzipFile(fileobj=opener()) as g:
                        self.scan_member_stream(g, path, prefix + "(gunzip)", stats, depth + 1)
        except (OSError, EOFError, zipfile.BadZipFile, tarfile.TarError) as e:
            stats["unreadable"].append({"path": path, "member": member, "error": f"archive: {e}"})

    def scan_member_stream(self, stream, path, member, stats, depth):
        # member は seek できないことがあるので、archive 判定のために読み切って持つ
        data = stream.read()
        self.scan_bytes_member(data, path, member, stats, depth)

    def scan_file(self, path, stats):
        try:
            with open(path, "rb") as f:
                stats["files"] += 1
                _, head = self.scan_stream(f, path, None, stats)
        except OSError as e:
            stats["unreadable"].append({"path": path, "error": str(e)})
            return
        self.maybe_archive(lambda: open(path, "rb"), head, path, None, stats, 0)

    def check_link(self, path, stats):
        stats["symlinks"] += 1
        real = os.path.realpath(path)
        for target, (kind, nid) in self.link_targets.items():
            inside = real.startswith(target.rstrip(os.sep) + os.sep)
            contains = os.path.isdir(real) and target.startswith(real.rstrip(os.sep) + os.sep)
            if real == target or inside or contains:
                self.add_hit({"kind": "symlink", "path": path, "target": real, "needle_kind": kind,
                              "needle_id": nid})
        return real

    def scan_git(self, repo_dir, stats):
        cmd = ["git", "-c", "safe.directory=*", "-c", "core.fsmonitor=false", "-C", repo_dir,
               "cat-file", "--batch-all-objects", "--batch"]
        env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
        env["GIT_CONFIG_NOSYSTEM"] = "1"
        try:
            p = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, stdin=subprocess.DEVNULL,
                                 env=env)
        except OSError as e:
            return f"git を起動できない: {e}"
        out = p.stdout
        while True:
            header = out.readline()
            if not header:
                break
            parts = header.decode("ascii", "replace").split()
            if len(parts) != 3 or parts[1] == "missing":
                continue
            oid, size = parts[0], int(parts[2])
            data = out.read(size)
            out.read(1)
            stats["git_objects"] += 1
            self.scan_stream(io.BytesIO(data), repo_dir, f"git:{oid}", stats)
        err = p.stderr.read().decode("utf-8", "replace")
        if p.wait() != 0:
            return f"git cat-file が失敗: {err.strip()[:200]}"
        return None

    def scan_root(self, root, stats, limits):
        visited = set()

        def walk_dir(d):
            real = os.path.realpath(d)
            if real in visited:
                return
            visited.add(real)
            try:
                entries = sorted(os.scandir(d), key=lambda e: e.name)
            except OSError as e:
                stats["unreadable"].append({"path": d, "error": str(e)})
                return
            if any(e.name == ".git" for e in entries):
                err = self.scan_git(d, stats)
                stats["git_repos"].append({"path": d, "error": err})
                if err:
                    limits.append({"kind": "git_objects_unscanned", "path": d, "detail": err,
                                   "note": "git の object（圧縮）を読めなかった。.git の生 byte だけを見た"})
            for e in entries:
                p = e.path
                if os.path.realpath(p) in self.exclude_real:
                    stats["excluded"].append(p)
                    continue
                try:
                    if e.is_symlink():
                        real_t = self.check_link(p, stats)
                        if os.path.isdir(real_t):
                            walk_dir(p)
                        elif os.path.isfile(real_t):
                            self.scan_file(p, stats)
                        continue
                    if e.is_dir(follow_symlinks=False):
                        if e.name == ".git" and not any(g["path"] == d and g["error"] for g in stats["git_repos"]):
                            continue  # object は git cat-file で読んだ
                        walk_dir(p)
                    elif e.is_file(follow_symlinks=False):
                        if e.name == ".git":
                            continue
                        self.scan_file(p, stats)
                    else:
                        stats["skipped_special"] += 1
                except OSError as err:
                    stats["unreadable"].append({"path": p, "error": str(err)})

        if os.path.islink(root):
            self.check_link(root, stats)
        if os.path.isdir(root):
            walk_dir(root)
        elif os.path.isfile(root):
            self.scan_file(root, stats)


def is_tar_head(head):
    return len(head) >= 262 and head[257:262] == b"ustar"


# ---------------------------------------------------------------- readability


def mode_allows(st, uid, gids, bits):
    """bits: (owner_bit, group_bit, other_bit) の組のどれか（r や x）が許すか。root は常に許す（x は除く）。"""
    if uid == 0:
        return True
    if st.st_uid == uid:
        return bool(st.st_mode & bits[0])
    if st.st_gid in gids:
        return bool(st.st_mode & bits[1])
    return bool(st.st_mode & bits[2])


READ = (stat.S_IRUSR, stat.S_IRGRP, stat.S_IROTH)
EXEC = (stat.S_IXUSR, stat.S_IXGRP, stat.S_IXOTH)


def has_acl(path):
    try:
        return "system.posix_acl_access" in os.listxattr(path, follow_symlinks=False)
    except (OSError, AttributeError):
        return False


def judge_readable(path, uid, gids, same_process):
    """path が uid/gids から読めるか。(readable, detail) を返す。"""
    real = os.path.realpath(path)
    try:
        st = os.stat(real)
    except OSError as e:
        return False, {"error": str(e)}
    detail = {"realpath": real, "owner_uid": st.st_uid, "owner_gid": st.st_gid,
              "mode": oct(stat.S_IMODE(st.st_mode)), "is_dir": stat.S_ISDIR(st.st_mode)}
    if same_process:
        readable = os.access(real, os.R_OK) and (not detail["is_dir"] or os.access(real, os.X_OK))
        detail["method"] = "os.access"
        return readable, detail
    # 祖先の x と自身の r（ディレクトリは r と x）
    parent = os.path.dirname(real)
    while True:
        try:
            pst = os.stat(parent)
        except OSError:
            return False, dict(detail, error=f"祖先 {parent} を stat できない")
        if not mode_allows(pst, uid, gids, EXEC):
            detail["method"] = "mode_bits"
            detail["blocked_by"] = parent
            return False, detail
        if parent == os.path.dirname(parent):
            break
        parent = os.path.dirname(parent)
    readable = mode_allows(st, uid, gids, READ)
    if detail["is_dir"]:
        readable = readable and mode_allows(st, uid, gids, EXEC)
    detail["method"] = "mode_bits"
    if has_acl(real):
        detail["acl_present"] = True
    return readable, detail


def owned_chain(path, uid):
    """path か祖先ディレクトリで、uid が持ち主のもの（chmod・rename できる）を返す。root は全部。"""
    real = os.path.realpath(path)
    cur = real if os.path.lexists(real) else os.path.dirname(real)
    owned = []
    while True:
        try:
            st = os.stat(cur)
            if uid == 0 or st.st_uid == uid:
                owned.append(cur)
        except OSError:
            pass
        if cur == os.path.dirname(cur):
            break
        cur = os.path.dirname(cur)
    return owned


def check_deny(path, uid, gids, same_process, scanner, limits, have_canary):
    entry = {"path": path, "exists": os.path.lexists(path)}
    if not entry["exists"]:
        entry["readable"] = False
        return entry
    readable, detail = judge_readable(path, uid, gids, same_process)
    entry.update(readable=readable, **detail)
    if readable:
        scanner.add_hit({"kind": "deny_readable", "path": path, "detail": detail})
    readable_children = 0
    if detail.get("is_dir") and readable:
        for d, dirs, files in os.walk(detail["realpath"]):
            for name in files:
                fp = os.path.join(d, name)
                r, det = judge_readable(fp, uid, gids, same_process)
                if r:
                    readable_children += 1
                    scanner.add_hit({"kind": "deny_readable", "path": fp, "detail": det})
    entry["readable_children"] = readable_children
    owned = owned_chain(path, uid)
    entry["same_uid_limit"] = bool(owned)
    if owned:
        limits.append({
            "kind": "same_uid", "path": path, "owned_by_run_uid": owned[:5],
            "note": "run の uid がこの path か祖先の持ち主なので、mode を絞っても run は chmod・rename で読める。"
                    "可読でないことの判定では漏洩を防げない",
            "alternative": "canary の出現検査（--canary を private の写しに入れ、scan 先に現れないことを見る）",
            "canary_given": have_canary,
        })
        if not have_canary:
            scanner.add_hit({"kind": "same_uid_without_canary", "path": path,
                             "detail": "同じ uid の限界があるのに --canary が無く、代わりの検出が無い"})
    return entry


# ---------------------------------------------------------------- main


def parse_args(argv):
    ap = argparse.ArgumentParser(description="ops-r1 の grader-private 漏洩検査")
    ap.add_argument("--private", action="append", default=[], required=True)
    ap.add_argument("--forbidden-sha", action="append", default=[])
    ap.add_argument("--canary", action="append", default=[])
    ap.add_argument("--scan", action="append", default=[], required=True)
    ap.add_argument("--deny-readable", action="append", default=[])
    ap.add_argument("--out", required=True)
    ap.add_argument("--run-uid", type=int, default=None, help="測定の run の uid（既定はこの process の uid）")
    ap.add_argument("--run-gid", type=int, action="append", default=None,
                    help="測定の run の gid（複数可。既定はこの process の gid と補助 group）")
    ap.add_argument("--allow-string", action="append", default=[],
                    help="private の文字列のうち公開入力にも載るもの（検査から外し、結果に書く）")
    ap.add_argument("--min-len", type=int, default=DEFAULT_MIN_LEN)
    ap.add_argument("--chunk-bytes", type=int, default=DEFAULT_CHUNK, help=argparse.SUPPRESS)
    return ap.parse_args(argv)


def run(args):
    if args.min_len < 1 or args.chunk_bytes < 1:
        raise InputError("--min-len と --chunk-bytes は 1 以上")
    for s in args.forbidden_sha:
        if not is_hex64(s):
            raise InputError(f"--forbidden-sha が 64 桁の 16 進でない: {s!r}")
    for c in args.canary:
        if not c:
            raise InputError("--canary が空")
    for p in args.scan:
        if not os.path.lexists(p):
            raise InputError(f"--scan {p} が無い")

    needles, content_hashes, link_targets = [], {}, {}
    checked_private = []
    allow = set(args.allow_string)
    for i, p in enumerate(args.private):
        raw, used, excluded, total = load_private(p, args.min_len, DEFAULT_PUBLIC_PATHS, allow)
        digest = sha256_hex(raw)
        needles.append(Needle(f"private[{i}].sha256", "private_sha256", digest))
        content_hashes[digest] = ("private_sha256", f"private[{i}].sha256")
        link_targets[os.path.realpath(p)] = ("private_file", f"private[{i}]")
        for j, (jpath, s) in enumerate(used):
            needles.append(Needle(f"private[{i}].strings[{j}]{jpath}", "private_string", s))
        checked_private.append({"path": p, "sha256": digest, "strings_total": total,
                                "strings_checked": len(used), "excluded": excluded})
    for i, s in enumerate(args.forbidden_sha):
        needles.append(Needle(f"forbidden_sha[{i}]", "forbidden_sha256", s.lower()))
        content_hashes.setdefault(s.lower(), ("forbidden_sha256", f"forbidden_sha[{i}]"))
    for i, c in enumerate(args.canary):
        needles.append(Needle(f"canary[{i}]", "canary", c))
    for i, p in enumerate(args.deny_readable):
        link_targets.setdefault(os.path.realpath(p), ("deny_readable", f"deny_readable[{i}]"))

    exclude_real = {os.path.realpath(args.out)}
    scanner = Scanner(needles, content_hashes, link_targets, args.chunk_bytes, exclude_real)
    limits = []
    scans = []
    for root in args.scan:
        stats = {"path": root, "files": 0, "bytes": 0, "archive_members": 0, "git_objects": 0,
                 "symlinks": 0, "skipped_special": 0, "git_repos": [], "unreadable": [], "excluded": []}
        scanner.scan_root(root, stats, limits)
        if stats["unreadable"]:
            limits.append({"kind": "unreadable_in_scan", "path": root, "count": len(stats["unreadable"]),
                           "note": "この process が読めなかった file は検査していない（run が別の uid なら読める可能性がある）"})
        scans.append(stats)

    uid = os.geteuid() if args.run_uid is None else args.run_uid
    gids = set(args.run_gid) if args.run_gid else ({os.getegid()} | set(os.getgroups()))
    same_process = args.run_uid in (None, os.geteuid()) and args.run_gid is None
    deny = [check_deny(p, uid, gids, same_process, scanner, limits, bool(args.canary))
            for p in args.deny_readable]
    limits.append({"kind": "host_view", "note": "可読の判定は host から見た今の mode・owner で行う。run の sandbox が"
                                                "見える範囲を狭める効果は数えない（多めに読めると判定する）"})

    return {
        "schema": SCHEMA,
        "pass": scanner.hit_count == 0,
        "hit_count": scanner.hit_count,
        "hits": scanner.hits,
        "checked": {
            "private": checked_private,
            "forbidden_sha256": [s.lower() for s in args.forbidden_sha],
            "canary": [{"len": len(c), "sha256": sha256_hex(c.encode("utf-8"))} for c in args.canary],
            "needles": len(needles),
            "min_len": args.min_len,
            "public_identifier_paths": list(DEFAULT_PUBLIC_PATHS),
            "scan": scans,
            "deny_readable": deny,
            "run_identity": {"uid": uid, "gids": sorted(gids),
                             "source": "process" if args.run_uid is None and args.run_gid is None else "args"},
        },
        "limits": limits,
    }


def write_out(path, doc):
    tmp = f"{path}.tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(doc, f, ensure_ascii=False, indent=1, sort_keys=False)
        f.write("\n")
    os.replace(tmp, path)


def main(argv):
    args = parse_args(argv)
    try:
        doc = run(args)
    except InputError as e:
        write_out(args.out, {"schema": SCHEMA, "pass": False, "error": str(e), "hits": [], "checked": {}})
        print(f"error: {e}", file=sys.stderr)
        return 2
    write_out(args.out, doc)
    print(f"pass={str(doc['pass']).lower()} hits={doc['hit_count']} needles={doc['checked']['needles']}")
    return 0 if doc["pass"] else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
