#!/usr/bin/env python3
"""健全性v2 R5: 試験関数 → 契約（capability）の対応表を作り、整理の候補を挙げる台本。

python3 の標準 library だけを使う。build・試験はしない。crates/ は読むだけで書かない。

出口:
  --scan                 HEAD の tree を走査し、試験・excluded・helper・組を JSON で stdout に出す
  --scaffold NAME        mapping/NAME.json の雛形を書く（既にあれば止まる。--out で出力先を変える）
  --check-fragment FILE  mapping/<name>.json か candidates/old-impl.json を検査する
  --build                mapping/*.json と candidates/old-impl.json から contracts.json を stdout に出す
  --check JSON           JSON に記録した tree から走査し直し、全 fn・invariant・hash・fragment の一致を検査する
  --self-test            内蔵 fixture で字句解析・入れ子の mod・hash・組の検出を確かめる

走査の対象は crates/task-api・crates/celeris・crates/celerisctl の .rs。HEAD（--rev で変更可）の
crate ごとの tree id を記録し、git cat-file で blob を読む（作業ツリーは読まない）。
試験の数え方は git grep -E '#\\[test\\]|#\\[tokio::test' の行と同じ。一致した行のうち試験でないもの
（コメント・文字列・macro_rules の雛形・fn 以外の item・crate root から届かない file）は excluded に理由付きで出す。
"""
from __future__ import annotations

import argparse
import bisect
import fnmatch
import hashlib
import types
import json
import os
import re
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
REL_DIR = "agent-docs/progress/2026-11-10-health-v2-r5"
SCHEMA = "celeris.health-v2-r5.contracts/1"
MAP_SCHEMA = "celeris.health-v2-r5.contracts-mapping/1"
OLD_SCHEMA = "celeris.health-v2-r5.old-impl/1"
CRATES = ("task-api", "celeris", "celerisctl")
CAPMAP_PATH = "agent-docs/progress/2026-11-07-health-v2-b0/capability-map.json"
ARCHMAP_PATH = "docs/architecture-map.md"
ARCHMAP_CHECKER = "scripts/dev/check-architecture-map.py"
GREP_RE = re.compile(r"#\[test\]|#\[tokio::test")
FRAGMENTS = [
    ("api-a", r"^crates/task-api/tests/[a-c]"),
    ("api-b", r"^crates/task-api/tests/[d-r]"),
    ("api-c", r"^crates/task-api/(tests/[s-z]|src/)"),
    ("celeris-src", r"^crates/celeris/src/"),
    ("cli", r"^crates/(celeris/tests/|celerisctl/)"),
]
ACTIONS = ("delete", "merge")
CONTRACT_MAX = 200


def die(msg: str) -> None:
    print(f"contracts.py: {msg}", file=sys.stderr)
    sys.exit(2)


# ---------------------------------------------------------------- git

def git(*args: str, input_bytes: bytes | None = None) -> bytes:
    r = subprocess.run(["git", *args], input=input_bytes, capture_output=True, cwd=repo_root())
    if r.returncode != 0:
        die(f"git {' '.join(args)}: {r.stderr.decode(errors='replace').strip()}")
    return r.stdout


_ROOT: Path | None = None


def repo_root() -> Path:
    global _ROOT
    if _ROOT is None:
        out = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, cwd=HERE, text=True)
        if out.returncode != 0:
            die("git の作業ツリーの中で実行する")
        _ROOT = Path(out.stdout.strip())
    return _ROOT


def rev_parse(spec: str) -> str:
    return git("rev-parse", spec).decode().strip()


def ls_tree(tree: str) -> list[tuple[str, str]]:
    """tree の下の blob を (相対 path, blob id) で返す。"""
    out = git("ls-tree", "-r", "-z", tree)
    res = []
    for ent in out.split(b"\0"):
        if not ent:
            continue
        meta, path = ent.split(b"\t", 1)
        _mode, kind, sha = meta.decode().split()
        if kind == "blob":
            res.append((path.decode(), sha))
    return res


def cat_blobs(shas: list[str]) -> dict[str, str]:
    if not shas:
        return {}
    uniq = sorted(set(shas))
    out = git("cat-file", "--batch", input_bytes=("\n".join(uniq) + "\n").encode())
    res, pos = {}, 0
    for sha in uniq:
        nl = out.index(b"\n", pos)
        head = out[pos:nl].decode().split()
        size = int(head[2])
        res[sha] = out[nl + 1 : nl + 1 + size].decode("utf-8", errors="replace")
        pos = nl + 1 + size + 1
    return res


def object_exists(spec: str) -> bool:
    return subprocess.run(["git", "cat-file", "-e", spec], capture_output=True, cwd=repo_root()).returncode == 0


# ---------------------------------------------------------------- lexer

RAW_STR_RE = re.compile(r'(?:b|c)?r(#*)"')
STR_RE = re.compile(r'(?:b|c)?"(?:[^"\\]|\\.)*"', re.S)
CHAR_RE = re.compile(r"b?'(?:\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F_]+\}|.)|[^\\'\n])'")
IDENT_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
NUM_RE = re.compile(r"(?:0x[0-9a-fA-F_]+|0o[0-7_]+|0b[01_]+|\d[\d_]*(?:\.\d[\d_]*)?(?:[eE][+-]?\d+)?)[A-Za-z0-9_]*")


def lex(src: str) -> list[tuple[str, int, int]]:
    """(kind, start, end) の列。kind: ws comment str char num ident lifetime punct。"""
    toks: list[tuple[str, int, int]] = []
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        prev_ident = i > 0 and (src[i - 1].isalnum() or src[i - 1] == "_")
        if c.isspace():
            j = i + 1
            while j < n and src[j].isspace():
                j += 1
            toks.append(("ws", i, j))
            i = j
            continue
        if src.startswith("//", i):
            j = src.find("\n", i)
            j = n if j < 0 else j
            toks.append(("comment", i, j))
            i = j
            continue
        if src.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if src.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif src.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            toks.append(("comment", i, j))
            i = j
            continue
        if not prev_ident and c in "bcr":
            m = RAW_STR_RE.match(src, i)
            if m:
                close = '"' + m.group(1)
                j = src.find(close, m.end())
                j = n if j < 0 else j + len(close)
                toks.append(("str", i, j))
                i = j
                continue
        if c == '"' or (not prev_ident and c in "bc" and src.startswith('"', i + 1)):
            m = STR_RE.match(src, i)
            j = m.end() if m else n
            toks.append(("str", i, j))
            i = j
            continue
        if c == "'" or (not prev_ident and c == "b" and src.startswith("'", i + 1)):
            m = CHAR_RE.match(src, i)
            if m:
                toks.append(("char", i, m.end()))
                i = m.end()
                continue
            if c == "'":
                m = IDENT_RE.match(src, i + 1)
                j = m.end() if m else i + 1
                toks.append(("lifetime", i, j))
                i = j
                continue
        if c.isdigit():
            m = NUM_RE.match(src, i)
            toks.append(("num", i, m.end()))
            i = m.end()
            continue
        if c.isalpha() or c == "_":
            m = IDENT_RE.match(src, i)
            toks.append(("ident", i, m.end()))
            i = m.end()
            continue
        if not c.isascii() and c.isalnum():  # 非 ASCII の識別子（まれ）
            j = i + 1
            while j < n and (src[j].isalnum() or src[j] == "_"):
                j += 1
            toks.append(("ident", i, j))
            i = j
            continue
        toks.append(("punct", i, i + 1))
        i += 1
    return toks


