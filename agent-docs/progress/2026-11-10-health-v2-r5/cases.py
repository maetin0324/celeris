#!/usr/bin/env python3
"""健全性v2 R5 guard: base と作業ツリーの試験関数・表の case を名前で数えて比べる。

使い方:
    python3 agent-docs/progress/2026-11-10-health-v2-r5/cases.py --crate <crate> --base <rev> [--json]

- `<crate>` は `crates/<dir>` の dir 名か Cargo の package 名（例 `celeris-campaign` → `crates/campaign`）。
- base は `git ls-tree` / `git cat-file` で `<rev>` から読み、作業ツリーは `git ls-files -co --exclude-standard`
  の file を disk から読む（未 commit・未追跡の変更も数える）。
- 試験関数は `#[test]`・`#[tokio::test(...)]`・`#[<path>::test]`・`#[rstest]`・`#[test_case(...)]` の付いた fn。
  `macro_rules!` の中の `fn $name` は、規則が 1 つで matcher が `$x:frag` の並びのときに呼び出しの引数から名前を引く。
- 表の case の行は、試験関数の本体で `for <pat> in [...]`（`&[...]`・`vec![...]`、または同じ本体の
  `let <x> = [...]` を回す `for <pat> in <x>`）の配列の要素数と、`#[case(...)]`・`#[test_case(...)]` の数。
- 試験の同一性は crate 内の関数名（多重集合）。file・module path は見ないので、束ねで module path が変わるだけの
  移動は消えたと数えない。

非 0 になる条件（exit 1）:
1. 作業ツリーで減った関数（名前ごとの数の減り）・表の行（名前ごとの合計の減り）が
   `agent-docs/progress/2026-11-10-health-v2-r5/removals/<dir>.json` に載っていない。
2. removals の項目が不正（理由が無い・代わりの試験が作業ツリーに無い・mutation の結果が killed でない・
   patch が無い・filter が無い）。
3. B0 の `capability-map.json` の capabilities[].tests と invariants[].evidence_path（と、その file が
   `#[cfg(test)] mod x;` で読む子 file）にある base の試験関数が、作業ツリーで減っている（removals に載せても不可）。
usage・git・入力の誤りは exit 2。

removals/<dir>.json の形:
    {"crate": "<dir>", "removals": [
      {"name": "<消えた関数名>", "count": 1,            # 省略時 1（同名の関数が複数あるとき減った数）
       "cases": 2,                                     # 書けば「関数ではなく表の行を 2 行減らした」
       "reason": "<契約の廃止 / 代わりが同じ不具合を検出する理由>",
       "replacement": ["<作業ツリーに在る試験関数名>", "<other-crate>:<関数名>"],
       "mutation": {"patch": "<repo 相対の patch>", "filter": "<nextest filter>", "result": "killed"}}]}
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from collections import Counter
from dataclasses import dataclass, field
from pathlib import Path

CAPABILITY_MAP = "agent-docs/progress/2026-11-07-health-v2-b0/capability-map.json"
REMOVALS_DIR = "agent-docs/progress/2026-11-10-health-v2-r5/removals"
SCHEMA = "celeris.health-v2-r5.cases/1"


class UsageError(Exception):
    pass


# ---------------------------------------------------------------------------
# 字句: comment と文字列の中身を空白に置き換える（長さと改行は保つ。offset が原文と一致する）
# ---------------------------------------------------------------------------

_LEX = re.compile(r"""//|/\*|(?<![A-Za-z0-9_])b?r#*"|(?<![A-Za-z0-9_])b'|"|'""")
_CHAR = re.compile(r"""'(?:\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F]{1,6}\}|.)|[^\\'\n])'""")


def _blank(s: str) -> str:
    return "".join("\n" if c == "\n" else " " for c in s)


