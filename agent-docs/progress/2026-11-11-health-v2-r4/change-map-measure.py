#!/usr/bin/env python3
"""健全性 v2 R4 change-map: Dispatcher の field の読み書き file と共変更を数え、等価性の記録を取り込む。

使い方（repo の root で）:
    python3 agent-docs/progress/2026-11-11-health-v2-r4/change-map-measure.py --out <path>

読むのは crates/task-dispatch/src/dispatcher.rs と crates/task-dispatch/src/dispatcher/ の下の *.rs
（作業ツリーの内容）、同じ dir の owners.json と equivalence/*.json、`git log`。build・試験は走らせない。
標準 library だけを使う。equivalence/*.json の形か参照（file・行・関数）が壊れていれば、出力を書かずに exit 1。
同じ HEAD（作業ツリーが HEAD と同じ）なら同じ bytes を書く。判定の規則と限界は出力の `method` に書く。
"""

import argparse
import collections
import itertools
import json
import os
import re
import subprocess
import sys

DISPATCHER_RS = "crates/task-dispatch/src/dispatcher.rs"
DISPATCHER_DIR = "crates/task-dispatch/src/dispatcher"
TESTS_DIR = DISPATCHER_DIR + "/tests"
CRATE_ROOT_DIR = "crates/task-dispatch/src"
SINCE = "2026-09-15"
GIT_SINCE = SINCE + " 00:00:00 +0000"
FIELD_AWK = (
    "awk '/^pub struct Dispatcher *\\{/{f=1;next} f&&/^\\}/{exit} "
    "f&&/^    [a-z_0-9]+:/{n++} END{print n}' crates/task-dispatch/src/dispatcher.rs"
)
GIT_LOG = [
    "git",
    "-c",
    "core.quotepath=off",
    "log",
    "--no-merges",
    "--no-renames",
    "--full-history",
    "--since=" + GIT_SINCE,
    "--name-only",
    "--format=format:%x01%H",
    "HEAD",
    "--",
    DISPATCHER_RS,
    DISPATCHER_DIR,
]
EVIDENCE_MAX = 6
EQUIV_DIR = "equivalence"
EQUIV_KEYS = {"id", "sites", "verdict", "evidence", "shared_contract"}
EQUIV_SITE_KEYS = {"at", "fn", "subject"}
EQUIV_VERDICTS = ("equivalent", "different")
EQUIV_SITES_MIN = 2
EQUIV_ID_RE = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")
EQUIV_REF_RE = re.compile(r"^([A-Za-z0-9_./-]+):([0-9]+)$")
FIELD_PAIRS_MAX = 5
TOP_PAIRS_MIN_COMMITS = 3

# 標準の型（HashMap・Vec・Option・atomic・mpsc 受け口など）で &mut self を要る、または中身を変える method。
STD_MUT_METHODS = sorted(
    {
        "append",
        "as_mut",
        "clear",
        "compare_exchange",
        "compare_exchange_weak",
        "dedup",
        "drain",
        "entry",
        "extend",
        "extend_from_slice",
        "fetch_add",
        "fetch_and",
        "fetch_or",
        "fetch_sub",
        "first_mut",
        "get_mut",
        "get_or_insert",
        "get_or_insert_with",
        "insert",
        "iter_mut",
        "last_mut",
        "pop",
        "pop_back",
        "pop_first",
        "pop_front",
        "pop_last",
        "push",
        "push_back",
        "push_front",
        "recv",
        "remove",
        "remove_entry",
        "replace",
        "resize",
        "retain",
        "retain_mut",
        "set",
        "sort",
        "sort_by",
        "sort_by_key",
        "sort_unstable",
        "split_off",
        "store",
        "swap",
        "swap_remove",
        "take",
        "truncate",
        "try_recv",
        "values_mut",
    }
)