def sha(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()


def norm_text(src: str, toks, lo: int, hi: int, mask: bool) -> str:
    """toks[lo:hi] からコメント・空白を除き、mask なら literal（文字列・文字・数・bool）を伏せて並べる。"""
    out = []
    for kind, s, e in toks[lo:hi]:
        if kind in ("ws", "comment"):
            continue
        t = src[s:e]
        if mask and (kind in ("str", "char", "num") or (kind == "ident" and t in ("true", "false"))):
            t = "<lit>"
        out.append(t)
    return " ".join(out)


# ---------------------------------------------------------------- 構造の読み取り

class FileInfo:
    """1 file の字句と構造（inline mod・fn・mod 宣言・attribute）。"""

    def __init__(self, path: str, src: str):
        self.path, self.src = path, src
        self.toks = lex(src)
        self.sig = [k for k, t in enumerate(self.toks) if t[0] not in ("ws", "comment")]
        self.pos_in_sig = {k: p for p, k in enumerate(self.sig)}
        self.match = self._match_braces()
        self.mods: list[dict] = []  # inline mod: name, open, close（tok index）, cfg_test
        self.macro_spans: list[tuple[int, int]] = []
        self.impl_spans: list[tuple[int, int]] = []  # impl / trait の block
        self.mod_decls: list[dict] = []  # mod x; の宣言
        self.fns: list[dict] = []
        self._parse()

    def text(self, k: int) -> str:
        _, s, e = self.toks[k]
        return self.src[s:e]

    def kind(self, k: int) -> str:
        return self.toks[k][0]

    def _match_braces(self) -> dict[int, int]:
        match, stack = {}, []
        for k in self.sig:
            t = self.text(k)
            if self.kind(k) != "punct":
                continue
            if t in "([{":
                stack.append(k)
            elif t in ")]}" and stack:
                o = stack.pop()
                match[o], match[k] = k, o
        return match

    def sig_next(self, k: int, step: int = 1) -> int | None:
        p = self.pos_in_sig.get(k)
        if p is None:
            return None
        q = p + step
        return self.sig[q] if 0 <= q < len(self.sig) else None

    def attrs_before(self, k: int) -> tuple[list[str], int]:
        """item の先頭 token k の直前に並ぶ外側 attribute の文字列と、その先頭 token。"""
        attrs, start = [], k
        while True:
            prev = self.sig_next(start, -1)
            if prev is None or self.text(prev) != "]":
                break
            o = self.match.get(prev)
            if o is None:
                break
            h = self.sig_next(o, -1)
            if h is None or self.text(h) != "#":
                break
            s, e = self.toks[h][1], self.toks[prev][2]
            attrs.insert(0, self.src[s:e])
            start = h
        return attrs, start

    def item_start(self, k_fn: int) -> int:
        """fn / mod の key word から修飾子（pub・async・unsafe・const・extern "C"）を遡った先頭。"""
        k = k_fn
        while True:
            prev = self.sig_next(k, -1)
            if prev is None:
                return k
            t = self.text(prev)
            if t in ("pub", "async", "unsafe", "const", "default") or self.kind(prev) == "str":
                k = prev
                continue
            if t == "extern":
                k = prev
                continue
            if t == ")":  # pub(crate) / pub(super)
                o = self.match.get(prev)
                if o is not None:
                    pp = self.sig_next(o, -1)
                    if pp is not None and self.text(pp) == "pub":
                        k = pp
                        continue
            return k

    def _parse(self) -> None:
        sig = self.sig
        for p, k in enumerate(sig):
            t = self.text(k)
            if self.kind(k) != "ident":
                continue
            if t == "macro_rules" and p + 2 < len(sig) and self.text(sig[p + 1]) == "!":
                q = p + 3 if p + 3 < len(sig) and self.kind(sig[p + 2]) == "ident" else p + 2
                if q < len(sig) and sig[q] in self.match:
                    self.macro_spans.append((sig[q], self.match[sig[q]]))
            elif t in ("impl", "trait") and self._at_item_position(p):
                q = p + 1
                while q < len(sig):
                    tt = self.text(sig[q])
                    if tt in ("(", "[") and sig[q] in self.match:
                        q = self.pos_in_sig[self.match[sig[q]]] + 1
                        continue
                    if tt == "{" and sig[q] in self.match:
                        self.impl_spans.append((sig[q], self.match[sig[q]]))
                        break
                    if tt == ";":
                        break
                    q += 1
            elif t == "mod" and p + 2 < len(sig) and self.kind(sig[p + 1]) == "ident":
                name = self.text(sig[p + 1])
                nxt = sig[p + 2]
                attrs, _ = self.attrs_before(self.item_start(k))
                cfg_test = any(is_cfg_test(a) for a in attrs)
                if self.text(nxt) == "{" and nxt in self.match:
                    self.mods.append({"name": name, "open": nxt, "close": self.match[nxt], "cfg_test": cfg_test, "attrs": attrs})
                elif self.text(nxt) == ";":
                    path_attr = None
                    for a in attrs:
                        m = re.match(r'#\s*\[\s*path\s*=\s*"([^"]*)"\s*\]', a)
                        if m:
                            path_attr = m.group(1)
                    self.mod_decls.append({"name": name, "tok": k, "cfg_test": cfg_test, "path_attr": path_attr})
            elif t == "fn" and p + 1 < len(sig) and self.kind(sig[p + 1]) == "ident":
                name_k = sig[p + 1]
                start = self.item_start(k)
                attrs, attr_start = self.attrs_before(start)
                body_open = None
                q = p + 2
                while q < len(sig):
                    tt = self.text(sig[q])
                    if tt in ("(", "[") and sig[q] in self.match:
                        q = self.pos_in_sig[self.match[sig[q]]] + 1
                        continue
                    if tt == "{":
                        body_open = sig[q]
                        break
                    if tt == ";":
                        break
                    q += 1
                if body_open is None or body_open not in self.match:
                    continue
                self.fns.append({
                    "name": self.text(name_k), "fn_tok": k, "attr_start": attr_start, "attrs": attrs,
                    "open": body_open, "close": self.match[body_open],
                    "is_test": any(is_test_attr(a) for a in attrs),
                    "in_impl": any(o < k < c for o, c in self.impl_spans),
                })

    def _at_item_position(self, p: int) -> bool:
        """sig[p] が item の先頭か（型の位置の `impl Trait` を除く）。"""
        r = p - 1
        while r >= 0 and self.text(self.sig[r]) in ("unsafe", "pub", "default"):
            r -= 1
        if r >= 0 and self.text(self.sig[r]) == ")":  # pub(crate)
            o = self.match.get(self.sig[r])
            if o is not None:
                r = self.pos_in_sig[o] - 1
        return r < 0 or self.text(self.sig[r]) in ("}", ";", "{", "]")

    def use_paths(self) -> list[list[str]]:
        """use 文の path（ident の列。{a, b} は別の path に分ける。glob は末尾に '*'）。"""
        out, sig, p = [], self.sig, 0
        while p < len(sig):
            if self.text(sig[p]) == "use" and self.kind(sig[p]) == "ident":
                q = p + 1
                while q < len(sig) and self.text(sig[q]) != ";":
                    q += 1
                self._split_use([self.text(k) for k in sig[p + 1 : q]], [], out)
                p = q
            p += 1
        return out

    def _split_use(self, toks: list[str], prefix: list[str], out: list[list[str]]) -> None:
        cur, i = list(prefix), 0
        while i < len(toks):
            t = toks[i]
            if t == "{":
                depth, j = 1, i + 1
                while j < len(toks) and depth:
                    depth += toks[j] == "{"
                    depth -= toks[j] == "}"
                    j += 1
                inner, part, d = toks[i + 1 : j - 1], [], 0
                for x in inner + [","]:
                    if x == "," and d == 0:
                        self._split_use(part, cur, out)
                        part = []
                        continue
                    d += x == "{"
                    d -= x == "}"
                    part.append(x)
                return
            if t == "as":
                break
            if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*|\*", t):
                cur.append(t)
            i += 1
        if len(cur) > len(prefix) or cur:
            out.append(cur)

    def inline_mods_at(self, k: int) -> list[dict]:
        return [m for m in self.mods if m["open"] < k < m["close"]]

    def in_macro(self, k: int) -> bool:
        return any(o < k < c for o, c in self.macro_spans)

    def line_of(self, offset: int) -> int:
        return self.src.count("\n", 0, offset) + 1

    def doc_first_line(self, fn: dict) -> str:
        prev = self.sig_next(fn["attr_start"], -1)
        lo = (prev + 1) if prev is not None else 0
        for kind, s, e in self.toks[lo : fn["fn_tok"]]:
            if kind != "comment":
                continue
            t = self.src[s:e]
            if t.startswith("///") and not t.startswith("////"):
                line = t[3:].strip()
                if line:
                    return line
            elif t.startswith("/**") and not t.startswith("/***"):
                for line in t[3:-2].splitlines():
                    line = line.strip().lstrip("*").strip()
                    if line:
                        return line
        for a in fn["attrs"]:
            m = re.match(r'#\s*\[\s*doc\s*=\s*"(.*)"\s*\]$', a, re.S)
            if m and m.group(1).strip():
                return m.group(1).strip().splitlines()[0]
        return ""

    def calls(self, fn: dict) -> list[tuple[tuple[str, ...], str]]:
        """本体が呼ぶ自由関数の (修飾の path, 名)。method 呼び出し・macro・型の関連関数を除く。"""
        out, seen = [], set()
        lo, hi = self.pos_in_sig[fn["open"]], self.pos_in_sig[fn["close"]]
        sig = self.sig
        for p in range(lo + 1, hi):
            k = sig[p]
            if self.kind(k) != "ident":
                continue
            q = p + 1
            if q + 2 < hi and self.text(sig[q]) == ":" and self.text(sig[q + 1]) == ":" \
                    and self.text(sig[q + 2]) == "<":
                depth, r = 0, q + 2
                while r < hi:  # turbofish の <...> を飛ばす
                    tt = self.text(sig[r])
                    depth += tt == "<"
                    depth -= tt == ">"
                    r += 1
                    if depth == 0:
                        break
                q = r
            if q >= hi or self.text(sig[q]) != "(":
                continue
            pt = self.text(sig[p - 1])
            if pt in (".", "fn"):
                continue
            qual: list[str] = []
            r = p - 1
            while r - 2 >= lo and self.text(sig[r]) == ":" and self.text(sig[r - 1]) == ":" \
                    and self.kind(sig[r - 2]) == "ident":
                qual.insert(0, self.text(sig[r - 2]))
                r -= 3
            name = self.text(k)
            if name[:1].isupper() or name in RUST_KEYWORDS or any(x[:1].isupper() for x in qual):
                continue
            key = (tuple(qual), name)
            if key not in seen:
                seen.add(key)
                out.append(key)
        return out


RUST_KEYWORDS = {"if", "while", "match", "for", "loop", "return", "in", "as", "move", "async", "await",
                 "Some", "Ok", "Err", "None", "self", "super", "crate", "let", "mut", "ref", "where", "impl"}


def is_test_attr(a: str) -> bool:
    return bool(re.match(r"#\s*\[\s*(test\s*\]|tokio\s*::\s*test\b)", a))


def is_cfg_test(a: str) -> bool:
    m = re.match(r"#\s*\[\s*cfg\s*\((.*)\)\s*\]$", a, re.S)
    if not m:
        return False
    inner = m.group(1)
    return bool(re.search(r"(?<![A-Za-z0-9_\"])test(?![A-Za-z0-9_\"])", inner)) and "not(test" not in inner.replace(" ", "")


# ---------------------------------------------------------------- crate の module 解決

def module_dir(path: str) -> str:
    base = os.path.basename(path)
    d = os.path.dirname(path)
    if base in ("mod.rs", "lib.rs", "main.rs"):
        return d
    return os.path.join(d, base[:-3])


def resolve_crate(crate: str, files: dict[str, FileInfo]) -> dict[str, dict]:
    """crate の root から mod 宣言を辿り、file → {mod_path, test_ctx, subject} を返す。

    test_ctx: その file 全体が試験 code か（tests/ の下、または cfg(test) の mod から届く）。
    subject: cfg(test) の境界を宣言した src file（src の試験 module の持ち主）。tests/ は None。
    """
    pre = f"crates/{crate}/"
    roots = []
    for p in sorted(files):
        rel = p[len(pre):]
        if rel in ("src/lib.rs",):
            roots.append((p, "lib", False))
        elif rel == "src/main.rs":
            roots.append((p, "bin:" + crate, False))
        elif re.fullmatch(r"src/bin/[^/]+\.rs", rel):
            roots.append((p, "bin:" + rel[8:-3], False))
        elif re.fullmatch(r"src/bin/[^/]+/main\.rs", rel):
            roots.append((p, "bin:" + rel.split("/")[2], False))
        elif re.fullmatch(r"tests/[^/]+\.rs", rel):
            roots.append((p, "test:" + rel[6:-3], True))
        elif re.fullmatch(r"(examples|benches)/[^/]+\.rs", rel):
            roots.append((p, rel.split("/")[0][:-1] + ":" + rel.split("/")[1][:-3], False))
    info: dict[str, dict] = {}
    root_paths = {p for p, _, _ in roots}
    stack = [(p, mp, tc, None) for p, mp, tc in reversed(roots)]
    while stack:
        path, mod_path, test_ctx, subject = stack.pop()
        if path in info or path not in files:
            continue
        info[path] = {"mod_path": mod_path, "test_ctx": test_ctx, "subject": subject}
        fi = files[path]
        is_mod_rs = os.path.basename(path) in ("mod.rs", "lib.rs", "main.rs") or path in root_paths
        for d in reversed(fi.mod_decls):
            inl = fi.inline_mods_at(d["tok"])
            inline_names = [m["name"] for m in inl]
            inline_cfg = any(m["cfg_test"] for m in inl)
            if d["path_attr"]:
                if inline_names:
                    base = os.path.dirname(path) if is_mod_rs else module_dir(path)
                    child = os.path.normpath(os.path.join(base, *inline_names, d["path_attr"]))
                else:
                    child = os.path.normpath(os.path.join(os.path.dirname(path), d["path_attr"]))
            else:
                base = os.path.dirname(path) if is_mod_rs else module_dir(path)
                base = os.path.join(base, *inline_names)
                child = os.path.join(base, d["name"] + ".rs")
                if child not in files:
                    child = os.path.join(base, d["name"], "mod.rs")
            child_test = test_ctx or d["cfg_test"] or inline_cfg
            child_subject = subject
            if child_subject is None and child_test and not test_ctx:
                child_subject = path
            stack.append((child, "::".join([mod_path, *inline_names, d["name"]]), child_test, child_subject))
    return info


# ---------------------------------------------------------------- 走査

def load_tree_files(trees: dict[str, str]) -> dict[str, dict[str, str]]:
    """crate → {repo 相対 path: 本文}（.rs だけ）。"""
    out, want = {}, {}
    for crate, tree in trees.items():
        ents = [(f"crates/{crate}/{p}", s) for p, s in ls_tree(tree) if p.endswith(".rs")]
        want[crate] = ents
    blobs = cat_blobs([s for ents in want.values() for _, s in ents])
    for crate, ents in want.items():
        out[crate] = {p: blobs[s] for p, s in ents}
    return out


def fragment_of(path: str) -> str | None:
    for name, pat in FRAGMENTS:
        if re.search(pat, path):
            return name
    return None


def scan_sources(sources: dict[str, dict[str, str]], capmap: dict | None, archmap: str | None) -> dict:
    """crate → {path: 本文} を走査する（git に依らない。--self-test もここを通す）。"""
    tests, excluded, helpers = [], [], []
    grep_lines = 0
    per_crate_files: dict[str, dict[str, FileInfo]] = {}
    per_crate_info: dict[str, dict[str, dict]] = {}
    for crate in sorted(sources):
        files = {p: FileInfo(p, t) for p, t in sorted(sources[crate].items())}
        per_crate_files[crate] = files
        per_crate_info[crate] = resolve_crate(crate, files)

    for crate in sorted(sources):
        files, info = per_crate_files[crate], per_crate_info[crate]
        for path, fi in files.items():
            finfo = info.get(path)
            line_starts = [0] + [m.end() for m in re.finditer("\n", fi.src)]
            fn_by_attr: dict[int, dict] = {}
            for fn in fi.fns:
                for k in range(fn["attr_start"], fn["fn_tok"]):
                    fn_by_attr.setdefault(k, fn)
            tok_starts = [t[1] for t in fi.toks]
            file_uses: dict[str, list[list[str]]] = {}
            for ln, ls in enumerate(line_starts, 1):
                le = fi.src.find("\n", ls)
                le = len(fi.src) if le < 0 else le
                m = GREP_RE.search(fi.src, ls, le)
                if not m:
                    continue
                grep_lines += 1
                k = bisect.bisect_right(tok_starts, m.start()) - 1
                kind = fi.toks[k][0]
                ent = {"crate": crate, "path": path, "line": ln, "text": fi.src[ls:le].strip()[:160]}
                if kind == "comment":
                    excluded.append({**ent, "reason": "comment: コメントの中の文字列"})
                    continue
                if kind in ("str", "char"):
                    excluded.append({**ent, "reason": "string: 文字列 literal の中（fixture の本文）"})
                    continue
                if fi.in_macro(k):
                    excluded.append({**ent, "reason": "macro_rules: macro の雛形の中（展開先は走査しない）"})
                    continue
                fn = fn_by_attr.get(k)
                if fn is None or not fn["is_test"]:
                    excluded.append({**ent, "reason": "not_fn: 試験の attribute の後に fn が無い"})
                    continue
                if finfo is None:
                    excluded.append({**ent, "reason": "unreachable: crate の root から mod 宣言で届かない file（compile されない）"})
                    continue
                inl = fi.inline_mods_at(fn["fn_tok"])
                inner = "::".join(m["name"] for m in inl)
                lo, hi = fi.pos_in_sig[fn["open"]], fi.pos_in_sig[fn["close"]]
                subject = finfo["subject"]
                if subject is None and not finfo["test_ctx"] and any(m["cfg_test"] for m in inl):
                    subject = path
                tests.append({
                    "id": f"{path}::{inner + '::' if inner else ''}{fn['name']}",
                    "crate": crate,
                    "path": path,
                    "line": ln,
                    "mod_path": "::".join(x for x in [finfo["mod_path"], inner] if x),
                    "fn": fn["name"],
                    "doc": fi.doc_first_line(fn),
                    "subject": subject,
                    "body_sha256": sha(norm_text(fi.src, fi.toks, fn["open"], fn["close"] + 1, False)),
                    "masked_sha256": sha(norm_text(fi.src, fi.toks, fn["open"], fn["close"] + 1, True)),
                    "_calls": fi.calls(fn),
                    "_uses": file_uses.setdefault(path, fi.use_paths()),
                    "fragment": fragment_of(path),
                })
            # helper: 試験 code の中の試験でない fn
            for fn in fi.fns:
                if fn["is_test"] or fn["in_impl"] or finfo is None:
                    continue
                inl = fi.inline_mods_at(fn["fn_tok"])
                in_test = finfo["test_ctx"] or any(m["cfg_test"] for m in inl)
                if not in_test:
                    continue
                inner = "::".join(m["name"] for m in inl)
                body = norm_text(fi.src, fi.toks, fn["open"], fn["close"] + 1, False)
                helpers.append({
                    "id": f"{path}::{inner + '::' if inner else ''}{fn['name']}",
                    "crate": crate, "path": path, "name": fn["name"],
                    "line": fi.line_of(fi.toks[fn["fn_tok"]][1]),
                    "body_sha256": sha(body), "body_tokens": len(body.split()),
                })

    # 同じ id の重複（cfg で分けた同名など）に番号を付ける
    for coll in (tests, helpers):
        seen: dict[str, int] = defaultdict(int)
        for t in coll:
            seen[t["id"]] += 1
            if seen[t["id"]] > 1:
                t["id"] = f"{t['id']}#{seen[t['id']]}"

    resolve_setup(tests, helpers)
    test_module_files = sorted(p for c in per_crate_info.values() for p, i in c.items() if i["test_ctx"])
    b0 = b0_capabilities(tests, capmap)
    inv = invariants(tests, per_crate_info, capmap, archmap)
    for t in tests:
        t["b0_capabilities"] = b0.get(t["id"], [])
        t["invariant"] = bool(inv.get(t["id"]))
        t["invariant_sources"] = sorted(inv.get(t["id"], []))
    groups = find_groups(tests, helpers)
    tests.sort(key=lambda t: t["id"])
    excluded.sort(key=lambda e: (e["path"], e["line"]))
    return {"tests": tests, "excluded": excluded, "helpers": sorted(helpers, key=lambda h: h["id"]),
            "groups": groups, "grep_lines": grep_lines, "test_module_files": test_module_files}


def helper_segments(h: dict) -> set[str]:
    path, _, inner = h["id"].partition("::")
    rel = re.sub(r"^crates/[^/]+/(src|tests)/", "", path)[:-3]
    segs = {x for x in rel.split("/") if x not in ("mod", "lib", "main")}
    return segs | {x for x in inner.split("::")[:-1] if x}


def resolve_setup(tests: list[dict], helpers: list[dict]) -> None:
    """試験の本体が呼ぶ名を、同じ file・tests/common・tests/support・src の試験 module の helper に結ぶ。

    同じ file に定義があればそれ。無ければ crate の helper のうち、修飾の path（common::boot の common）か
    use 文の path（use crate::test_support::boot の test_support）の module 名を持つものに限る。
    tests/ の試験は tests/common・tests/support の helper だけ、src の試験は src の試験 module の helper だけを見る。
    """
    by_file: dict[str, dict[str, list[str]]] = defaultdict(lambda: defaultdict(list))
    shared: dict[tuple[str, str], list[dict]] = defaultdict(list)  # (crate, name) → helper
    for h in helpers:
        by_file[h["path"]][h["name"]].append(h["id"])
        shared[(h["crate"], h["name"])].append(h)
    for t in tests:
        setup: list[str] = []
        uses = t.pop("_uses")
        in_tests_dir = t["path"].startswith(f"crates/{t['crate']}/tests/")
        for qual, name in t.pop("_calls"):
            mods = [x for x in qual if x not in ("self", "super", "crate")]
            local = by_file[t["path"]].get(name)
            if local and not mods:
                setup.extend(x for x in local if x not in setup)
                continue
            if not mods:  # use 文から module 名を引く
                for u in uses:
                    if len(u) >= 2 and u[-1] in (name, "*"):
                        mods.append(u[-2])
                if in_tests_dir and not mods:
                    mods = ["common", "support"]
            if not mods:
                continue
            cands = shared.get((t["crate"], name), [])
            if in_tests_dir:
                cands = [h for h in cands if re.match(rf"crates/{re.escape(t['crate'])}/tests/(common|support)/", h["path"])]
            else:
                cands = [h for h in cands if h["path"].startswith(f"crates/{t['crate']}/src/")]
            cands = [h for h in cands if helper_segments(h) & set(mods)]
            if len(cands) == 1:
                ids = [cands[0]["id"]]
            elif len(cands) > 1:
                ids = ["ambiguous:" + name + ":" + ",".join(sorted(h["id"] for h in cands))]
            else:
                ids = []
            setup.extend(x for x in ids if x not in setup)
        t["setup"] = setup


_CHECKER_SRC: str | None = None  # 記録した blob の本文（scan_recorded が入れる）


def load_archmap_helpers():
    """architecture-map の検査器（scripts/dev/check-architecture-map.py）の表の読み方を使う（二重に実装しない）。

    走査では記録した blob の本文を読み、作業ツリーの版には依らない。
    """
    name = "check_architecture_map_" + (sha(_CHECKER_SRC)[:12] if _CHECKER_SRC else "worktree")
    mod = sys.modules.get(name)
    if mod is None:
        src = _CHECKER_SRC if _CHECKER_SRC is not None else (repo_root() / ARCHMAP_CHECKER).read_text()
        mod = types.ModuleType(name)
        mod.__file__ = ARCHMAP_CHECKER
        sys.modules[name] = mod
        exec(compile(src, ARCHMAP_CHECKER, "exec"), mod.__dict__)
    return mod


def path_covers(listed: str, target: str) -> bool:
    listed = listed.rstrip("/")
    if "*" in listed:
        return fnmatch.fnmatch(target, listed)
    if target == listed or target.startswith(listed + "/"):
        return True
    stem = os.path.splitext(target)[0]
    return listed.startswith(stem + "/")


def test_targets(t: dict) -> list[str]:
    """試験が「その file の試験」として数えられる path（試験の file と、試験 module の持ち主の src file）。"""
    out = [t["path"]]
    if t.get("subject"):
        out.append(t["subject"])
    return out


def b0_capabilities(tests: list[dict], capmap: dict | None) -> dict[str, list[str]]:
    res: dict[str, list[str]] = {}
    if not capmap:
        return res
    for t in tests:
        caps = []
        for cap in capmap.get("capabilities", []):
            paths = list(cap.get("tests", [])) + [o.get("path", "") for o in cap.get("owners", [])]
            if any(p and any(path_covers(p, x) for x in test_targets(t)) for p in paths):
                caps.append(cap["id"])
        res[t["id"]] = sorted(set(caps))
    return res


def archmap_test_entries(archmap: str) -> list[tuple[int, str, list[str]]]:
    """architecture-map の関連試験の列から (行番号, path, fn 名の絞り込み) を返す。"""
    mod = load_archmap_helpers()
    out = []
    for no, line in enumerate(archmap.splitlines(), 1):
        if not mod.TABLE_ROW_RE.match(line) or line.lstrip().startswith("|---"):
            continue
        cells = mod.split_cells(line)
        if cells[0] == "subsystem" or len(cells) != len(mod.COLUMNS):
            continue
        cell = cells[4]
        filters: dict[str, list[str]] = {}
        for m in mod.BACKTICK_RE.finditer(cell):
            base = m.group(1)
            if not mod.looks_like_path(base) or cell[m.end() : m.end() + 1] != "（":
                continue
            close = cell.find("）", m.end())
            body = cell[m.end() : close if close >= 0 else len(cell)]
            names = [s.group(1) for s in mod.BACKTICK_RE.finditer(body)
                     if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_*]*", s.group(1))]
            if names:
                for p in mod.expand_braces(base):
                    filters[p] = names
        for p in sorted(mod.collect_backtick_paths(cell)):
            out.append((no, p, filters.get(p, [])))
    return out


def fn_matches(name: str, filters: list[str]) -> bool:
    if not filters:
        return True
    return any(fnmatch.fnmatch(name, f) if "*" in f else name.startswith(f) for f in filters)


def invariants(tests, per_crate_info, capmap, archmap) -> dict[str, set[str]]:
    res: dict[str, set[str]] = defaultdict(set)
    if capmap:
        for cap in capmap.get("capabilities", []):
            for i, inv in enumerate(cap.get("invariants", [])):
                ev = inv.get("evidence_path") or ""
                if not ev.startswith("crates/"):
                    continue
                for t in tests:
                    # 試験 file ならその試験、src file ならその file の試験 module（持ち主 = ev）
                    if t["path"] == ev or t.get("subject") == ev:
                        res[t["id"]].add(f"b0:{cap['id']}#{i}:{ev}")
    if archmap:
        for no, p, filters in archmap_test_entries(archmap):
            if not p.startswith(tuple(f"crates/{c}/" for c in CRATES)):
                continue
            for t in tests:
                if any(path_covers(p, x) for x in test_targets(t)) and fn_matches(t["fn"], filters):
                    res[t["id"]].add(f"arch-map:L{no}:{p}")
    return res


def find_groups(tests: list[dict], helpers: list[dict]) -> list[dict]:
    groups = []
    # duplicate_setup: 2 file 以上にある helper で、コメント・空白を除いた本体の hash が同じ組
    by_hash: dict[str, list[dict]] = defaultdict(list)
    for h in helpers:
        if h["body_tokens"] > 2:  # 空の本体 {} は写しと数えない
            by_hash[h["body_sha256"]].append(h)
    users: dict[str, list[str]] = defaultdict(list)
    for t in tests:
        for s in t["setup"]:
            users[s].append(t["id"])
    for hsh, hs in by_hash.items():
        if len({h["path"] for h in hs}) < 2:
            continue
        members = sorted(h["id"] for h in hs)
        first = min(hs, key=lambda h: h["id"])
        groups.append({
            "id": "dup-" + hsh[:12], "kind": "duplicate_setup", "members": members,
            "names": sorted({h["name"] for h in hs}), "body_sha256": hsh, "body_tokens": first["body_tokens"],
            "users": sorted({u for m in members for u in users.get(m, [])}),
            "owner_fragment": fragment_of(first["path"]),
        })
    # equivalent_scenario: 同じ file か同じ capability（B0）の試験で、literal を伏せた本体の hash が同じ組
    bucket: dict[str, list[dict]] = defaultdict(list)
    for t in tests:
        if not t["invariant"]:
            bucket[t["masked_sha256"]].append(t)
    for hsh, ts in bucket.items():
        if len(ts) < 2:
            continue
        parent = {t["id"]: t["id"] for t in ts}

        def find(x):
            while parent[x] != x:
                parent[x] = parent[parent[x]]
                x = parent[x]
            return x

        for a in range(len(ts)):
            for b in range(a + 1, len(ts)):
                x, y = ts[a], ts[b]
                if x["path"] == y["path"] or set(x["b0_capabilities"]) & set(y["b0_capabilities"]):
                    parent[find(x["id"])] = find(y["id"])
        comps: dict[str, list[dict]] = defaultdict(list)
        for t in ts:
            comps[find(t["id"])].append(t)
        for comp in comps.values():
            if len(comp) < 2:
                continue
            members = sorted(t["id"] for t in comp)
            groups.append({
                "id": "eq-" + sha(members[0])[:12], "kind": "equivalent_scenario", "members": members,
                "masked_sha256": hsh,
                "basis": "same_file" if len({t["path"] for t in comp}) == 1 else "same_capability",
                "owner_fragment": fragment_of(members[0].split("::", 1)[0]),
            })
    groups.sort(key=lambda g: (g["kind"], g["id"]))
    return groups


# ---------------------------------------------------------------- 入力と記録

def read_at(commit: str, path: str) -> tuple[str, str]:
    blob = rev_parse(f"{commit}:{path}")
    return blob, cat_blobs([blob])[blob]


META_KEYS = ("commit", "crates_tree", "trees", "adr_tree", "capability_map_blob", "architecture_map_blob",
             "architecture_map_checker_blob")


def rev_meta(rev: str) -> dict:
    """走査の入力の id。commit は記録だけで、走査・検査は tree と blob の id で行う（rebase で commit が消えても読める）。"""
    commit = rev_parse(f"{rev}^{{commit}}")
    return {"commit": commit, "crates_tree": rev_parse(f"{commit}:crates"),
            "trees": {c: rev_parse(f"{commit}:crates/{c}") for c in CRATES},
            "adr_tree": rev_parse(f"{commit}:agent-docs/adr"),
            "capability_map_blob": rev_parse(f"{commit}:{CAPMAP_PATH}"),
            "architecture_map_blob": rev_parse(f"{commit}:{ARCHMAP_PATH}"),
            "architecture_map_checker_blob": rev_parse(f"{commit}:{ARCHMAP_CHECKER}")}


def scan_rev(rev: str) -> dict:
    return scan_recorded(rev_meta(rev))


def scan_recorded(meta: dict) -> dict:
    global _CHECKER_SRC
    for k in META_KEYS[1:]:
        ids = meta[k].values() if isinstance(meta[k], dict) else [meta[k]]
        if not all(object_exists(i) for i in ids):
            die(f"記録した {k} が object store に無い")
    _CHECKER_SRC = cat_blobs([meta["architecture_map_checker_blob"]])[meta["architecture_map_checker_blob"]]
    capmap = json.loads(cat_blobs([meta["capability_map_blob"]])[meta["capability_map_blob"]])
    archmap = cat_blobs([meta["architecture_map_blob"]])[meta["architecture_map_blob"]]
    res = scan_sources(load_tree_files(meta["trees"]), capmap, archmap)
    res["meta"] = dict(meta)
    res["capability_ids"] = sorted(c["id"] for c in capmap.get("capabilities", []))
    return res


def git_grep_count(meta: dict) -> int:
    total = 0
    for c in CRATES:
        r = subprocess.run(["git", "grep", "-c", "-E", r"#\[test\]|#\[tokio::test", meta["trees"][c], "--"],
                           capture_output=True, text=True, cwd=repo_root())
        for line in r.stdout.splitlines():
            total += int(line.rsplit(":", 1)[1])
    return total


def summarize(scan: dict) -> dict:
    per_crate = defaultdict(lambda: {"tests": 0, "excluded": 0, "invariant": 0})
    for t in scan["tests"]:
        per_crate[t["crate"]]["tests"] += 1
        per_crate[t["crate"]]["invariant"] += t["invariant"]
    for e in scan["excluded"]:
        per_crate[e["crate"]]["excluded"] += 1
    per_frag = defaultdict(int)
    for t in scan["tests"]:
        per_frag[t["fragment"]] += 1
    kinds = defaultdict(int)
    for g in scan["groups"]:
        kinds[g["kind"]] += 1
    reasons = defaultdict(int)
    for e in scan["excluded"]:
        reasons[e["reason"].split(":", 1)[0]] += 1
    return {"tests": len(scan["tests"]), "excluded": len(scan["excluded"]), "grep_lines": scan["grep_lines"],
            "invariant": sum(t["invariant"] for t in scan["tests"]),
            "helpers": len(scan["helpers"]), "per_crate": dict(sorted(per_crate.items())),
            "per_fragment": dict(sorted(per_frag.items())), "groups": dict(sorted(kinds.items())),
            "excluded_reasons": dict(sorted(reasons.items()))}


def cmd_scan(args) -> int:
    scan = scan_rev(args.rev)
    scan["summary"] = summarize(scan)
    scan["summary"]["git_grep_lines"] = git_grep_count(scan["meta"])
    scan["schema"] = SCHEMA.replace("contracts/", "contracts-scan/")
    json.dump(scan, sys.stdout, ensure_ascii=False, indent=1)
    print()
    s = scan["summary"]
    ok = s["tests"] + s["excluded"] == s["git_grep_lines"] == s["grep_lines"]
    print(f"tests={s['tests']} excluded={s['excluded']} git_grep_lines={s['git_grep_lines']} "
          f"match={'yes' if ok else 'NO'}", file=sys.stderr)
    return 0 if ok else 1


# ---------------------------------------------------------------- fragment

def frag_pattern(name: str) -> str:
    for n, p in FRAGMENTS:
        if n == name:
            return p
    die(f"fragment {name!r} は無い（{', '.join(n for n, _ in FRAGMENTS)}）")
    return ""


def cmd_scaffold(args) -> int:
    pat = frag_pattern(args.scaffold)
    out = Path(args.out) if args.out else WORK / "mapping" / f"{args.scaffold}.json"
    if out.exists():
        die(f"{out} は既にある（上書きしない）")
    scan = scan_rev(args.rev)
    files: dict[str, dict] = {}
    for t in scan["tests"]:
        if not re.search(pat, t["path"]):
            continue
        f = files.setdefault(t["path"], {"capability": "", "contract": "", "b0_hint": [], "tests": [], "fns": {}})
        f["tests"].append(t["id"].split("::", 1)[1])
        for c in t["b0_capabilities"]:
            if c not in f["b0_hint"]:
                f["b0_hint"].append(c)
    # users・names・basis は判断の材料（--check-fragment は見ない）
    groups = [{"id": g["id"], "kind": g["kind"], "members": g["members"],
               **{k: g[k] for k in ("names", "users", "basis") if k in g},
               "adopt": None, "reason": "", "keep_tests": [], "action": "", "plan": "", "mutation_target": ""}
              for g in scan["groups"] if g["owner_fragment"] == args.scaffold]
    doc = {"schema": MAP_SCHEMA, "fragment": args.scaffold, "scope": pat,
           "scan": scan["meta"],
           "extra_capabilities": [], "files": dict(sorted(files.items())), "groups": groups}
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(doc, ensure_ascii=False, indent=1) + "\n")
    print(f"wrote {out}: files={len(files)} tests={sum(len(f['tests']) for f in files.values())} groups={len(groups)}",
          file=sys.stderr)
    return 0