def strip_rust(text: str) -> str:
    out: list[str] = []
    pos = 0
    n = len(text)
    while pos < n:
        m = _LEX.search(text, pos)
        if m is None:
            out.append(text[pos:])
            break
        start = m.start()
        out.append(text[pos:start])
        tok = m.group(0)
        if tok == "//":
            end = text.find("\n", start)
            end = n if end < 0 else end
            out.append(_blank(text[start:end]))
            pos = end
        elif tok == "/*":
            depth = 0
            i = start
            while i < n:
                if text.startswith("/*", i):
                    depth += 1
                    i += 2
                elif text.startswith("*/", i):
                    depth -= 1
                    i += 2
                    if depth == 0:
                        break
                else:
                    i += 1
            out.append(_blank(text[start:i]))
            pos = i
        elif tok.endswith('"') and "r" in tok:
            close = '"' + "#" * tok.count("#")
            cstart = m.end()
            end = text.find(close, cstart)
            if end < 0:
                out.append(tok + _blank(text[cstart:]))
                pos = n
            else:
                out.append(tok + _blank(text[cstart:end]) + close)
                pos = end + len(close)
        elif tok.endswith('"'):
            i = m.end()
            while i < n and text[i] != '"':
                i += 2 if text[i] == "\\" else 1
            end = min(i + 1, n)
            out.append(tok + _blank(text[m.end() : end - 1]) + text[end - 1 : end])
            pos = end
        else:  # ' または b'
            cstart = start + (1 if tok == "b'" else 0)
            cm = _CHAR.match(text, cstart)
            if cm is not None:
                out.append(text[start:cstart] + "'" + _blank(text[cstart + 1 : cm.end() - 1]) + "'")
                pos = cm.end()
            else:  # lifetime・label
                out.append(text[start : m.end()])
                pos = m.end()
    res = "".join(out)
    if len(res) != len(text):  # 念のため（offset の一致が前提）
        raise RuntimeError("lexer length mismatch")
    return res


# ---------------------------------------------------------------------------
# 構文の断片
# ---------------------------------------------------------------------------

_OPEN = {"(": ")", "[": "]", "{": "}"}
_CLOSE = {")", "]", "}"}


def match_bracket(s: str, i: int) -> int:
    """s[i] が開き括弧のとき、対応する閉じ括弧の次の index を返す（見つからなければ len(s)）。"""
    depth = 0
    n = len(s)
    j = i
    while j < n:
        c = s[j]
        if c in _OPEN:
            depth += 1
        elif c in _CLOSE:
            depth -= 1
            if depth == 0:
                return j + 1
        j += 1
    return n


def split_top(s: str, seps: str = ",") -> list[str]:
    """括弧の深さ 0 の区切りで分ける（空の要素は捨てる）。"""
    parts: list[str] = []
    depth = 0
    cur: list[str] = []
    for c in s:
        if c in _OPEN:
            depth += 1
        elif c in _CLOSE:
            depth -= 1
        if depth == 0 and c in seps:
            parts.append("".join(cur))
            cur = []
        else:
            cur.append(c)
    parts.append("".join(cur))
    return [p.strip() for p in parts if p.strip()]


_ATTR_START = re.compile(r"#\s*(!?)\s*\[")
_TEST_ATTR = re.compile(r"\s*(?:[A-Za-z_]\w*\s*::\s*)*test\s*(?:\(|$)|\s*rstest\s*(?:\(|$)")
_CASE_ATTR = re.compile(r"\s*(?:test_case|case)\s*(?:::\s*[A-Za-z_]\w*\s*)?(?:\(|$)")
_FN = re.compile(
    r"\s*(?:pub\s*(?:\([^)]*\)\s*)?)?(?:default\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?"
    r"(?:extern\s+(?:\"[^\"]*\"\s*)?)?fn\s+(\$?[A-Za-z_]\w*)"
)
_MACRO_DEF = re.compile(r"\bmacro_rules\s*!\s*([A-Za-z_]\w*)\s*([\(\[\{])")
_FOR = re.compile(r"\bfor\s+")
_LET = re.compile(r"\blet\s+(?:mut\s+)?([A-Za-z_]\w*)\s*(?::[^=;]*)?=\s*")
_CFG_TEST_MOD = re.compile(r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]")
_MOD_DECL = re.compile(r"\s*(?:pub\s*(?:\([^)]*\)\s*)?)?mod\s+([A-Za-z_]\w*)\s*;")
_PATH_ATTR = re.compile(r'#\s*\[\s*path\s*=\s*"([^"]*)"\s*\]')