IDENT = r"[A-Za-z_][A-Za-z0-9_]*"
RAW_STR_RE = re.compile(r'b?r(#*)"')
IMPL_DISPATCHER_RE = re.compile(
    r"\bimpl\b(?:\s*<[^{>]*>)?\s+(?:[\w:<>, ]+?\s+for\s+)?"
    r"(?:(?:super|crate|self)::)*(?:dispatcher::)?Dispatcher\s*\{"
)
IMPL_ANY_RE = re.compile(r"\bimpl\b[^{;]*\{")
PARAM_RE = re.compile(
    r"(?<![\w.:])(?:mut\s+)?(" + IDENT + r")\s*:\s*(?:&\s*(?:'\w+\s+)?(?:mut\s+)?)?"
    r"(?:(?:super|crate|self)::)*(?:dispatcher::)?Dispatcher\b"
)
LET_RE = re.compile(
    r"\blet\s+(?:\(\s*)?(?:mut\s+)?(" + IDENT + r")\b[^=;{]*?=\s*(?:&\s*(?:mut\s+)?)?"
    r"((?:" + IDENT + r"::)*" + IDENT + r")\s*(?:::\s*<[^>]*>)?\s*\("
)
MUT_SELF_FN_RE = re.compile(
    r"\bfn\s+(" + IDENT + r")\s*(?:<[^>]*>)?\s*\(\s*&\s*(?:'\w+\s+)?mut\s+self\b"
)
CFG_TEST_RE = re.compile(r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]")
INNER_CFG_TEST_RE = re.compile(r"#\s*!\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]")
MOD_DECL_RE = re.compile(r"\bmod\s+(" + IDENT + r")\s*;")
PATH_ATTR_RE = re.compile(r'#\s*\[\s*path\s*=\s*"([^"]*)"\s*\]')
COMPOUND_ASSIGN_RE = re.compile(r"(?:<<|>>|[-+*/%|&^])=")


def run(cmd):
    return subprocess.run(cmd, check=True, capture_output=True, text=True).stdout


def mask_rust(src):
    """comment と文字列・文字 literal の中身を空白にする（改行と長さは保つ）。"""
    out = list(src)
    n = len(src)

    def blank(a, b):
        for k in range(a, min(b, n)):
            if out[k] != "\n":
                out[k] = " "

    i = 0
    while i < n:
        c = src[i]
        if c == "/" and src.startswith("//", i):
            j = src.find("\n", i)
            j = n if j < 0 else j
            blank(i, j)
            i = j
            continue
        if c == "/" and src.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth > 0:
                if src.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif src.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            blank(i, j)
            i = j
            continue
        prev_ident = i > 0 and (src[i - 1].isalnum() or src[i - 1] == "_")
        if c in "rb" and not prev_ident:
            m = RAW_STR_RE.match(src, i)
            if m:
                close = '"' + m.group(1)
                j = src.find(close, m.end())
                j = n if j < 0 else j
                blank(m.end(), j)
                i = j + len(close)
                continue
            if c == "b" and i + 1 < n and src[i + 1] in "\"'":
                i += 1
                c = src[i]
        if c == '"':
            j = i + 1
            while j < n:
                if src[j] == "\\":
                    j += 2
                    continue
                if src[j] == '"':
                    break
                j += 1
            blank(i + 1, j)
            i = j + 1
            continue
        if c == "'":
            if i + 1 < n and src[i + 1] == "\\":
                j = src.find("'", i + 3)
                j = n if j < 0 else j
                blank(i + 1, j)
                i = j + 1
                continue
            if i + 2 < n and src[i + 2] == "'":
                blank(i + 1, i + 2)
                i += 3
                continue
        i += 1
    return "".join(out)


def brace_pairs(text):
    """{ } の対応（open → close）。"""
    pairs, stack = {}, []
    for i, c in enumerate(text):
        if c == "{":
            stack.append(i)
        elif c == "}" and stack:
            pairs[stack.pop()] = i
    return pairs


def skip_ws(text, i):
    n = len(text)
    while i < n and text[i].isspace():
        i += 1
    return i


def match_close(text, i):
    """text[i] が開き括弧のとき、対応する閉じ括弧の位置。"""
    opener = text[i]
    closer = {"(": ")", "[": "]", "{": "}"}[opener]
    depth = 0
    for j in range(i, len(text)):
        if text[j] == opener:
            depth += 1
        elif text[j] == closer:
            depth -= 1
            if depth == 0:
                return j
    return len(text) - 1


def item_end(text, s):
    """s から始まる item・文・field の終わり（排他的）。"""
    n = len(text)
    head = text[s : s + 200]
    field_like = re.match(r"\s*(?:pub(?:\([^)]*\))?\s+)?" + IDENT + r"\s*[:,}]", head) and not re.match(
        r"\s*(?:let|fn|impl|mod|use|struct|enum|type|const|static|if|match|for|while|loop|unsafe|async)\b",
        head,
    )
    depth, brace_top, i = 0, False, s
    while i < n:
        c = text[i]
        if c in "([{":
            depth += 1
            if c == "{" and depth == 1:
                brace_top = True
        elif c in ")]}":
            depth -= 1
            if depth < 0:
                return i
            if c == "}" and depth == 0 and brace_top:
                j = skip_ws(text, i + 1)
                if re.match(r"else\b", text[j : j + 5]):
                    i = j
                    continue
                if j < n and text[j] in ";,":
                    return j + 1
                if j < n and text[j] in ".?":
                    i = j
                    continue
                return i + 1
        elif c == ";" and depth == 0:
            return i + 1
        elif c == "," and depth == 0 and field_like:
            return i + 1
        i += 1
    return n