WORK = HERE  # mapping/ と candidates/ を読む場所（--dir で変える）


def mapping_files() -> list[Path]:
    return sorted((WORK / "mapping").glob("*.json"))


def load_json(path) -> dict:
    try:
        return json.loads(Path(path).read_text())
    except (OSError, ValueError) as e:
        die(f"{path}: {e}")
        return {}


def all_extra_caps(maps: list[dict]) -> dict[str, dict]:
    out = {}
    for m in maps:
        for c in m.get("extra_capabilities", []):
            out[c.get("id", "")] = c
    return out


def one_line(s) -> bool:
    return isinstance(s, str) and s.strip() != "" and "\n" not in s and len(s) <= CONTRACT_MAX


def in_crates_tree(scan: dict, path: str) -> bool:
    return path.startswith("crates/") and object_exists(f"{scan['meta']['crates_tree']}:{path[len('crates/'):]}")


def is_non_test_src(scan: dict, path: str) -> bool:
    if not re.fullmatch(r"crates/[a-z0-9-]+/src/[A-Za-z0-9_/]+\.rs", path):
        return False
    if not in_crates_tree(scan, path):
        return False
    base = os.path.basename(path)
    if base == "tests.rs" or base.endswith("_tests.rs") or "/tests/" in path:
        return False
    return path not in set(scan["test_module_files"])