@dataclass
class TestFn:
    name: str
    path: str
    line: int
    rows: int = 0


@dataclass
class MacroDef:
    params: list[str] | None  # None: 名前を引数から引けない形
    test_params: list[str]  # 試験 fn の名前になる param（出現順）
    literal_tests: int = 0


_ARRAY = re.compile(r"\s*&?\s*(?:mut\s+)?(?:vec\s*!\s*)?\[")
_IDENT_AT = re.compile(r"\s*&?\s*([A-Za-z_]\w*)\b")
_ATTR_AT = re.compile(r"\s*#\s*\[")


def array_rows(s: str, i: int) -> int | None:
    """s[i:] が `[..]`・`&[..]`・`vec![..]`・`&vec![..]` で始まるなら要素数を返す（`[expr; N]` は 0）。"""
    m = _ARRAY.match(s, i)
    if m is None:
        return None
    b = m.end() - 1
    inner = s[b + 1 : match_bracket(s, b) - 1]
    if len(split_top(inner, ";")) > 1:
        return 0
    return len(split_top(inner, ","))


def table_rows(body: str) -> int:
    lets: dict[str, int] = {}
    for m in _LET.finditer(body):
        r = array_rows(body, m.end())
        if r is not None:
            lets[m.group(1)] = r
    total = 0
    n = len(body)
    for m in _FOR.finditer(body):
        # pattern（括弧を含みうる）の後の深さ 0 の ` in ` を探す
        j = m.end()
        depth = 0
        k = None
        while j < n:
            c = body[j]
            if depth == 0 and c.isspace() and body.startswith("in", j + 1) and (j + 3 >= n or not (body[j + 3].isalnum() or body[j + 3] == "_")):
                k = j + 3
                break
            if c in _OPEN:
                depth += 1
            elif c in _CLOSE:
                depth -= 1
            if depth < 0 or c == ";":
                break
            j += 1
        if k is None:
            continue
        r = array_rows(body, k)
        if r is not None:
            total += r
            continue
        im = _IDENT_AT.match(body, k)
        if im and im.group(1) in lets:
            total += lets[im.group(1)]
    return total


def scan_test_attrs(clean: str, lo: int = 0, hi: int | None = None):
    """[lo, hi) の中の試験 fn を (fn の名前, fn の名前の offset, body の (start, end), case 属性の数) で返す。"""
    hi = len(clean) if hi is None else hi
    found: dict[int, list] = {}
    pos = lo
    while True:
        m = _ATTR_START.search(clean, pos, hi)
        if m is None:
            break
        b = m.end() - 1
        e = match_bracket(clean, b)
        pos = max(e, m.end())
        if m.group(1) == "!":
            continue
        content = clean[b + 1 : e - 1]
        is_test = _TEST_ATTR.match(content) is not None
        is_case = _CASE_ATTR.match(content) is not None
        if not (is_test or is_case):
            continue
        # 後ろの属性を飛ばして fn を探す
        j = e
        while True:
            am = _ATTR_AT.match(clean, j)
            if am is None:
                break
            j = match_bracket(clean, am.end() - 1)
        fm = _FN.match(clean, j)
        if fm is None or fm.start(1) >= hi:
            continue
        key = fm.start(1)
        rec = found.setdefault(key, [fm.group(1), key, None, 0])
        if is_case or content.lstrip().startswith("test_case"):
            rec[3] += 1
        if rec[2] is None:
            p = clean.find("(", fm.end())
            if p >= 0:
                q = match_bracket(clean, p)
                ob = clean.find("{", q)
                semi = clean.find(";", q)
                if ob >= 0 and (semi < 0 or ob < semi):
                    rec[2] = (ob, match_bracket(clean, ob))
    return [tuple(v) for _, v in sorted(found.items())]