def field_type_end(text, s):
    """struct の field の型の終わり（`<>`・括弧の外の `,` か struct の `}`）。"""
    depth = 0
    for i in range(s, len(text)):
        c = text[i]
        if c in "(<[{":
            depth += 1
        elif c in ")]}" or (c == ">" and text[i - 1] != "-"):
            depth -= 1
            if depth < 0:
                return i
        elif c == "," and depth == 0:
            return i
    return len(text)


def cfg_test_regions(text):
    """#[cfg(test)] の付いた item・文・field の範囲。#![cfg(test)] なら file 全体。"""
    if INNER_CFG_TEST_RE.search(text):
        return [(0, len(text))]
    regions = []
    for m in CFG_TEST_RE.finditer(text):
        i = skip_ws(text, m.end())
        while text.startswith("#", i):
            j = text.find("[", i)
            i = skip_ws(text, match_close(text, j) + 1)
        regions.append((m.start(), item_end(text, i)))
    return regions


def in_regions(pos, regions):
    return any(a <= pos < b for a, b in regions)


def line_of(line_starts, pos):
    lo, hi = 0, len(line_starts) - 1
    while lo < hi:
        mid = (lo + hi + 1) // 2
        if line_starts[mid] <= pos:
            lo = mid
        else:
            hi = mid - 1
    return lo + 1


def scope_files():
    files = [DISPATCHER_RS]
    for root, dirs, names in os.walk(DISPATCHER_DIR):
        dirs.sort()
        for name in sorted(names):
            if name.endswith(".rs"):
                files.append(os.path.join(root, name).replace(os.sep, "/"))
    return sorted(files)


def mod_target(decl_file, name, path_attr):
    base = os.path.dirname(decl_file)
    if path_attr is not None:
        return [os.path.normpath(os.path.join(base, path_attr)).replace(os.sep, "/")]
    stem = os.path.splitext(os.path.basename(decl_file))[0]
    if stem not in ("mod", "lib", "main"):
        base = os.path.join(base, stem)
    return [
        os.path.normpath(os.path.join(base, name + ".rs")).replace(os.sep, "/"),
        os.path.normpath(os.path.join(base, name, "mod.rs")).replace(os.sep, "/"),
    ]


def parse_fields():
    with open(DISPATCHER_RS, encoding="utf-8") as f:
        lines = f.read().split("\n")
    fields, inside = [], False
    for idx, line in enumerate(lines):
        if not inside:
            if re.match(r"^pub struct Dispatcher *\{", line):
                inside = True
            continue
        if line.startswith("}"):
            break
        m = re.match(r"^    ([a-z_0-9]+):", line)
        if not m:
            continue
        cfg_test, k = False, idx - 1
        while k >= 0 and re.match(r"^\s*(///|//|#\[)", lines[k]):
            if CFG_TEST_RE.search(lines[k]):
                cfg_test = True
            k -= 1
        fields.append({"name": m.group(1), "line": idx + 1, "cfg_test": cfg_test})
    text = mask_rust("\n".join(lines))
    starts = [0] + [i + 1 for i, c in enumerate(text) if c == "\n"]
    for f in fields:
        s = text.index(":", starts[f["line"] - 1]) + 1
        f["type_idents"] = set(re.findall(IDENT, text[s : field_type_end(text, s)]))
    return fields


def receiver_scopes(text, pairs):
    """(receiver の名前, 範囲の開始, 範囲の終わり, 種類) の一覧。"""
    scopes = []
    for m in IMPL_DISPATCHER_RE.finditer(text):
        o = m.end() - 1
        scopes.append(("self", o, pairs.get(o, len(text)), "impl"))
    opens = sorted(pairs.items())
    for m in PARAM_RE.finditer(text):
        name = m.group(1)
        if name == "self":
            continue
        o = text.find("{", m.end())
        if o < 0:
            continue
        scopes.append((name, m.start(), pairs.get(o, len(text)), "param"))
    for m in LET_RE.finditer(text):
        name, callee = m.group(1), m.group(2).split("::")
        last = callee[-1]
        if not ("dispatcher" in last.lower() or (len(callee) >= 2 and callee[-2] == "Dispatcher")):
            continue
        end = len(text)
        for o, c in opens:
            if o > m.start():
                break
            if c > m.start():
                end = c
        scopes.append((name, m.start(), end, "let"))
    return scopes


def impl_type_name(header):
    """`impl<…> Trait for path::Type<…> where …` の Type。"""
    h = re.sub(r"^impl\s*", "", header.strip())
    if h.startswith("<"):
        depth = 0
        for i, c in enumerate(h):
            depth += {"<": 1, ">": -1}.get(c, 0)
            if depth == 0:
                h = h[i + 1 :]
                break
    h = re.split(r"\s+for\s+", h)[-1]
    m = re.match(r"\s*(?:&\s*(?:'\w+\s+)?(?:mut\s+)?)?(?:\w+::)*(\w+)", h)
    return m.group(1) if m else None