def check_mapping(m: dict, scan: dict, extra: dict[str, dict]) -> list[str]:
    errs: list[str] = []
    if m.get("schema") != MAP_SCHEMA:
        errs.append(f"schema が {MAP_SCHEMA} でない")
    name = m.get("fragment", "")
    pat = dict(FRAGMENTS).get(name)
    if pat is None:
        return errs + [f"fragment {name!r} は無い"]
    known_caps = set(scan["capability_ids"]) | set(extra)
    for c in m.get("extra_capabilities", []):
        if not re.fullmatch(r"[a-z0-9-]+", c.get("id", "")) or c.get("id") in scan["capability_ids"]:
            errs.append(f"extra_capabilities: id {c.get('id')!r} が不正か B0 と重なる")
        if not one_line(c.get("name")) or not one_line(c.get("reason")):
            errs.append(f"extra_capabilities {c.get('id')}: name と reason（1 行）が要る")
        for p in c.get("owner_paths", []) or ["(無し)"]:
            if not in_crates_tree(scan, p):
                errs.append(f"extra_capabilities {c.get('id')}: owner_paths {p} が crates/ の下に実在しない")
    tests = {t["id"]: t for t in scan["tests"]}
    scope = {i: t for i, t in tests.items() if re.search(pat, t["path"])}
    files = m.get("files", {})
    for p in files:
        if not any(t["path"] == p for t in scope.values()):
            errs.append(f"files: {p} は scope の試験の file でない")
    for tid, t in sorted(scope.items()):
        f = files.get(t["path"], {})
        local = tid.split("::", 1)[1]
        o = f.get("fns", {}).get(local, {})
        cap = o.get("capability") or f.get("capability")
        con = o.get("contract") or f.get("contract")
        if not cap or cap not in known_caps:
            errs.append(f"{tid}: capability {cap!r} が無いか知らない id")
        if not one_line(con):
            errs.append(f"{tid}: 契約（1 行・{CONTRACT_MAX} 字以内）が無い")
    for p, f in files.items():
        for local in f.get("fns", {}):
            if f"{p}::{local}" not in scope:
                errs.append(f"files.{p}.fns: {local} は実在しない試験")
    owned = {g["id"]: g for g in scan["groups"] if g["owner_fragment"] == name}
    judged = {}
    for g in m.get("groups", []):
        gid = g.get("id")
        if gid not in owned:
            errs.append(f"groups: {gid} はこの fragment の組でない（走査に無い）")
            continue
        judged[gid] = g
        errs += check_judgement(g, owned[gid], scan, tests, gid)
    for gid in owned:
        if gid not in judged:
            errs.append(f"groups: {gid} の判断が無い")
    return errs