def parse_macro_defs(clean: str) -> tuple[dict[str, MacroDef], list[tuple[int, int]]]:
    defs: dict[str, MacroDef] = {}
    spans: list[tuple[int, int]] = []
    for m in _MACRO_DEF.finditer(clean):
        b = m.end() - 1
        e = match_bracket(clean, b)
        spans.append((m.start(), e))
        body = clean[b + 1 : e - 1]
        # 規則: (matcher) => {transcriber} ;
        rules = []
        i = 0
        while i < len(body):
            while i < len(body) and body[i] in " \t\r\n;":
                i += 1
            if i >= len(body) or body[i] not in _OPEN:
                break
            me = match_bracket(body, i)
            arrow = body.find("=>", me)
            if arrow < 0:
                break
            ts = arrow + 2
            while ts < len(body) and body[ts].isspace():
                ts += 1
            if ts >= len(body) or body[ts] not in _OPEN:
                break
            te = match_bracket(body, ts)
            rules.append((body[i + 1 : me - 1], body[ts + 1 : te - 1]))
            i = te
        tests: list[str] = []
        literal = 0
        for _r_idx, (_matcher, trans) in enumerate(rules):
            for name, _off, _body, _cases in scan_test_attrs(trans):
                if name.startswith("$"):
                    tests.append(name[1:])
                else:
                    literal += 1
        params = None
        if len(rules) == 1:
            matcher = rules[0][0]
            items = split_top(matcher, ",;")
            ps = []
            ok = True
            for it in items:
                pm = re.fullmatch(r"\$([A-Za-z_]\w*)\s*:\s*[a-z_]+", it.strip())
                if pm is None:
                    ok = False
                    break
                ps.append(pm.group(1))
            params = ps if ok else None
        if tests or literal:
            defs[m.group(1)] = MacroDef(params=params, test_params=tests, literal_tests=literal)
    return defs, spans


def line_of(text: str, off: int) -> int:
    return text.count("\n", 0, off) + 1


@dataclass
class FileScan:
    tests: list[TestFn] = field(default_factory=list)
    clean: str = ""


def scan_file(path: str, text: str) -> tuple[FileScan, dict[str, MacroDef], list[tuple[int, int]]]:
    clean = strip_rust(text)
    defs, spans = parse_macro_defs(clean)
    fs = FileScan(clean=clean)

    def in_macro(off: int) -> bool:
        return any(a <= off < b for a, b in spans)

    for name, off, body, cases in scan_test_attrs(clean):
        if name.startswith("$") or in_macro(off):
            if not name.startswith("$"):
                fs.tests.append(TestFn(name=name, path=path, line=line_of(clean, off)))
            continue
        rows = cases
        if body is not None:
            rows += table_rows(clean[body[0] : body[1]])
        fs.tests.append(TestFn(name=name, path=path, line=line_of(clean, off), rows=rows))
    return fs, defs, spans


def expand_macros(scans: dict[str, FileScan], spans: dict[str, list], defs: dict[str, MacroDef], warnings: list[str]):
    if not defs:
        return
    pat = re.compile(r"\b(" + "|".join(re.escape(k) for k in sorted(defs)) + r")\s*!\s*([\(\[\{])")
    for path, fs in scans.items():
        clean = fs.clean
        for m in pat.finditer(clean):
            if clean[max(0, m.start() - 20) : m.start()].rstrip().endswith("macro_rules!"):
                continue
            if any(a <= m.start() < b for a, b in spans.get(path, [])):
                continue
            d = defs[m.group(1)]
            b = m.end() - 1
            e = match_bracket(clean, b)
            args = split_top(clean[b + 1 : e - 1], ",;")
            ln = line_of(clean, m.start())
            if d.params is not None and len(d.params) == len(args):
                for tp in d.test_params:
                    if tp in d.params:
                        nm = args[d.params.index(tp)].strip()
                        if re.fullmatch(r"[A-Za-z_]\w*", nm):
                            fs.tests.append(TestFn(name=nm, path=path, line=ln))
                            continue
                    fs.tests.append(TestFn(name=f"{m.group(1)}!({' '.join(args)})#{tp}", path=path, line=ln))
                    warnings.append(f"{path}:{ln}: macro {m.group(1)}! の試験名を引数から引けない（{tp}）")
            else:
                for k, tp in enumerate(d.test_params):
                    fs.tests.append(TestFn(name=f"{m.group(1)}!({' '.join(args)})#{k}", path=path, line=ln))
                if d.test_params:
                    warnings.append(f"{path}:{ln}: macro {m.group(1)}! の規則が単純な形でない。呼び出しの引数全体を名前にした")