def mut_self_methods(text, test_regions):
    """Dispatcher 以外の impl で &mut self を取る method の名前（impl の型名ごと）。"""
    names = collections.defaultdict(set)
    dispatcher_impls = [(m.start(), m.end()) for m in IMPL_DISPATCHER_RE.finditer(text)]
    pairs = brace_pairs(text)
    for m in IMPL_ANY_RE.finditer(text):
        if any(a == m.start() for a, _ in dispatcher_impls) or in_regions(m.start(), test_regions):
            continue
        o = m.end() - 1
        type_name = impl_type_name(text[m.start() : o])
        if not type_name:
            continue
        body = text[o : pairs.get(o, len(text))]
        names[type_name].update(f.group(1) for f in MUT_SELF_FN_RE.finditer(body))
    return names


def classify(text, recv_start, field_end, mut_methods):
    """field の参照が書きか。"""
    before = text[max(0, recv_start - 24) : recv_start]
    if re.search(r"&\s*mut\s*\(?\s*$", before):
        return True
    n, i = len(text), field_end
    while True:
        j = skip_ws(text, i)
        if j < n and text[j] == "[":
            i = match_close(text, j) + 1
            continue
        m = re.match(r"\.\s*(" + IDENT + r"|\d+)", text[j : j + 80])
        if m:
            k = skip_ws(text, j + m.end())
            if text.startswith("(", k) or text.startswith("::", k):
                return m.group(1) in mut_methods
            i = j + m.end()
            continue
        break
    j = skip_ws(text, i)
    if j < n and text[j] == "=" and not text.startswith("==", j) and not text.startswith("=>", j):
        return True
    return bool(COMPOUND_ASSIGN_RE.match(text, j))


def collect(files, fields):
    srcs, texts, test_regions, line_starts = {}, {}, {}, {}
    for path in files:
        with open(path, encoding="utf-8") as f:
            src = f.read()
        srcs[path] = src
        texts[path] = mask_rust(src)
        line_starts[path] = [0] + [i + 1 for i, c in enumerate(src) if c == "\n"]
        test_regions[path] = cfg_test_regions(texts[path])

    # 試験の file: dispatcher/tests/ の下と、試験の範囲（#[cfg(test)] の item か試験の file）で宣言された mod。
    test_files = {p for p in files if p.startswith(TESTS_DIR + "/")}
    unread_mounts = set()
    changed = True
    while changed:
        changed = False
        for path in files:
            text = texts[path]
            for m in MOD_DECL_RE.finditer(text):
                lo = max(0, m.start() - 400)
                seg_start = max(text.rfind(c, lo, m.start()) for c in ";{}") + 1
                attrs = PATH_ATTR_RE.findall(srcs[path][max(lo, seg_start) : m.start()])
                path_attr = attrs[-1] if attrs else None
                targets = mod_target(path, m.group(1), path_attr)
                found = [t for t in targets if t in texts]
                if not found:
                    existing = [t for t in targets if os.path.isfile(t)]
                    if existing:
                        unread_mounts.add(existing[0])
                    continue
                is_test = path in test_files or in_regions(m.start(), test_regions[path])
                if is_test and found[0] not in test_files:
                    test_files.add(found[0])
                    changed = True

    custom_mut = collections.defaultdict(set)
    for path in files:
        if path in test_files:
            continue
        for type_name, names in mut_self_methods(texts[path], test_regions[path]).items():
            custom_mut[type_name] |= names
    custom_mut = {t: sorted(n) for t, n in sorted(custom_mut.items()) if n}
    field_mut = {}
    for f in fields:
        methods = set(STD_MUT_METHODS)
        for t in f["type_idents"]:
            methods.update(custom_mut.get(t, []))
        field_mut[f["name"]] = methods

    field_set = set(field_mut)
    sites = []  # (field, path, line, is_write, is_test)
    for path in files:
        text = texts[path]
        pairs = brace_pairs(text)
        seen = set()
        for name, a, b, _kind in receiver_scopes(text, pairs):
            pat = re.compile(r"(?<![\w:$#])" + re.escape(name) + r"\s*\.\s*(" + IDENT + r")\b")
            for m in pat.finditer(text, a, b):
                field = m.group(1)
                if field not in field_set or m.start(1) in seen:
                    continue
                if m.start() >= 1 and text[m.start() - 1] == "." and not text.startswith("..", m.start() - 2):
                    continue  # `x.self` のような途中の要素（`..self.f` は数える）
                k = skip_ws(text, m.end(1))
                if text.startswith("(", k) or text.startswith("::", k):
                    continue  # method 呼び出し
                seen.add(m.start(1))
                is_write = classify(text, m.start(), m.end(1), field_mut[field])
                is_test = path in test_files or in_regions(m.start(), test_regions[path])
                sites.append((field, path, line_of(line_starts[path], m.start(1)), is_write, is_test))
    return sites, sorted(test_files), sorted(unread_mounts), custom_mut