def check_judgement(g: dict, sg: dict, scan: dict, tests: dict, label: str) -> list[str]:
    errs = []
    if g.get("adopt") not in (True, False):
        errs.append(f"{label}: adopt（true/false）が無い")
    if not one_line(g.get("reason")):
        errs.append(f"{label}: reason（1 行）が無い")
    if sg is not None and sorted(g.get("members", [])) != sorted(sg["members"]):
        errs.append(f"{label}: members が走査と違う")
    if g.get("adopt") is not True:
        return errs
    keep = g.get("keep_tests", [])
    if not keep:
        errs.append(f"{label}: keep_tests が空")
    for k in keep:
        if k not in tests:
            errs.append(f"{label}: keep_tests {k} は実在しない試験")
    if g.get("action") not in ACTIONS:
        errs.append(f"{label}: action は {ACTIONS} のどれか")
    if not one_line(g.get("plan")):
        errs.append(f"{label}: plan（消すか束ねるかの案、1 行）が無い")
    mt = g.get("mutation_target", "")
    if not is_non_test_src(scan, mt):
        errs.append(f"{label}: mutation_target {mt!r} が crates/*/src/ の実在の非試験 file でない")
    if sg is not None and sg["kind"] == "equivalent_scenario":
        bad = [m for m in sg["members"] if tests.get(m, {}).get("invariant")]
        if bad:
            errs.append(f"{label}: invariant の試験が members にある: {bad}")
    return errs


