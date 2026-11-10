#!/usr/bin/env python3
"""R3（TaskStore の consumer 駆動分割）の計測と検査。標準ライブラリだけを使う。

呼び方（repo の根から。どの cwd からでも repo の根を自分で決める）:
  measure.py --json <出力path>
      計測を JSON で書く（exit 0）。key: task_store_fn_decls・task_store_supertraits（B0 と同じ
      command の値）・consumers.<crate>.{taskstore_mentions,methods,...}・unreferenced_methods・
      test_only_methods・table_writers.<表>（試験以外で INSERT/UPDATE/DELETE を書く file）。
  measure.py --check-single-decl
      TaskStore とその supertrait（推移的）の trait 定義の間で同じ fn 名が 2 回以上宣言されていれば
      その名前と trait を出して exit 1。
  measure.py --check-writers <表>,<表>
      crates/task-core/src 以外の試験でない file がその表を書けば、表と file を出して exit 1。
  measure.py --check-pruned <prune-list.json>
      list の各 fn 名が crates/ に `fn <名>` で残っていれば、名前と場所を出して exit 1。

数え方の約束（近似。ADR 2026-11-10-taskstore-consumer-ports の「指標」節）:
- 試験の file: path の成分が `tests`・`benches`・`*_tests`、file 名が `tests.rs`・`*_tests.rs`、
  または `#[cfg(test)]` の付いた `mod x;` が指す file（推移的）。試験でない file の中の
  `#[cfg(test)]` の付いた item（inline の mod・fn・impl）の行を取り除いてから数える。
- method の呼び出し: 注釈と文字列を除いたコードの `.name(`・`::name(`・`.name::<`。std の型と名前が
  重なる method（AMBIGUOUS）は受け手の識別子に `store` を含むときだけ数える。
- 表の書き手: 文字列リテラルの中の `INSERT [OR x] INTO t`・`REPLACE INTO t`・`UPDATE [OR x] t`・
  `DELETE FROM t`（大小文字を問わない）。t は migrations の `CREATE TABLE` にある表に限る。
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
CRATES = ROOT / "crates"
TASK_STORE_RS = "crates/task-core/src/store/task_store.rs"
TASK_STORE_IMPL_RS = "crates/task-core/src/store/task_store_impl.rs"
TASK_CORE_SRC = "crates/task-core/src/"

# B0（slices.json の structural_metrics）と同じ command。値はこの command の出力をそのまま使う。
B0_COMMANDS = {
    "task_store_fn_decls": f"grep -c '^    fn ' {TASK_STORE_RS}",
    "task_store_supertraits": (
        "awk '/^pub trait TaskStore:/{f=1;next} f&&/^\\{/{exit} f&&/\\+ crate::/{n++} "
        f"END{{print n}}' {TASK_STORE_RS}"
    ),
}

# 必ず consumers に出す crate（R3 の slice の consumer と task-worker）。他の crate も出す。
REQUIRED_CONSUMERS = [
    "task-ops",
    "task-dispatch",
    "task-api",
    "celeris",
    "celerisctl",
    "celeris-mcp",
    "task-worker",
]

# std の型（HashMap・Vec・Option など）の method と名前が重なるもの。受け手に store を含むときだけ数える。
AMBIGUOUS = {"get", "insert", "list", "remove", "update", "delete", "len", "clear", "set"}


# ---------------------------------------------------------------- 字句（近似）


def _skip_string(src: str, i: int) -> int:
    """src[i] が文字列の開始なら、その終わりの次の位置を返す。そうでなければ -1。"""
    n = len(src)
    j = i
    if src.startswith("b", j) and j + 1 < n and src[j + 1] in "r\"":
        j += 1
    if src.startswith("r", j) and j + 1 < n and src[j + 1] in "#\"":
        k = j + 1
        hashes = 0
        while k < n and src[k] == "#":
            hashes += 1
            k += 1
        if k < n and src[k] == '"':
            end = src.find('"' + "#" * hashes, k + 1)
            return n if end < 0 else end + 1 + hashes
        return -1
    if j < n and src[j] == '"':
        k = j + 1
        while k < n:
            c = src[k]
            if c == "\\":
                k += 2
                continue
            if c == '"':
                return k + 1
            k += 1
        return n
    return -1


def lex(src: str):
    """(code, strings) を返す。code は注釈を空白に、文字列の中身を空にしたもの（行数は保つ）。
    strings は文字列リテラルの中身の list。"""
    out = []
    strings = []
    i = 0
    n = len(src)
    while i < n:
        c = src[i]
        if c == "/" and src.startswith("//", i):
            j = src.find("\n", i)
            j = n if j < 0 else j
            i = j
            continue
        if c == "/" and src.startswith("/*", i):
            depth = 0
            j = i
            while j < n:
                if src.startswith("/*", j):
                    depth += 1
                    j += 2
                elif src.startswith("*/", j):
                    depth -= 1
                    j += 2
                    if depth == 0:
                        break
                else:
                    j += 1
            out.append("\n" * src.count("\n", i, j))
            i = j
            continue
        if c in "br\"":
            prev = src[i - 1] if i > 0 else " "
            if not (prev.isalnum() or prev == "_") or c == '"':
                end = _skip_string(src, i)
                if end > 0:
                    body = src[i:end]
                    strings.append(body)
                    out.append('""' + "\n" * body.count("\n"))
                    i = end
                    continue
        if c == "'":
            # 文字リテラル（'{'、'\n'、'\''）と lifetime（'a）を分ける。
            m = re.match(r"'(\\.[^']*|[^\\'\n])'", src[i : i + 12])
            if m:
                out.append("' '")
                i += len(m.group(0))
                continue
        out.append(c)
        i += 1
    return "".join(out), strings


def _match_brace(code: str, open_idx: int) -> int:
    depth = 0
    for k in range(open_idx, len(code)):
        ch = code[k]
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                return k
    return len(code) - 1


CFG_TEST = re.compile(r"#\[cfg\(\s*test\s*\)\]")
ATTR = re.compile(r"\s*#\[[^\]]*\]")


def cfg_test_lines(code: str) -> set[int]:
    """`#[cfg(test)]` の付いた item（inline の mod・fn・impl・use）が占める行番号（0 始まり）。"""
    lines: set[int] = set()
    for m in CFG_TEST.finditer(code):
        j = m.end()
        while True:
            a = ATTR.match(code, j)
            if not a:
                break
            j = a.end()
        semi = code.find(";", j)
        brace = code.find("{", j)
        if brace < 0 or (0 <= semi < brace):
            end = semi if semi >= 0 else len(code) - 1
        else:
            end = _match_brace(code, brace)
        first = code.count("\n", 0, m.start())
        last = code.count("\n", 0, end)
        lines.update(range(first, last + 1))
    return lines


def blank_lines(text: str, drop: set[int]) -> str:
    if not drop:
        return text
    return "\n".join("" if i in drop else line for i, line in enumerate(text.split("\n")))


# ---------------------------------------------------------------- file と試験の判定


def rust_files() -> list[Path]:
    files = []
    for dirpath, dirnames, filenames in os.walk(CRATES):
        dirnames[:] = [d for d in dirnames if d not in ("target", "node_modules", ".git")]
        for f in filenames:
            if f.endswith(".rs"):
                files.append(Path(dirpath) / f)
    return sorted(files)


def rel(p: Path) -> str:
    return p.relative_to(ROOT).as_posix()


def _path_says_test(r: str) -> bool:
    parts = r.split("/")
    for comp in parts[:-1]:
        if comp in ("tests", "benches") or comp.endswith("_tests"):
            return True
    base = parts[-1]
    return base == "tests.rs" or base.endswith("_tests.rs")


MOD_DECL = re.compile(r"((?:\s*#\[[^\]]*\])*)\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([a-z_0-9]+)\s*;")
PATH_ATTR = re.compile(r'#\[path\s*=\s*"([^"]+)"\s*\]')


def _child_module_file(parent: Path, name: str, path_attr: str | None) -> Path | None:
    is_mod_rs = parent.name in ("mod.rs", "lib.rs", "main.rs")
    if path_attr:
        cand = parent.parent / path_attr
        return cand if cand.exists() else None
    base = parent.parent if is_mod_rs else parent.parent / parent.stem
    for cand in (base / f"{name}.rs", base / name / "mod.rs"):
        if cand.exists():
            return cand
    return None


class Source:
    def __init__(self):
        self.files = rust_files()
        self.raw = {}
        self.code = {}
        self.strings = {}
        for p in self.files:
            r = rel(p)
            text = p.read_text(encoding="utf-8", errors="replace")
            self.raw[r] = text
            self.code[r], self.strings[r] = lex(text)
        self.test_files = self._test_files()
        self.prod_code = {}
        self.prod_strings = {}
        for r, c in self.code.items():
            if r in self.test_files:
                continue
            drop = cfg_test_lines(c)
            self.prod_code[r] = blank_lines(c, drop)
            self.prod_strings[r] = lex(blank_lines(self.raw[r], drop))[1] if drop else self.strings[r]

    def _test_files(self) -> set[str]:
        tests = {r for r in self.code if _path_says_test(r)}
        decl_children: dict[str, list[tuple[str, bool]]] = {}
        for r, code in self.code.items():
            # path 属性は注釈を除いた code では文字列が空になるので raw から読む。
            raw = self.raw[r]
            kids = []
            for m in MOD_DECL.finditer(raw):
                attrs = m.group(1) or ""
                pa = PATH_ATTR.search(attrs)
                child = _child_module_file(ROOT / r, m.group(2), pa.group(1) if pa else None)
                if child is None:
                    continue
                kids.append((rel(child), bool(CFG_TEST.search(attrs))))
            decl_children[r] = kids
        changed = True
        while changed:
            changed = False
            for r, kids in decl_children.items():
                for child, cfg_test in kids:
                    if child in tests:
                        continue
                    if cfg_test or r in tests:
                        tests.add(child)
                        changed = True
        return tests

    def is_test(self, r: str) -> bool:
        return r in self.test_files


def crate_of(r: str) -> str:
    return r.split("/")[1]


# ---------------------------------------------------------------- trait の解析


def trait_body(code: str, name: str):
    """`trait <name>` の (header, body, body の開始 offset) を返す。無ければ None。"""
    m = re.search(r"\btrait\s+" + re.escape(name) + r"\b", code)
    if not m:
        return None
    brace = code.find("{", m.end())
    end = _match_brace(code, brace)
    return code[m.end() : brace], code[brace + 1 : end], brace + 1


def depth1_fns(body: str) -> list[str]:
    names = []
    depth = 0
    i = 0
    for m in re.finditer(r"[{}]|\bfn\s+([a-z_0-9]+)", body):
        tok = m.group(0)
        if tok == "{":
            depth += 1
        elif tok == "}":
            depth -= 1
        elif depth == 0:
            names.append(m.group(1))
        i = m.end()
    return names


def find_trait(src: Source, name: str):
    for r in sorted(src.code):
        if not r.startswith(TASK_CORE_SRC) or src.is_test(r):
            continue
        code = src.prod_code[r]
        if re.search(r"\btrait\s+" + re.escape(name) + r"\b", code):
            tb = trait_body(code, name)
            if tb:
                return r, tb
    return None


def supertrait_names(header: str) -> list[str]:
    names = []
    for part in header.split("+"):
        part = part.strip().strip(":").strip()
        if not part or part in ("Send", "Sync") or part.startswith("'"):
            continue
        last = part.split("::")[-1].strip()
        last = re.sub(r"<.*", "", last).strip()
        if re.fullmatch(r"[A-Z][A-Za-z0-9_]*", last):
            names.append(last)
    return names


def store_traits(src: Source):
    """TaskStore と推移的な supertrait の {trait 名: {file, fns, supertraits}}。"""
    out = {}
    todo = ["TaskStore"]
    while todo:
        t = todo.pop(0)
        if t in out:
            continue
        found = find_trait(src, t)
        if not found:
            out[t] = {"file": None, "fns": [], "supertraits": []}
            continue
        r, (header, body, _) = found
        sups = supertrait_names(header.replace("\n", " "))
        out[t] = {"file": r, "fns": depth1_fns(body), "supertraits": sups}
        todo.extend(sups)
    return out


# ---------------------------------------------------------------- 呼び出しと参照


def called_methods(code: str, names: set[str]) -> set[str]:
    hits = set()
    for m in re.finditer(r"([A-Za-z_][A-Za-z_0-9]*(?:\(\))?)?\s*(\.|::)\s*([a-z_][a-z_0-9]*)\s*(\(|::<)", code):
        name = m.group(3)
        if name not in names:
            continue
        if name in AMBIGUOUS:
            recv = (m.group(1) or "").lower()
            if "store" not in recv:
                continue
        hits.add(name)
    return hits


def run_b0(cmd: str) -> int:
    out = subprocess.run(cmd, shell=True, cwd=ROOT, capture_output=True, text=True)
    try:
        return int(out.stdout.strip() or "0")
    except ValueError:
        return -1


def schema_tables() -> set[str]:
    tables = set()
    for mig in sorted(CRATES.glob("*/migrations/*.sql")):
        text = mig.read_text(encoding="utf-8", errors="replace")
        for m in re.finditer(r"CREATE\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?\"?([A-Za-z_][A-Za-z_0-9]*)", text, re.I):
            tables.add(m.group(1).lower())
    return tables


WRITE_SQL = re.compile(
    r"\b(?:INSERT(?:\s+OR\s+\w+)?\s+INTO|REPLACE\s+INTO|UPDATE(?:\s+OR\s+\w+)?|DELETE\s+FROM)\s+\"?([A-Za-z_][A-Za-z_0-9]*)",
    re.I,
)


def table_writers(src: Source) -> dict[str, list[str]]:
    known = schema_tables()
    writers: dict[str, set[str]] = {}
    for r, strings in src.prod_strings.items():
        for s in strings:
            norm = re.sub(r"\\\n\s*", " ", s)
            norm = norm.replace("\\n", " ").replace("\\t", " ")
            for m in WRITE_SQL.finditer(norm):
                t = m.group(1).lower()
                if known and t not in known:
                    continue
                writers.setdefault(t, set()).add(r)
    return {t: sorted(fs) for t, fs in sorted(writers.items())}


# ---------------------------------------------------------------- 計測


def measure() -> dict:
    src = Source()
    traits = store_traits(src)
    own = traits["TaskStore"]["fns"]
    own_set = set(own)
    all_trait_fns = {t: v["fns"] for t, v in traits.items()}
    visible = sum(len(v) for v in all_trait_fns.values())

    crates = sorted({crate_of(r) for r in src.code})
    consumers = {}
    for c in crates:
        if c == "task-core":
            continue
        mentions = 0
        mentions_tests = 0
        methods: set[str] = set()
        methods_tests: set[str] = set()
        super_methods: set[str] = set()
        named_traits: set[str] = set()
        sqlite_opens_tests = 0
        files_calling: dict[str, list[str]] = {}
        sup_names = {f for t, fs in all_trait_fns.items() if t != "TaskStore" for f in fs}
        for r in src.code:
            if crate_of(r) != c:
                continue
            if src.is_test(r):
                code = src.code[r]
                mentions_tests += len(re.findall(r"\bTaskStore\b", code))
                methods_tests |= called_methods(code, own_set)
                sqlite_opens_tests += len(re.findall(r"\bSqliteStore::open", code))
                continue
            code = src.prod_code[r]
            mentions += len(re.findall(r"\bTaskStore\b", code))
            hit = called_methods(code, own_set)
            methods |= hit
            for h in hit:
                files_calling.setdefault(h, []).append(r)
            super_methods |= called_methods(code, sup_names)
            for t in traits:
                if re.search(r"\b" + t + r"\b", code):
                    named_traits.add(t)
            # 試験 file 以外の cfg(test) の中の SqliteStore::open も fixture として数える。
            sqlite_opens_tests += len(re.findall(r"\bSqliteStore::open", src.code[r])) - len(
                re.findall(r"\bSqliteStore::open", code)
            )
        consumers[c] = {
            "taskstore_mentions": mentions,
            "taskstore_mentions_tests": mentions_tests,
            "methods": sorted(methods),
            "method_count": len(methods),
            "methods_tests_only": sorted(methods_tests - methods),
            "supertrait_methods": sorted(super_methods),
            "store_traits_named": sorted(named_traits),
            "files_calling": {k: sorted(v) for k, v in sorted(files_calling.items())},
            "sqlite_store_opens_in_tests": sqlite_opens_tests,
        }
    for c in REQUIRED_CONSUMERS:
        consumers.setdefault(c, {"taskstore_mentions": 0, "methods": [], "method_count": 0})

    # 参照: 宣言（task_store.rs）・転送（task_store_impl.rs）・`fn <名>` の定義を除いた語の出現。
    refs: dict[str, dict[str, list[str]]] = {n: {"src": [], "tests": []} for n in own}
    word = re.compile(r"\b(" + "|".join(sorted(own_set, key=len, reverse=True)) + r")\b")
    for r, code in src.code.items():
        if r in (TASK_STORE_RS, TASK_STORE_IMPL_RS):
            continue
        is_t = src.is_test(r)
        body = code if is_t else src.prod_code[r]
        found = _refs(body, word)
        if not is_t:
            # cfg(test) の中だけの参照は試験の参照として数える。
            test_only_here = _refs(code, word) - found
            for n in test_only_here:
                refs[n]["tests"].append(r)
        for n in found:
            refs[n]["tests" if is_t else "src"].append(r)
    unreferenced = sorted(n for n in own if not refs[n]["src"] and not refs[n]["tests"])
    test_only = {
        n: sorted(set(refs[n]["tests"])) for n in own if not refs[n]["src"] and refs[n]["tests"]
    }

    dup = duplicate_decls(traits)
    return {
        "schema": "taskstore-consumer-ports/measure/1",
        "generator": "agent-docs/progress/2026-11-10-taskstore-consumer-ports/measure.py",
        "head_subject": _git(["log", "-1", "--format=%s", "HEAD"]),
        "task_store_fn_decls": {
            "value": run_b0(B0_COMMANDS["task_store_fn_decls"]),
            "command": B0_COMMANDS["task_store_fn_decls"],
            "parsed_trait_fns": len(own),
        },
        "task_store_supertraits": {
            "value": run_b0(B0_COMMANDS["task_store_supertraits"]),
            "command": B0_COMMANDS["task_store_supertraits"],
            "names": traits["TaskStore"]["supertraits"],
        },
        "store_traits": {
            t: {"file": v["file"], "fn_count": len(v["fns"]), "supertraits": v["supertraits"]}
            for t, v in traits.items()
        },
        "visible_methods_via_taskstore": visible,
        "duplicate_decls": dup,
        "consumers": consumers,
        "unreferenced_methods": unreferenced,
        "test_only_methods": test_only,
        "table_writers": table_writers(src),
        "test_file_count": len(src.test_files),
        "rust_file_count": len(src.code),
    }


def _refs(text: str, word: re.Pattern) -> set[str]:
    def_spans = {m.start(1) for m in re.finditer(r"\bfn\s+([a-z_0-9]+)", text)}
    return {m.group(1) for m in word.finditer(text) if m.start(1) not in def_spans}


def duplicate_decls(traits) -> dict[str, list[str]]:
    seen: dict[str, list[str]] = {}
    for t, v in traits.items():
        for f in v["fns"]:
            seen.setdefault(f, []).append(t)
    return {f: ts for f, ts in sorted(seen.items()) if len(ts) > 1}


def _git(args: list[str]) -> str:
    out = subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True)
    return out.stdout.strip()


# ---------------------------------------------------------------- 検査


def check_single_decl() -> int:
    src = Source()
    traits = store_traits(src)
    missing = [t for t, v in traits.items() if v["file"] is None]
    dup = duplicate_decls(traits)
    for t in missing:
        print(f"trait not found: {t}", file=sys.stderr)
    for f, ts in dup.items():
        print(f"duplicate fn decl: {f} in {', '.join(ts)}")
    total = sum(len(v["fns"]) for v in traits.values())
    print(f"traits={len(traits)} fns={total} duplicates={len(dup)}")
    return 1 if dup or missing else 0


def check_writers(tables: list[str]) -> int:
    src = Source()
    writers = table_writers(src)
    bad = []
    for t in tables:
        for f in writers.get(t.lower(), []):
            if not f.startswith(TASK_CORE_SRC):
                bad.append((t, f))
    for t, f in bad:
        print(f"{t}: {f}")
    if not bad:
        print(f"ok: writers of {','.join(tables)} are only under {TASK_CORE_SRC}")
    return 1 if bad else 0


def prune_names(path: Path) -> list[str]:
    data = json.loads(path.read_text(encoding="utf-8"))
    items = data.get("fns", []) if isinstance(data, dict) else data
    names = []
    for it in items:
        names.append(it if isinstance(it, str) else it["name"])
    return names


def check_pruned(path: Path) -> int:
    names = prune_names(path)
    remaining = []
    pats = {n: re.compile(r"\bfn\s+" + re.escape(n) + r"\s*[<(]") for n in names}
    for p in rust_files():
        r = rel(p)
        code, _ = lex(p.read_text(encoding="utf-8", errors="replace"))
        lines = code.split("\n")
        for n, pat in pats.items():
            for i, line in enumerate(lines, 1):
                if pat.search(line):
                    remaining.append((n, f"{r}:{i}"))
    for n, loc in remaining:
        print(f"{n}: {loc}")
    print(f"names={len(names)} remaining={len({n for n, _ in remaining})}")
    return 1 if remaining else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--json", metavar="PATH")
    g.add_argument("--check-single-decl", action="store_true")
    g.add_argument("--check-writers", metavar="TABLES")
    g.add_argument("--check-pruned", metavar="PRUNE_LIST")
    a = ap.parse_args()
    if a.json:
        data = measure()
        out = Path(a.json)
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(json.dumps(data, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
        return 0
    if a.check_single_decl:
        return check_single_decl()
    if a.check_writers:
        return check_writers([t.strip() for t in a.check_writers.split(",") if t.strip()])
    path = Path(a.check_pruned)
    if not path.is_absolute():
        path = Path.cwd() / path
    return check_pruned(path)


if __name__ == "__main__":
    sys.exit(main())