def in_dispatcher_scope(path):
    return path == DISPATCHER_RS or path.startswith(DISPATCHER_DIR + "/")


def co_change():
    out = run(GIT_LOG)
    commits = []
    for chunk in out.split("\x01")[1:]:
        lines = [l.strip() for l in chunk.split("\n")[1:] if l.strip()]
        scoped = sorted({l for l in lines if in_dispatcher_scope(l)})
        if scoped:
            commits.append(scoped)
    files = collections.Counter()
    pairs = collections.Counter()
    for scoped in commits:
        files.update(scoped)
        pairs.update(itertools.combinations(scoped, 2))
    return len(commits), files, pairs


def pair_list(pairs, keys, limit=None, min_commits=1):
    rows = [
        {"a": a, "b": b, "commits": c}
        for (a, b), c in pairs.items()
        if c >= min_commits and (keys is None or (a in keys and b in keys))
    ]
    rows.sort(key=lambda r: (-r["commits"], r["a"], r["b"]))
    return rows[:limit] if limit else rows


OWNERS = {
    "execution-coordination", "run-supervisor", "routing", "project-orchestration",
    "host-maintenance", "cluster-runtime", "composition-root", "test-hooks",
}


def assign_owners(fields, rules):
    """明示規則だけで候補を決める。競合は候補の選択で消さない。"""
    modules, overrides = rules["modules"], rules["fields"]
    for key, rule in list(modules.items()) + list(overrides.items()):
        if rule.get("owner") not in OWNERS or not rule.get("reason", "").strip():
            raise ValueError("invalid owner rule: " + key)
    unknown_fields = set(overrides) - {f["name"] for f in fields}
    if unknown_fields:
        raise ValueError("stale field rules: " + ", ".join(sorted(unknown_fields)))
    for f in fields:
        touched = sorted(set(f["readers"]) | set(f["writers"]))
        missing = set(touched) - set(modules)
        if missing:
            raise ValueError("unmapped modules for " + f["name"] + ": " + ", ".join(sorted(missing)))
        writer_owners = {modules[p]["owner"] for p in f["writers"]}
        if f["cfg_test"]:
            owner, reason = "test-hooks", "宣言に #[cfg(test)] があり、製品の field ではない。"
        elif f["name"] in overrides:
            rule = overrides[f["name"]]
            owner, reason = rule["owner"], rule["reason"]
        else:
            candidates = writer_owners or {modules[p]["owner"] for p in f["readers"]}
            if len(candidates) != 1:
                raise ValueError("unresolved owner for " + f["name"] + ": " + ", ".join(sorted(candidates)))
            owner = next(iter(candidates))
            reason = "書き手" if writer_owners else "書き手未検出のため読み手"
            reason += "の module 規則が単一 owner を示す。"
        access = []
        for p in touched:
            mode = "/".join(k for k, paths in (("read", f["readers"]), ("write", f["writers"])) if p in paths)
            access.append(p + " (" + mode + ", " + modules[p]["owner"] + ": " + modules[p]["reason"] + ")")
        reason += " 参照 module: " + ("; ".join(access) or "製品の参照なし") + "。"
        pairs = f["co_change"]
        if pairs:
            reason += " co_change（出力された上位組）: " + "; ".join(
                p["a"] + " + " + p["b"] + " = " + str(p["commits"]) + "回" for p in pairs
            ) + "。共変更は file 単位の相関であり所有の証明ではない。"
        else:
            reason += " co_change は該当組0件（未観測であり独立性の証明ではない）。"
        conflict = len(writer_owners) > 1
        if conflict:
            reason += " 書き手 owner の競合: " + "; ".join(p + " → " + modules[p]["owner"] for p in f["writers"]) + "。"
        f.update(owner=owner, owner_reason=reason, owner_conflict=conflict)


def nonempty_str(v):
    return isinstance(v, str) and bool(v.strip())


class RefFiles:
    """参照先の file の行数と、comment・文字列を空白にした本文（.rs のみ）。"""

    def __init__(self):
        self.cache = {}

    def get(self, path):
        if path not in self.cache:
            if not os.path.isfile(path):
                self.cache[path] = None
            else:
                with open(path, encoding="utf-8") as f:
                    src = f.read()
                n_lines = src.count("\n") + (1 if src and not src.endswith("\n") else 0)
                self.cache[path] = (n_lines, mask_rust(src) if path.endswith(".rs") else src)
        return self.cache[path]