def commit_subjects() -> set[str]:
    """HEAD の履歴の commit 件名（件名は rebase で変わらない）。"""
    return set(git("log", "--format=%s", "HEAD").decode(errors="replace").splitlines())


def check_old_impl(doc: dict, scan: dict) -> list[str]:
    errs = []
    if doc.get("schema") != OLD_SCHEMA:
        errs.append(f"schema が {OLD_SCHEMA} でない")
    tests = {t["id"]: t for t in scan["tests"]}
    subjects = None
    ids = set()
    for c in doc.get("candidates", []):
        label = f"old_impl {c.get('id')}"
        if not re.fullmatch(r"old-[a-z0-9-]+", c.get("id", "")) or c.get("id") in ids:
            errs.append(f"{label}: id は old-<slug> で一意")
        ids.add(c.get("id"))
        members = c.get("members", [])
        if not members:
            errs.append(f"{label}: members が空")
        for mid in members:
            if mid not in tests:
                errs.append(f"{label}: members {mid} は実在しない試験")
            elif tests[mid]["invariant"]:
                errs.append(f"{label}: invariant の試験 {mid} を members に入れない")
        ev = c.get("evidence", {})
        if ev.get("adr"):
            if not re.match(r"agent-docs/adr/.+\.md$", ev["adr"]) \
                    or not object_exists(f"{scan['meta']['adr_tree']}:{ev['adr'][len('agent-docs/adr/'):]}"):
                errs.append(f"{label}: evidence.adr {ev['adr']} が実在しない")
        elif ev.get("commit_subject"):
            if subjects is None:
                subjects = commit_subjects()
            if ev["commit_subject"] not in subjects:
                errs.append(f"{label}: evidence.commit_subject が HEAD の履歴に無い")
        else:
            errs.append(f"{label}: evidence に adr か commit_subject が要る")
        if not one_line(ev.get("what")):
            errs.append(f"{label}: evidence.what（消した仕組み・旧い API や挙動、1 行）が無い")
        if not one_line(c.get("not_replay_reason")):
            errs.append(f"{label}: not_replay_reason（DB に残る旧い shape を読む replay の契約でない理由）が無い")
        errs += check_judgement({**c, "adopt": True, "reason": c.get("not_replay_reason")}, None, scan, tests, label)
        for k in c.get("keep_tests", []):
            if k in members:
                errs.append(f"{label}: keep_tests {k} が members と重なる")
    return errs