# ---------------------------------------------------------------------------
# 木の読み込み
# ---------------------------------------------------------------------------


def git(repo: Path, *args: str, check: bool = True) -> str:
    p = subprocess.run(["git", "-C", str(repo), *args], capture_output=True, text=True)
    if check and p.returncode != 0:
        raise UsageError(f"git {' '.join(args)}: {p.stderr.strip()}")
    return p.stdout


def read_base(repo: Path, rev: str, prefix: str) -> dict[str, str]:
    names = [n for n in git(repo, "ls-tree", "-r", "-z", "--name-only", rev, "--", prefix).split("\0") if n.endswith(".rs")]
    if not names:
        return {}
    req = "".join(f"{rev}:{n}\n" for n in names).encode()
    p = subprocess.run(["git", "-C", str(repo), "cat-file", "--batch"], input=req, capture_output=True)
    if p.returncode != 0:
        raise UsageError(f"git cat-file: {p.stderr.decode(errors='replace').strip()}")
    out = p.stdout
    files: dict[str, str] = {}
    i = 0
    for n in names:
        nl = out.index(b"\n", i)
        header = out[i:nl].split()
        if len(header) < 3 or header[1] != b"blob":
            i = nl + 1
            continue
        size = int(header[2])
        files[n] = out[nl + 1 : nl + 1 + size].decode("utf-8", errors="replace")
        i = nl + 1 + size + 1
    return files


def read_worktree(repo: Path, prefix: str) -> dict[str, str]:
    names = git(repo, "ls-files", "-z", "-co", "--exclude-standard", "--", prefix).split("\0")
    files: dict[str, str] = {}
    for n in sorted(set(names)):
        if not n.endswith(".rs"):
            continue
        p = repo / n
        if p.is_file():
            files[n] = p.read_text(encoding="utf-8", errors="replace")
    return files


@dataclass
class Tree:
    tests: list[TestFn]
    scans: dict[str, FileScan]
    warnings: list[str]

    def counts(self) -> Counter:
        return Counter(t.name for t in self.tests)

    def rows(self) -> Counter:
        c: Counter = Counter()
        for t in self.tests:
            c[t.name] += t.rows
        return c

    def names_in(self, path: str) -> Counter:
        return Counter(t.name for t in self.tests if t.path == path)


def analyze(files: dict[str, str]) -> Tree:
    scans: dict[str, FileScan] = {}
    spans: dict[str, list] = {}
    defs: dict[str, MacroDef] = {}
    for path, text in files.items():
        fs, d, sp = scan_file(path, text)
        scans[path] = fs
        spans[path] = sp
        defs.update(d)
    warnings: list[str] = []
    expand_macros(scans, spans, defs, warnings)
    tests = [t for fs in scans.values() for t in fs.tests]
    return Tree(tests=tests, scans=scans, warnings=warnings)


def resolve_crate(repo: Path, rev: str, crate: str) -> str:
    """`crates/<dir>` の dir 名を返す。dir 名でなければ package 名として引く。"""
    if (repo / "crates" / crate).is_dir() or git(repo, "ls-tree", "--name-only", rev, f"crates/{crate}", check=False).strip():
        return crate
    for toml in sorted((repo / "crates").glob("*/Cargo.toml")):
        m = re.search(r'^\s*name\s*=\s*"([^"]+)"', toml.read_text(encoding="utf-8"), re.M)
        if m and m.group(1) == crate:
            return toml.parent.name
    raise UsageError(f"crate が見つからない: {crate}（crates/<dir> の dir 名か package 名）")