def check_ref(ref, where, files, errors):
    """`path:行` の参照を検査する。file が読めれば path を返す。"""
    m = EQUIV_REF_RE.match(ref) if isinstance(ref, str) else None
    if not m:
        errors.append("%s: `path:行` の文字列でない: %r" % (where, ref))
        return None
    path, line = m.group(1), int(m.group(2))
    if os.path.normpath(path).replace(os.sep, "/") != path or path.startswith("../"):
        errors.append("%s: repo の root からの正規化した相対 path でない: %s" % (where, ref))
        return None
    entry = files.get(path)
    if entry is None:
        errors.append("%s: file が無い: %s" % (where, ref))
        return None
    if not 1 <= line <= entry[0]:
        errors.append("%s: 行 %d が file の行数 %d の外: %s" % (where, line, entry[0], ref))
    return path


def load_equivalence(eq_dir):
    """equivalence/*.json を id の順に読み、形と参照を検査する。(記録の一覧, 違反の一覧) を返す。"""
    errors, records = [], []
    names = sorted(n for n in os.listdir(eq_dir) if n.endswith(".json")) if os.path.isdir(eq_dir) else []
    if not names:
        errors.append(EQUIV_DIR + "/*.json が 0 件")
    files = RefFiles()
    for name in names:
        where = EQUIV_DIR + "/" + name
        try:
            with open(os.path.join(eq_dir, name), encoding="utf-8") as f:
                rec = json.load(f)
        except (OSError, ValueError) as error:
            errors.append("%s: JSON として読めない: %s" % (where, error))
            continue
        if not isinstance(rec, dict):
            errors.append(where + ": 最上位が object でない")
            continue
        keys = set(rec)
        if keys != EQUIV_KEYS:
            errors.append(
                "%s: key が %s でない（欠け: %s、余分: %s）"
                % (where, sorted(EQUIV_KEYS), sorted(EQUIV_KEYS - keys), sorted(keys - EQUIV_KEYS))
            )
            continue
        rid = rec["id"]
        if not (isinstance(rid, str) and EQUIV_ID_RE.match(rid)):
            errors.append("%s: id が小文字・数字・`-` の文字列でない: %r" % (where, rid))
        elif rid + ".json" != name:
            errors.append("%s: id %s が file 名と一致しない" % (where, rid))
        if rec["verdict"] not in EQUIV_VERDICTS:
            errors.append("%s: verdict が %s のどちらでもない: %r" % (where, "・".join(EQUIV_VERDICTS), rec["verdict"]))
        if not nonempty_str(rec["shared_contract"]):
            errors.append(where + ": shared_contract が空でない文字列でない")
        sites = rec["sites"]
        if not isinstance(sites, list) or len(sites) < EQUIV_SITES_MIN:
            errors.append("%s: sites が %d 件以上の配列でない" % (where, EQUIV_SITES_MIN))
            sites = sites if isinstance(sites, list) else []
        for i, site in enumerate(sites):
            sw = "%s: sites[%d]" % (where, i)
            if not isinstance(site, dict) or set(site) != EQUIV_SITE_KEYS:
                errors.append("%s: key が %s の object でない" % (sw, sorted(EQUIV_SITE_KEYS)))
                continue
            if not nonempty_str(site["subject"]):
                errors.append(sw + ": subject が空でない文字列でない")
            fn = site["fn"]
            if not (isinstance(fn, str) and re.fullmatch(IDENT, fn)):
                errors.append("%s: fn が識別子でない: %r" % (sw, fn))
                fn = None
            path = check_ref(site["at"], sw + ".at", files, errors)
            if path and fn and not re.search(r"\bfn\s+" + re.escape(fn) + r"\s*[<(]", files.get(path)[1]):
                errors.append("%s: fn %s が %s に無い" % (sw, fn, path))
        evidence = rec["evidence"]
        if not isinstance(evidence, list) or not evidence:
            errors.append(where + ": evidence が 1 件以上の配列でない")
            evidence = evidence if isinstance(evidence, list) else []
        for i, ref in enumerate(evidence):
            check_ref(ref, "%s: evidence[%d]" % (where, i), files, errors)
        records.append(rec)
    records.sort(key=lambda r: str(r["id"]))
    return records, errors


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    if not os.path.isfile(DISPATCHER_RS):
        sys.exit("repo の root で実行する（" + DISPATCHER_RS + " が無い）")

    fields = parse_fields()
    files = scope_files()
    sites, test_files, unread_mounts, custom_mut = collect(files, fields)
    n_commits, file_counts, pair_counts = co_change()

    by_field = collections.defaultdict(list)
    for s in sites:
        by_field[s[0]].append(s)

    out_fields = []
    for f in fields:
        prod = [s for s in by_field[f["name"]] if not s[4]]
        readers = sorted({s[1] for s in prod if not s[3]})
        writers = sorted({s[1] for s in prod if s[3]})
        test_access = sorted({s[1] for s in by_field[f["name"]] if s[4]})
        first = {}
        for _, path, line, is_write, _ in sorted(prod, key=lambda s: (s[1], s[2])):
            key = (path, is_write)
            first.setdefault(key, line)
        picked = []
        for path in writers:
            picked.append((path, first[(path, True)]))
        for path in readers:
            if path not in writers:
                picked.append((path, first[(path, False)]))
        evidence = sorted(
            {"%s:%d" % (p, l) for p, l in picked[:EVIDENCE_MAX]}, key=lambda e: (e.rsplit(":", 1)[0], int(e.rsplit(":", 1)[1]))
        )
        touched = set(readers) | set(writers)
        out_fields.append(
            {
                "name": f["name"],
                "decl": "%s:%d" % (DISPATCHER_RS, f["line"]),
                "cfg_test": f["cfg_test"],
                "readers": readers,
                "writers": writers,
                "test_access": test_access,
                "evidence": evidence,
                "co_change": pair_list(pair_counts, touched, limit=FIELD_PAIRS_MAX),
            }
        )
    out_fields.sort(key=lambda f: f["name"])

    rules_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "owners.json")
    with open(rules_path, encoding="utf-8") as f:
        owner_rules = json.load(f)
    try:
        assign_owners(out_fields, owner_rules)
    except ValueError as error:
        sys.exit(str(error))
    equivalence, eq_errors = load_equivalence(os.path.join(os.path.dirname(rules_path), EQUIV_DIR))
    if eq_errors:
        sys.exit("equivalence の検査で違反 %d 件:\n" % len(eq_errors) + "\n".join(eq_errors))

    doc = {
        "fields": out_fields,
        "equivalence": equivalence,
        "co_change": {
            "since": SINCE,
            "commits": n_commits,
            "files": dict(sorted(file_counts.items())),
            "pairs": pair_list(pair_counts, None, min_commits=TOP_PAIRS_MIN_COMMITS),
        },
        "method": {
            "field_list": {
                "command": FIELD_AWK,
                "count": len(fields),
                "cfg_test_count": sum(1 for f in fields if f["cfg_test"]),
                "rule": "pub struct Dispatcher の 4 字下げの `name:` 行（slices.json の dispatcher_fields と同じ数え方）。"
                "直前の attribute・comment の行に #[cfg(test)] があれば cfg_test = true。",
            },
            "scope": {
                "files_read": len(files),
                "roots": [DISPATCHER_RS, DISPATCHER_DIR + "/"],
                "test_files": test_files,
                "unread_mounted_modules": unread_mounts,
            },
            "access_rules": [
                "comment と文字列・文字 literal の中身を空白にした本文で探す（行番号は元の file と同じ）。",
                "受け手は 3 種: (1) `impl Dispatcher {`・`impl super::Dispatcher {`・`impl <trait> for Dispatcher {` の中の `self`、"
                "(2) 型が `Dispatcher`・`&Dispatcher`・`&mut Dispatcher` の引数・束縛（範囲は直後の `{` の block）、"
                "(3) 関数名の最後の部分に dispatcher を含む呼び出し（`dispatcher(…)`・`parallel_dispatcher(…)` など）"
                "か `Dispatcher::<関数>(…)` の結果を `let` で束ねた名前（tuple なら最初の名前。範囲は束縛から囲む block の終わり）。",
                "`<受け手>.<field>` の <field> が field の一覧にあり、直後が `(` か `::` でない（method 呼び出しでない）ものを参照として数える。",
                "書き: `<受け手>.<field>[.sub|[i]]* =`（`==`・`=>` は除く）、複合代入（+= -= *= /= %= |= &= ^= <<= >>=）、"
                "直前が `&mut`、または field（とその sub field の連なり）の直後の method が変更系の名前"
                "（std_mut_methods と、field の宣言の型に現れる型名の custom_mut_methods）。それ以外は読み。"
                "複合代入・変更系 method は書きだけに数える。custom_mut_methods は読む範囲の Dispatcher 以外の impl"
                "（試験の code を除く）で &mut self を取る method を impl の型名ごとに集めたもの。",
                "試験の code: dispatcher/tests/ の下の file、#[cfg(test)] の付いた item・文・field（mod なら block 全体、"
                "`#[cfg(test)] mod x;` と試験の file の中の `mod x;` の先の file も試験）、#![cfg(test)] の file。"
                "試験の code の参照は readers・writers に入れず test_access に入れる。",
                "evidence は writers の各 file の最初の書きの行、続いて readers だけの各 file の最初の読みの行を、"
                "合わせて最大 %d 件（path と行の順に並べる）。" % EVIDENCE_MAX,
            ],
            "std_mut_methods": STD_MUT_METHODS,
            "custom_mut_methods": custom_mut,
            "co_change_rules": [
                "command: " + " ".join(GIT_LOG),
                "--since は %s（UTC の 0 時）。--no-renames は git の設定（diff.renames）に結果を左右させないため。" % GIT_SINCE,
                "dispatcher.rs か dispatcher/ の下（試験の file を含む）を 1 件以上変えた merge でない commit だけを数える。"
                "pathspec と --full-history で git に絞らせる（pathspec なしの `git log --no-merges --since=… --name-only` を"
                "同じ範囲で絞った結果と同じ commit になる。2026-11-11 の確認では 5100 件中 724 件で一致）。"
                "files はその範囲の path ごとの commit 数（消えた path・rename 前の path も残る）。",
                "pairs は同じ commit で変わった範囲内の file の組と commit 数。最上位は commit 数 %d 以上の組だけを、"
                "commit 数の多い順、同数は a・b の順に並べる。" % TOP_PAIRS_MIN_COMMITS,
                "field の co_change は readers ∪ writers の file 同士の組の上位 %d 件（試験の file は含まない。閾値なし）。"
                % FIELD_PAIRS_MAX,
            ],
            "limits": [
                "型を解かない字句の判定。macro の中（`format!` の引数は拾うが、macro_rules! の展開先は見ない）、"
                "closure の引数・tuple の 2 つ目以降・struct の field に入れた Dispatcher（`self.dispatcher.f`）・"
                "`let this = &mut *self` などの別名経由の参照は拾えない。",
                "Dispatcher の分解（`let Dispatcher { a, .. } = self`）と、Dispatcher::new の struct literal での初期化は参照に数えない。",
                "field の型の自前の &mut self method は、field の宣言の型に名前が現れ、読む範囲で impl された型の分"
                "（custom_mut_methods）だけを書きとみなす。範囲外（task_worker・crate::accounts など）で定義された型の"
                "&mut self method と、sub field（`self.f.g.m()`）の型の method は読みに数える。"
                "std_mut_methods は型を見ずに当てるので、trait object の同名 method（例: `remove`）を書きと誤ることがある。",
                "Mutex・RwLock・Arc<atomic> の中身の変更は、lock() の後の変更を追わない（atomic の store・fetch_* と swap は書き）。",
                "unread_mounted_modules（#[path] で dispatcher/ の外から組み込まれる module。health_review/ など）は読まない。"
                "そこにある Dispatcher の field の参照は数に入らない。",
                "受け手の範囲は字句の block で決める。中括弧の無い closure の引数（`|d: &mut Dispatcher| d.f`）は範囲を誤る。",
                "共変更は file 単位。1 commit の中で field に関係の無い変更をした file も組に数える。",
                "co_change の --since は committer date で、rebase した commit は rebase の日時で数える。",
            ],
            "owner_rules": owner_rules,
            "equivalence_rules": [
                "同じ dir の %s/*.json を file 名の順に読み、id の順に並べて最上位の equivalence に入れる"
                "（中身は書き換えない）。" % EQUIV_DIR,
                "形: key は %s だけ。id は小文字・数字・`-` で file 名（.json を除く）と同じ。"
                "verdict は %s。shared_contract は空でない文字列。sites は %d 件以上の配列で、各要素の key は %s だけ"
                "（fn は識別子、subject は空でない文字列）。evidence は 1 件以上の配列。"
                % (sorted(EQUIV_KEYS), " か ".join(EQUIV_VERDICTS), EQUIV_SITES_MIN, sorted(EQUIV_SITE_KEYS)),
                "参照: sites の at と evidence の各要素は repo の root からの `path:行`。file が在り、"
                "行が 1 以上 file の行数以下（最後の改行の無い行も 1 行と数える）。sites の fn は、"
                "comment・文字列を空白にした at の file の本文に `fn <名前>` の宣言がある。",
                "違反が 1 件でもあれば全件を stderr に出し、出力 file を書かずに exit 1。",
                "検査は参照の実在までで、verdict と shared_contract の内容の正しさ、fn が at の行を含むことは検査しない。",
            ],
        },
    }
    out_dir = os.path.dirname(os.path.abspath(args.out))
    os.makedirs(out_dir, exist_ok=True)
    with open(args.out, "w", encoding="utf-8") as f:
        json.dump(doc, f, ensure_ascii=False, indent=1, sort_keys=True)
        f.write("\n")


if __name__ == "__main__":
    main()