def cmd_check_fragment(args) -> int:
    doc = load_json(args.check_fragment)
    scan = scan_recorded(meta_from_doc(doc, args.rev))
    if doc.get("schema") == OLD_SCHEMA:
        errs = check_old_impl(doc, scan)
    else:
        maps = [load_json(p) for p in mapping_files() if p.resolve() != Path(args.check_fragment).resolve()] + [doc]
        errs = check_mapping(doc, scan, all_extra_caps(maps))
    for e in errs:
        print(e)
    print(f"{args.check_fragment}: {'ok' if not errs else f'{len(errs)} 件の不備'}", file=sys.stderr)
    return 0 if not errs else 1


def meta_from_doc(doc: dict, rev: str) -> dict:
    """fragment は scaffold 時の tree と blob の id を記録する。その id で走査し直す（無ければ --rev）。"""
    rec = doc.get("scan") or {}
    if all(k in rec for k in META_KEYS):
        return {k: rec[k] for k in META_KEYS}
    return rev_meta(rev)


# ---------------------------------------------------------------- build / check

def build(scan: dict) -> tuple[dict, list[str]]:
    errs = []
    maps = [load_json(p) for p in mapping_files()]
    names = [m.get("fragment") for m in maps]
    for n, _ in FRAGMENTS:
        if n not in names:
            errs.append(f"mapping/{n}.json が無い")
    extra = all_extra_caps(maps)
    for m in maps:
        errs += [f"mapping/{m.get('fragment')}: {e}" for e in check_mapping(m, scan, extra)]
    old_path = WORK / "candidates" / "old-impl.json"
    old = load_json(old_path) if old_path.exists() else {"schema": OLD_SCHEMA, "candidates": []}
    errs += [f"candidates/old-impl.json: {e}" for e in check_old_impl(old, scan)]
    by_frag = {m.get("fragment"): m for m in maps}
    out_tests = []
    for t in scan["tests"]:
        m = by_frag.get(t["fragment"], {})
        f = m.get("files", {}).get(t["path"], {})
        o = f.get("fns", {}).get(t["id"].split("::", 1)[1], {})
        out_tests.append({k: t[k] for k in ("id", "crate", "path", "line", "mod_path", "fn", "doc", "setup",
                                            "invariant", "invariant_sources", "body_sha256", "masked_sha256",
                                            "fragment", "b0_capabilities")}
                         | {"capability": o.get("capability") or f.get("capability", ""),
                            "contract": o.get("contract") or f.get("contract", "")})
    judged = {g["id"]: g for m in maps for g in m.get("groups", [])}
    cands = {"duplicate_setup": [], "equivalent_scenario": [], "old_impl": []}
    rejected = []
    for g in scan["groups"]:
        j = judged.get(g["id"], {})
        rec = {"id": g["id"], "members": g["members"], "owner_fragment": g["owner_fragment"],
               "reason": j.get("reason", "")}
        if g["kind"] == "duplicate_setup":
            rec |= {"names": g["names"], "body_sha256": g["body_sha256"], "users": g["users"]}
        else:
            rec |= {"masked_sha256": g["masked_sha256"], "basis": g["basis"]}
        if j.get("adopt") is True:
            cands[g["kind"]].append(rec | {k: j.get(k) for k in ("keep_tests", "action", "plan", "mutation_target")})
        else:
            rejected.append(rec | {"kind": g["kind"]})
    for c in old.get("candidates", []):
        cands["old_impl"].append(c)
    caps = [{"id": i, "source": "b0"} for i in scan["capability_ids"]] + \
           [{"id": i, "source": "extra", **{k: v for k, v in c.items() if k != "id"}} for i, c in sorted(extra.items())]
    doc = {"schema": SCHEMA, "generator": f"{REL_DIR}/contracts.py", "scan": scan["meta"],
           "fragments": [{"name": n, "pattern": p} for n, p in FRAGMENTS],
           "summary": summarize(scan) | {"candidates": {k: len(v) for k, v in cands.items()},
                                         "rejected_groups": len(rejected)},
           "capabilities": caps, "tests": out_tests, "excluded": scan["excluded"],
           "candidates": cands, "rejected_groups": rejected}
    return doc, errs


def cmd_build(args) -> int:
    doc, errs = build(scan_rev(args.rev))
    for e in errs:
        print(e, file=sys.stderr)
    if errs:
        print(f"build: {len(errs)} 件の不備（contracts.json は出さない）", file=sys.stderr)
        return 1
    json.dump(doc, sys.stdout, ensure_ascii=False, indent=1)
    print()
    return 0