def child_test_mods(path: str, clean: str, raw: str) -> list[str]:
    """`#[cfg(test)] [#[path = ".."]] mod x;` の子 file の候補 path（repo 相対）。"""
    out: list[str] = []
    p = Path(path)
    stem_dir = p.parent if p.name in ("mod.rs", "lib.rs", "main.rs") else p.parent / p.stem
    for m in _CFG_TEST_MOD.finditer(clean):
        j = m.end()
        explicit = None
        while True:
            am = _ATTR_AT.match(clean, j)
            if am is None:
                break
            e = match_bracket(clean, am.end() - 1)
            pm = _PATH_ATTR.search(raw[am.start() : e])
            if pm:
                explicit = pm.group(1)
            j = e
        mm = _MOD_DECL.match(clean, j)
        if mm is None:
            continue
        if explicit is not None:
            out.append(os.path.normpath(str(p.parent / explicit)))
        else:
            out.append(str(stem_dir / f"{mm.group(1)}.rs"))
            out.append(str(stem_dir / mm.group(1) / "mod.rs"))
    return out


def protected_paths(capmap: dict, prefix: str) -> list[str]:
    paths: set[str] = set()
    for cap in capmap.get("capabilities", []):
        for t in cap.get("tests", []):
            if isinstance(t, str) and t.startswith(prefix):
                paths.add(t)
        for inv in cap.get("invariants", []):
            ev = inv.get("evidence_path")
            if isinstance(ev, str) and ev.startswith(prefix):
                paths.add(ev)
    return sorted(paths)


# ---------------------------------------------------------------------------
# 照合
# ---------------------------------------------------------------------------


def load_removals(path: Path) -> list[dict]:
    if not path.exists():
        return []
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as e:
        raise UsageError(f"{path}: JSON として読めない: {e}") from e
    items = data.get("removals") if isinstance(data, dict) else None
    if not isinstance(items, list):
        raise UsageError(f"{path}: removals の配列が無い")
    return items