def cmd_check(args) -> int:
    doc = load_json(args.check)
    errs = []
    if doc.get("schema") != SCHEMA:
        errs.append(f"schema が {SCHEMA} でない")
    meta = doc.get("scan", {})
    if not all(k in meta for k in META_KEYS):
        die(f"scan に {META_KEYS} が無い")
    scan = scan_recorded({k: meta[k] for k in META_KEYS})
    grep = git_grep_count(meta)
    if len(scan["tests"]) + len(scan["excluded"]) != grep:
        errs.append(f"tests {len(scan['tests'])} + excluded {len(scan['excluded'])} が git grep の {grep} 行と違う")
    want, wanted_errs = build(scan)
    errs += wanted_errs
    got_t = {t["id"]: t for t in doc.get("tests", [])}
    want_t = {t["id"]: t for t in want["tests"]}
    for i in sorted(set(got_t) ^ set(want_t)):
        errs.append(f"tests: {i} が {'json だけ' if i in got_t else '走査だけ'}にある")
    for i in sorted(set(got_t) & set(want_t)):
        for k in want_t[i]:
            if got_t[i].get(k) != want_t[i][k]:
                errs.append(f"tests {i}: {k} が一致しない")
        if not got_t[i].get("capability") or not one_line(got_t[i].get("contract")):
            errs.append(f"tests {i}: capability か契約が無い")
    if doc.get("excluded") != want["excluded"]:
        errs.append("excluded が走査と一致しない")
    if doc.get("candidates") != want["candidates"] or doc.get("rejected_groups") != want["rejected_groups"]:
        errs.append("candidates・rejected_groups が fragment（mapping/・candidates/）と一致しない")
    inv = {i for i, t in want_t.items() if t["invariant"]}
    for kind in ("duplicate_setup", "equivalent_scenario", "old_impl"):
        cs = doc.get("candidates", {}).get(kind, [])
        if not cs:
            errs.append(f"candidates.{kind} が 0 件")
        for c in cs:
            if kind != "duplicate_setup" and set(c.get("members", [])) & inv:
                errs.append(f"candidates.{kind} {c.get('id')}: invariant の試験が members にある")
            if not c.get("keep_tests") or not c.get("mutation_target"):
                errs.append(f"candidates.{kind} {c.get('id')}: keep_tests か mutation_target が無い")
    head = rev_parse("HEAD^{commit}")
    if head != meta.get("commit"):
        print(f"note: 記録した commit {meta.get('commit', '')[:12]} は HEAD {head[:12]} と違う（記録した tree で検査した）",
              file=sys.stderr)
    for e in errs:
        print(e)
    print(f"check: tests={len(want_t)} excluded={len(want['excluded'])} git_grep={grep} "
          f"{'ok' if not errs else f'{len(errs)} 件の不備'}", file=sys.stderr)
    return 0 if not errs else 1


# ---------------------------------------------------------------- self-test

FIXTURE = {
    "demo": {
        "crates/demo/src/lib.rs": r'''
pub mod store;
pub fn prod() -> u32 { 1 }
#[cfg(test)]
mod tests {
    use super::*;
    mod nested {
        /// 入れ子の mod の試験。
        #[test]
        fn inner_case() { assert_eq!(super::super::prod(), 1); }
    }
    #[tokio::test(flavor = "multi_thread")]
    async fn raw_string_case() {
        let s = r#"fn x() { "}" } #[test] fn fake() {}"#;
        // #[test] はコメントの中
        assert!(s.contains('}'));
    }
    #[test]
    fn lit_a() { let v = fixture(); assert_eq!(v.check("alpha", 1), true); }
    #[test]
    fn lit_b() { let v = fixture(); assert_eq!(v.check("beta", 22), false); }
    fn fixture() -> Store { Store::open("mem") }
}
''',
        "crates/demo/src/store.rs": r'''
pub struct Store;
#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
''',
        "crates/demo/src/store_tests.rs": r'''
fn fixture() -> Store {
    // ここはコメントだけ違う
    Store::open("mem")
}
struct Fake;
impl Fake {
    fn fixture() -> Store { Store::open("mem") }
}
fn write() {}
#[test]
fn store_case() { let _ = fixture(); std::fs::write("p", "x").unwrap(); }
''',
        "crates/demo/src/dead.rs": "#[test]\nfn never_compiled() {}\n",
        "crates/demo/tests/api.rs": r'''
mod common;
const SRC: &str = "#[test]\nfn t() {}\n";
macro_rules! gen { ($n:ident) => { #[test] fn $n() {} }; }
#[test]
fn api_case() { common::boot(); }
''',
        "crates/demo/tests/common/mod.rs": "pub fn boot() { /* 起動 */ }\n",
    }
}


def cmd_self_test(_args) -> int:
    fails = []

    def expect(cond, msg):
        if not cond:
            fails.append(msg)

    toks = lex('let s = r#"a "quoted" }"#; let c = \'}\'; let l: &\'a str = "x\\"y"; /* /* n */ */ 1.5e3')
    kinds = [k for k, _, _ in toks if k != "ws"]
    expect(kinds.count("str") == 2 and "char" in kinds and "lifetime" in kinds and "comment" in kinds,
           f"lexer: {kinds}")
    res = scan_sources(FIXTURE, None, None)
    ids = {t["id"]: t for t in res["tests"]}
    expect("crates/demo/src/lib.rs::tests::nested::inner_case" in ids, f"入れ子の mod: {sorted(ids)}")
    t = ids.get("crates/demo/src/lib.rs::tests::nested::inner_case", {})
    expect(t.get("mod_path") == "lib::tests::nested" and t.get("doc") == "入れ子の mod の試験。", f"mod_path/doc: {t}")
    expect("crates/demo/src/lib.rs::tests::raw_string_case" in ids, "tokio::test(flavor) の試験")
    expect(not any(i.endswith("::fake") for i in ids), "raw string の中の #[test] を試験にしない")
    st = ids.get("crates/demo/src/store_tests.rs::store_case", {})
    expect(st.get("mod_path") == "lib::store::tests" and st.get("subject") == "crates/demo/src/store.rs",
           f"#[path] の試験 module: {st}")
    reasons = sorted(e["reason"].split(":")[0] for e in res["excluded"])
    expect(reasons == ["comment", "macro_rules", "string", "string", "unreachable"], f"excluded: {reasons}")
    expect(len(res["tests"]) + len(res["excluded"]) == res["grep_lines"], "tests + excluded = grep の行数")
    dup = [g for g in res["groups"] if g["kind"] == "duplicate_setup"]
    expect(len(dup) == 1 and dup[0]["members"] == ["crates/demo/src/lib.rs::tests::fixture",
                                                   "crates/demo/src/store_tests.rs::fixture"],
           f"コメントだけ違う helper の組: {dup}")
    eq = [g for g in res["groups"] if g["kind"] == "equivalent_scenario"]
    expect(len(eq) == 1 and eq[0]["members"] == ["crates/demo/src/lib.rs::tests::lit_a",
                                                 "crates/demo/src/lib.rs::tests::lit_b"],
           f"literal だけ違う試験の組: {eq}")
    expect(ids.get("crates/demo/src/store_tests.rs::store_case", {}).get("setup") == ["crates/demo/src/store_tests.rs::fixture"],
           f"impl の method と std::fs::write を setup にしない: {st.get('setup')}")
    expect(ids.get("crates/demo/tests/api.rs::api_case", {}).get("setup") == ["crates/demo/tests/common/mod.rs::boot"],
           "tests/common の helper を setup に結ぶ")
    expect(ids.get("crates/demo/src/lib.rs::tests::lit_a", {}).get("setup") == ["crates/demo/src/lib.rs::tests::fixture"],
           "同じ file の helper を setup に結ぶ")
    # invariant: src file の evidence はその file の試験 module を true にし、組から外す
    capmap = {"capabilities": [{"id": "cap-x", "owners": [], "tests": [],
                                "invariants": [{"evidence_path": "crates/demo/src/lib.rs"}]}]}
    res2 = scan_sources(FIXTURE, capmap, None)
    ids2 = {t["id"]: t for t in res2["tests"]}
    expect(ids2["crates/demo/src/lib.rs::tests::lit_a"]["invariant"]
           and not ids2["crates/demo/src/store_tests.rs::store_case"]["invariant"], "evidence_path の試験 module")
    expect(not [g for g in res2["groups"] if g["kind"] == "equivalent_scenario"], "invariant の試験は組に入れない")
    for f in fails:
        print("FAIL", f)
    print(f"self-test: {'ok' if not fails else f'{len(fails)} 件の失敗'}", file=sys.stderr)
    return 0 if not fails else 1


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--scan", action="store_true")
    g.add_argument("--scaffold", metavar="NAME")
    g.add_argument("--check-fragment", metavar="FILE")
    g.add_argument("--build", action="store_true")
    g.add_argument("--check", metavar="JSON")
    g.add_argument("--self-test", action="store_true")
    ap.add_argument("--rev", default="HEAD", help="走査する commit（既定 HEAD）")
    ap.add_argument("--out", help="--scaffold の出力先（既定 mapping/NAME.json）")
    ap.add_argument("--dir", help="mapping/ と candidates/ を読む directory（既定はこの台本の directory）")
    args = ap.parse_args(argv)
    global WORK
    if args.dir:
        WORK = Path(args.dir).resolve()
    if args.scan:
        return cmd_scan(args)
    if args.scaffold:
        return cmd_scaffold(args)
    if args.check_fragment:
        return cmd_check_fragment(args)
    if args.build:
        return cmd_build(args)
    if args.check:
        return cmd_check(args)
    return cmd_self_test(args)


if __name__ == "__main__":
    sys.exit(main())