def check(repo: Path, crate: str, base: str, capmap_path: Path, removals_dir: Path) -> dict:
    git(repo, "rev-parse", "--verify", f"{base}^{{commit}}")
    dirname = resolve_crate(repo, base, crate)
    prefix = f"crates/{dirname}/"
    base_files = read_base(repo, base, prefix)
    wt_files = read_worktree(repo, prefix)
    if not base_files and not wt_files:
        raise UsageError(f"{prefix} に .rs が無い（base {base} と作業ツリーの両方）")
    bt = analyze(base_files)
    wt = analyze(wt_files)
    bc, wc = bt.counts(), wt.counts()
    br, wr = bt.rows(), wt.rows()

    removed_fns = {n: bc[n] - wc[n] for n in bc if bc[n] > wc[n]}
    removed_rows = {n: br[n] - wr[n] for n in br if br[n] > wr[n] and wc[n] > 0}

    errors: list[str] = []
    notes: list[str] = []

    # 不変条件の試験（B0 capability-map）
    if not capmap_path.exists():
        raise UsageError(f"capability-map が無い: {capmap_path}")
    capmap = json.loads(capmap_path.read_text(encoding="utf-8"))
    ppaths = protected_paths(capmap, prefix)
    expanded: list[str] = []
    for pp in ppaths:
        if pp not in base_files:
            notes.append(f"capability-map の {pp} は base に無い（照合しない）")
            continue
        expanded.append(pp)
        for child in child_test_mods(pp, bt.scans[pp].clean, base_files[pp]):
            if child in base_files and child not in expanded:
                expanded.append(child)
    protected: Counter = Counter()
    protected_from: dict[str, str] = {}
    for pp in expanded:
        for n, k in bt.names_in(pp).items():
            protected[n] += k
            protected_from.setdefault(n, pp)
    protected_lost = []
    for n in sorted(protected):
        if wc[n] < bc[n]:
            protected_lost.append({"name": n, "path": protected_from[n], "base": bc[n], "worktree": wc[n]})
            errors.append(f"不変条件の試験が減った: {n}（{protected_from[n]}、base {bc[n]} → 作業ツリー {wc[n]}）")

    # removals
    rpath = removals_dir / f"{dirname}.json"
    entries = load_removals(rpath)
    covered_fns: Counter = Counter()
    covered_rows: Counter = Counter()
    other_trees: dict[str, Counter] = {}
    entry_errors: list[str] = []
    for idx, ent in enumerate(entries):
        where = f"{rpath.relative_to(repo) if rpath.is_relative_to(repo) else rpath}[{idx}]"
        if not isinstance(ent, dict) or not isinstance(ent.get("name"), str) or not ent["name"]:
            entry_errors.append(f"{where}: name が無い")
            continue
        name = ent["name"]
        bad = []
        if not isinstance(ent.get("reason"), str) or not ent["reason"].strip():
            bad.append("reason が無い")
        repl = ent.get("replacement")
        if not isinstance(repl, list) or not repl:
            bad.append("replacement（代わりの試験）が無い")
        else:
            for r in repl:
                if not isinstance(r, str) or not r:
                    bad.append(f"replacement の要素が文字列でない: {r!r}")
                    continue
                if ":" in r and not r.startswith(":"):
                    oc, on = r.split(":", 1)
                    try:
                        od = resolve_crate(repo, base, oc)
                    except UsageError:
                        bad.append(f"replacement の crate が無い: {r}")
                        continue
                    if od not in other_trees:
                        other_trees[od] = wc if od == dirname else analyze(read_worktree(repo, f"crates/{od}/")).counts()
                    have = other_trees[od][on.split("::")[-1]]
                else:
                    have = wc[r.split("::")[-1]]
                if have == 0:
                    bad.append(f"replacement が作業ツリーに無い: {r}")
        mut = ent.get("mutation")
        if not isinstance(mut, dict):
            bad.append("mutation が無い")
        else:
            if mut.get("result") != "killed":
                bad.append(f"mutation の結果が killed でない: {mut.get('result')!r}")
            pt = mut.get("patch")
            if not isinstance(pt, str) or not pt or not (repo / pt).is_file():
                bad.append(f"mutation の patch が作業ツリーに無い: {pt!r}")
            if not isinstance(mut.get("filter"), str) or not mut["filter"].strip():
                bad.append("mutation の filter が無い")
        if name in protected and "cases" not in ent:
            bad.append("不変条件の試験は removals で消せない")
        if bad:
            entry_errors.append(f"{where} {name}: " + "、".join(bad))
            continue
        if "cases" in ent:
            k = ent["cases"]
            if not isinstance(k, int) or k <= 0:
                entry_errors.append(f"{where} {name}: cases は正の整数")
                continue
            covered_rows[name] += k
        else:
            k = ent.get("count", 1)
            if not isinstance(k, int) or k <= 0:
                entry_errors.append(f"{where} {name}: count は正の整数")
                continue
            covered_fns[name] += k
    errors.extend(entry_errors)

    def locate(tree: Tree, name: str) -> str:
        for t in tree.tests:
            if t.name == name:
                return f"{t.path}:{t.line}"
        return "?"

    unlisted_fns = []
    for n, k in sorted(removed_fns.items()):
        if covered_fns[n] < k:
            unlisted_fns.append({"name": n, "removed": k, "listed": covered_fns[n], "base_at": locate(bt, n)})
            if n not in protected:
                errors.append(f"消えた試験関数が removals に無い: {n}（{k} 個減、載っているのは {covered_fns[n]}、base {locate(bt, n)}）")
    unlisted_rows = []
    for n, k in sorted(removed_rows.items()):
        if covered_rows[n] < k:
            unlisted_rows.append({"name": n, "removed": k, "listed": covered_rows[n], "base_at": locate(bt, n)})
            errors.append(f"表の case の行が減ったのに removals に無い: {n}（{br[n]} → {wr[n]}、載っているのは {covered_rows[n]}）")
    for n, k in sorted(covered_fns.items()):
        if removed_fns.get(n, 0) < k:
            notes.append(f"removals の {n} は作業ツリーで減っていない（関数 {removed_fns.get(n, 0)} 個減、載せたのは {k}）")
    for n, k in sorted(covered_rows.items()):
        if removed_rows.get(n, 0) < k:
            notes.append(f"removals の {n} の表は {removed_rows.get(n, 0)} 行しか減っていない（載せたのは {k}）")

    bpaths: dict[str, Counter] = {}
    wpaths: dict[str, Counter] = {}
    for t in bt.tests:
        bpaths.setdefault(t.name, Counter())[t.path] += 1
    for t in wt.tests:
        wpaths.setdefault(t.name, Counter())[t.path] += 1
    moved = sum(1 for n in bc if wc[n] >= bc[n] and bpaths[n] != wpaths.get(n))

    return {
        "schema": SCHEMA,
        "crate": dirname,
        "base": base,
        "base_functions": sum(bc.values()),
        "worktree_functions": sum(wc.values()),
        "base_case_rows": sum(br.values()),
        "worktree_case_rows": sum(wr.values()),
        "moved_by_name": moved,
        "removed_functions": removed_fns,
        "removed_case_rows": removed_rows,
        "unlisted_functions": unlisted_fns,
        "unlisted_case_rows": unlisted_rows,
        "protected_paths": expanded,
        "protected_functions": sum(protected.values()),
        "protected_lost": protected_lost,
        "removals_file": str(rpath.relative_to(repo)) if rpath.is_relative_to(repo) else str(rpath),
        "removal_entries": len(entries),
        "errors": errors,
        "notes": notes + bt.warnings + [w for w in wt.warnings if w not in bt.warnings],
        "ok": not errors,
    }


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--crate", required=True)
    ap.add_argument("--base", required=True)
    ap.add_argument("--json", action="store_true", help="結果を JSON で出す")
    ap.add_argument("--repo", help="repo の root（既定は cwd の git top-level）")
    ap.add_argument("--capability-map", help=f"既定 <repo>/{CAPABILITY_MAP}")
    ap.add_argument("--removals-dir", help=f"既定 <repo>/{REMOVALS_DIR}")
    a = ap.parse_args(argv)
    try:
        if a.repo:
            repo = Path(a.repo).resolve()
        else:
            p = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True)
            if p.returncode != 0:
                raise UsageError("git の作業ツリーの中で実行する（または --repo）")
            repo = Path(p.stdout.strip())
        capmap = Path(a.capability_map) if a.capability_map else repo / CAPABILITY_MAP
        rdir = Path(a.removals_dir) if a.removals_dir else repo / REMOVALS_DIR
        res = check(repo, a.crate, a.base, capmap.resolve(), rdir.resolve())
    except UsageError as e:
        print(f"cases.py: {e}", file=sys.stderr)
        return 2
    if a.json:
        print(json.dumps(res, ensure_ascii=False, indent=1, sort_keys=True))
    else:
        print(
            f"crate {res['crate']}: 試験関数 base {res['base_functions']} → 作業ツリー {res['worktree_functions']}、"
            f"表の行 base {res['base_case_rows']} → {res['worktree_case_rows']}、名前で追った移動 {res['moved_by_name']}、"
            f"不変条件の試験 {res['protected_functions']}（{len(res['protected_paths'])} file）、removals {res['removal_entries']} 件"
        )
        for n in res["notes"]:
            print(f"note: {n}")
        for e in res["errors"]:
            print(f"FAIL: {e}")
        print("OK" if res["ok"] else "NG")
    return 0 if res["ok"] else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
