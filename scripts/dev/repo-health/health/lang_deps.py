"""言語 adapter の依存の解析: import・use・Cargo の依存を path と行つきの辺にする（deps.* が使う）。

方法は健全性 v2 B0 の module-deps-measure.py（agent-docs/progress/2026-11-07-health-v2-b0/）を汎用化した。
B0 との違い: crate 名を固定せず tree の Cargo.toml から package を見つける、`cargo metadata` を使わない
（base の tree にも同じ方法を当てるため。外部 tool も要らない）、Python・TypeScript の import も読む。

単位（unit。循環を数える節点）:
- Rust: file の論理 module（crate からの module 名の列）の第 1 要素（`<pkg>/src/<第 1 要素>`。ADR 2026-11-17
  repo-health-scc-edges-logical-modules D2）。論理 module は crate root（src/lib.rs・src/main.rs）から file の
  top level（column 0。inline の `mod` block と `#[cfg(test)]` の範囲の外）の `mod x;` をたどって決める:
  `#[path = "P"] mod x;` は宣言した file の dir からの相対の P を読み、宣言した file の module の子 x にする。
  `#[path]` の無い `mod x;` は rustc と同じく `x.rs`・`x/mod.rs`（crate root・mod.rs・`#[path]` で読んだ file は
  同じ dir、それ以外の `foo.rs` は `foo/` の下）。同じ file を 2 つの宣言が読めば宣言した file の path の辞書順で
  先のものを採る。どの宣言からも届かない file は path から決める（`<pkg>/src/store.rs` と `<pkg>/src/store/**`
  は `<pkg>/src/store`）。表は `rust_modules` が tree ごとに 1 回作り、deps.* と size.responsibility-growth が
  `rust_module_of` で同じ表を引く。
  crate root（src/lib.rs・src/main.rs・src/bin/**）は単位にしない。`crate::X` の X が module でなければ
  root の再 export とみなして辺を作らない（root を通る見かけの循環を数えない）。
  `super::` で始まる path は論理 module（inline の `mod` block も数える）から `super` の数だけ上がり、
  crate root に着けば次の要素 X を `crate::X` と同じに解決する（root に着かなければ自分の単位、上がりすぎは辺にしない）。
  他の package への `use <lib>::X` は `<他 pkg>/src/X`（module でなければ `<他 pkg>`）への辺。
  Cargo.toml の依存は package dir どうしの辺（kind cargo、dev-dependencies は kind cargo-dev）。
- Python: file 1 つが 1 単位。ast の import・from import を相対・tree 内の path に解決する（解決できない
  import は外部とみなして捨てる）。
- TypeScript・JavaScript: file 1 つが 1 単位。`./`・`../` の相対指定だけを解決する（package 名は外部）。
  `import type`・`export type` は type_only（実行時の循環に数えない）。

試験・fixture・生成の file（adapters の種類が product 以外）と Rust の `#[cfg(test)]` の範囲は辺の元にしない。
静的な文字列の解析なので、macro が作る path・条件付きの import・動的な import は拾わない。
"""

from __future__ import annotations

import ast
import functools
import posixpath
import re
import tomllib
from dataclasses import dataclass

from . import source_size

RUST_ROOT_FILES = ("lib.rs", "main.rs")
TS_EXTS = (".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs")


@dataclass(frozen=True)
class Edge:
    src: str  # 辺の元の file（repo の相対 path）
    line: int
    dst: str  # 先の file（Cargo の辺は相手の Cargo.toml）
    src_unit: str
    dst_unit: str
    kind: str  # cargo | cargo-dev | use | import
    excerpt: str
    type_only: bool = False


@dataclass
class Graph:
    edges: list[Edge]
    packages: dict[str, str]  # Rust package 名 -> dir
    modules: dict[str, tuple[str, tuple[str, ...] | None]]  # Rust file -> (package dir, 論理 module の path)


# ------------------------------------------------------------------ 共通

def _line_of(text: str, offset: int) -> int:
    return text.count("\n", 0, offset) + 1


def _excerpt(lines: list[str], line: int) -> str:
    return lines[line - 1].strip()[:200] if 0 < line <= len(lines) else ""


def build(ctx, tree) -> Graph:
    """tree の依存の辺を作る。tree ごとに 1 回だけ計算して tree に持たせる。"""
    cached = getattr(tree, "_rh_dep_graph", None)
    if cached is not None:
        return cached
    enabled = ctx.profile.enabled_adapters
    product: list = []
    rust_all: list[str] = []  # 論理 module の表は試験・道具の file にも作る（`crate::x` が module かの判定に使う）
    for info in ctx.target_files(tree):
        if info.language == "rust":
            rust_all.append(info.path)
        if info.cls == "product" and source_size.is_product_source(info.path):
            product.append(info)
    edges: list[Edge] = []
    packages: dict[str, str] = {}
    modules: dict[str, tuple[str, tuple[str, ...] | None]] = {}
    if "rust" in enabled:
        rust_edges, packages, modules = _rust(ctx, tree, product, rust_all)
        edges.extend(rust_edges)
    if "python" in enabled:
        edges.extend(_python(tree, [i for i in product if i.language == "python"]))
    if "typescript" in enabled:
        edges.extend(_typescript(tree, [i for i in product if i.language == "typescript"]))
    edges = sorted(set(edges), key=lambda e: (e.src, e.line, e.dst, e.kind))
    graph = Graph(edges=edges, packages=packages, modules=modules)
    tree._rh_dep_graph = graph  # noqa: SLF001 — tree ごとの cache
    return graph


# ------------------------------------------------------------------ Rust

def _packages(ctx, tree, infos) -> dict[str, str]:
    """package 名 -> dir。同名があれば試験・fixture の外のもの、次に path の短いものを採る。"""
    found: dict[str, list[str]] = {}
    for info in infos:
        if not (info.path == "Cargo.toml" or info.path.endswith("/Cargo.toml")):
            continue
        text = tree.read_text(info.path) or ""
        try:
            data = tomllib.loads(text)
        except tomllib.TOMLDecodeError:
            continue
        name = data.get("package", {}).get("name")
        if isinstance(name, str):
            found.setdefault(name, []).append(posixpath.dirname(info.path))
    out = {}
    for name, dirs in found.items():
        dirs.sort(key=lambda d: (len(d), d))
        out[name] = dirs[0]
    return out


def _cargo_dep_line(lines: list[str], key: str) -> int:
    pat = re.compile(r"^\s*(\[[^\]]*dependencies\.)?\"?" + re.escape(key) + r"\"?\s*(=|\]|\.)")
    for i, ln in enumerate(lines, 1):
        if pat.match(ln):
            return i
    return 1


def _cargo_edges(tree, packages: dict[str, str]) -> list[Edge]:
    by_dir = {d: n for n, d in packages.items()}
    edges = []
    for name, pkg_dir in sorted(packages.items()):
        manifest = posixpath.join(pkg_dir, "Cargo.toml") if pkg_dir else "Cargo.toml"
        text = tree.read_text(manifest) or ""
        lines = text.splitlines()
        try:
            data = tomllib.loads(text)
        except tomllib.TOMLDecodeError:
            continue
        tables: list[tuple[str, dict]] = []
        for sect, kind in (("dependencies", "cargo"), ("build-dependencies", "cargo"), ("dev-dependencies", "cargo-dev")):
            tables.append((kind, data.get(sect, {})))
            for tgt in data.get("target", {}).values():
                if isinstance(tgt, dict):
                    tables.append((kind, tgt.get(sect, {})))
        for kind, deps in tables:
            for key, spec in sorted(deps.items()):
                target = None
                if isinstance(spec, dict) and isinstance(spec.get("path"), str):
                    p = posixpath.normpath(posixpath.join(pkg_dir, spec["path"]))
                    target = by_dir.get("" if p == "." else p)
                if target is None:
                    real = spec.get("package", key) if isinstance(spec, dict) else key
                    target = real if real in packages else None
                if target is None or target == name:
                    continue
                dst_dir = packages[target]
                edges.append(Edge(
                    src=manifest, line=_cargo_dep_line(lines, key),
                    dst=posixpath.join(dst_dir, "Cargo.toml") if dst_dir else "Cargo.toml",
                    src_unit=pkg_dir or ".", dst_unit=dst_dir or ".", kind=kind,
                    excerpt=_excerpt(lines, _cargo_dep_line(lines, key))))
    return edges


def _lib_name(tree, pkg_dir: str, name: str) -> str:
    text = tree.read_text(posixpath.join(pkg_dir, "Cargo.toml") if pkg_dir else "Cargo.toml") or ""
    try:
        lib = tomllib.loads(text).get("lib", {})
    except tomllib.TOMLDecodeError:
        lib = {}
    return str(lib.get("name") or name).replace("-", "_")


def _package_of(path: str, pkg_dirs) -> str | None:
    best = None
    for d in pkg_dirs:
        prefix = d + "/" if d else ""
        if path.startswith(prefix) and (best is None or len(d) > len(best)):
            best = d
    return best


def _path_module(path: str, pkg_dir: str) -> tuple[str, ...] | None:
    """宣言から届かない file の論理 module（src からの path）。crate root は ()、src の外と src/bin/** は None。"""
    parts = (path[len(pkg_dir) + 1:] if pkg_dir else path).split("/")
    if len(parts) < 2 or parts[0] != "src" or parts[1] == "bin":
        return None
    parts = parts[1:]
    if len(parts) == 1 and parts[0] in RUST_ROOT_FILES:
        return ()
    stem = parts[-1][:-3] if parts[-1].endswith(".rs") else parts[-1]
    return tuple(parts[:-1]) if stem == "mod" else tuple(parts[:-1]) + (stem,)


MOD_DECL_RE = re.compile(r"^(?:#\[path\s*=\s*\"([^\"]+)\"\]\s*)?(?:pub(?:\([^)]*\))?\s+)?mod\s+(?:r#)?([A-Za-z_]\w*)\s*;")
PATH_ATTR_RE = re.compile(r'^#\[path\s*=\s*"([^"]+)"\]')


def _mod_decls(tree, path: str, owns_dir: bool) -> list[tuple[str, str, bool]]:
    """file の top level の `mod x;`（試験の範囲・文字列の中を除く）-> [(x, 読む file, #[path] か)]。"""
    text = tree.read_text(path)
    if text is None:
        return []
    lines, smask, tmask = source_size.rust_masks(text)
    here = posixpath.dirname(path)
    child_dir = here if owns_dir else posixpath.join(here, posixpath.basename(path)[:-3])
    out = []
    for i, ln in enumerate(lines):
        m = None if smask[i] or tmask[i] else MOD_DECL_RE.match(ln)
        if not m:
            continue
        attr, j = m.group(1), i - 1
        while attr is None and j >= 0 and lines[j].startswith(("#[", "//")):
            a = PATH_ATTR_RE.match(lines[j])
            attr, j = (a.group(1) if a else None), j - 1
        if attr is not None:
            target = posixpath.normpath(posixpath.join(here, attr))
            if tree.has(target):
                out.append((m.group(2), target, True))
            continue
        for cand in (f"{child_dir}/{m.group(2)}.rs", f"{child_dir}/{m.group(2)}/mod.rs"):
            if tree.has(cand):
                out.append((m.group(2), cand, False))
                break
    return out


def rust_modules(tree, packages: dict[str, str], paths) -> dict[str, tuple[str, tuple[str, ...] | None]]:
    """Rust file -> (package dir, 論理 module の path)。paths（本体の file）は宣言から届かなくても path で決めて載せる。"""
    pkg_dirs = sorted(packages.values())
    table: dict[str, tuple[str, tuple[str, ...] | None]] = {}
    for path in paths:
        pkg = _package_of(path, pkg_dirs)
        if pkg is not None:
            table[path] = (pkg, _path_module(path, pkg))
    for pkg in pkg_dirs:
        prefix = f"{pkg}/src/" if pkg else "src/"
        roots = {f"{prefix}{r}": ("", (), True) for r in RUST_ROOT_FILES if tree.has(f"{prefix}{r}")}
        chosen = dict(roots)
        for _ in range(64):  # 宣言の木の深さで止まる。同じ file を複数の宣言が読むときの選び直しの上限
            cands: dict[str, tuple[str, tuple[str, ...], bool]] = {}
            for decl, (_, logical, owns) in sorted(chosen.items()):
                for name, target, via_attr in _mod_decls(tree, decl, owns):
                    if target in roots or (target in cands and cands[target][0] <= decl):
                        continue
                    cands[target] = (decl, logical + (name,), via_attr or target.endswith("/mod.rs"))
            nxt = {**roots, **cands}
            if nxt == chosen:
                break
            chosen = nxt
        for path, (_, logical, _) in chosen.items():
            table[path] = (pkg, logical)
    return table


def rust_module_of(graph: Graph, path: str) -> tuple[str, str | None] | None:
    """Rust file の (package dir, 論理 module の第 1 要素)。crate root と src の外は module None。"""
    loc = graph.modules.get(path)
    if loc is None:
        pkg = _package_of(path, sorted(graph.packages.values()))
        if pkg is None:
            return None
        loc = (pkg, _path_module(path, pkg))
    return loc[0], (loc[1][0] if loc[1] else None)


def _unit(pkg_dir: str, module: str | None) -> str:
    base = pkg_dir or "."
    return f"{base}/src/{module}" if module else base


def _module_files(table) -> dict[tuple[str, str], str]:
    """(package dir, crate root の直下の module 名) -> その module の file。"""
    out: dict[tuple[str, str], str] = {}
    for path, (pkg, logical) in sorted(table.items()):
        if logical is not None and len(logical) == 1:
            out.setdefault((pkg, logical[0]), path)
    return out


INLINE_MOD_RE = re.compile(r"^(\s*)(?:pub(?:\([^)]*\))?\s+)?mod\s+(?:r#)?([A-Za-z_]\w*)\s*\{")


def _inline_mods(raw: str) -> list[tuple[str, ...]]:
    """行ごとの inline の `mod x { … }` block の名前の列（外から順）。`super::` の解決に使う。"""
    lines, smask, _ = source_size.rust_masks(raw)
    out: list[tuple[str, ...]] = [()] * len(lines)
    for i, ln in enumerate(lines):
        m = None if smask[i] else INLINE_MOD_RE.match(ln)
        if not m:
            continue
        end = source_size.block_end(lines, i + 1, len(m.group(1)), smask)
        for k in range(i + 1, min(end, len(lines))):
            out[k] = out[k] + (m.group(2),)
    return out


USE_RE = re.compile(r"\buse\s+([^;{}]*(?:\{[^;]*\})?[^;]*);", re.S)
STRING_RE = re.compile(r'"(?:\\.|[^"\\])*"')


def expand_use(text: str) -> list[str]:
    """`a::{b, c::{d}}` -> ["a::b", "a::c::d"]（B0 の expand_use と同じ）。"""
    text = re.sub(r"\s+", "", text)
    out: list[str] = []

    def rec(prefix: str, s: str) -> None:
        i = s.find("{")
        if i < 0:
            out.append(prefix + s)
            return
        head, body = s[:i], s[i + 1:s.rfind("}")]
        depth, start, items = 0, 0, []
        for j, ch in enumerate(body):
            if ch == "{":
                depth += 1
            elif ch == "}":
                depth -= 1
            elif ch == "," and depth == 0:
                items.append(body[start:j])
                start = j + 1
        items.append(body[start:])
        for it in items:
            if it:
                rec(prefix + head, it)

    rec("", text)
    return out


@functools.lru_cache(maxsize=None)
def rust_code_lines(text: str) -> tuple[str, ...]:
    """試験の範囲・文字列の中で始まる行・行の // 以降を空にした行（行番号は保つ）。"""
    lines, mask, test = source_size.rust_masks(text)
    out = []
    for i, ln in enumerate(lines):
        if mask[i] or test[i]:
            out.append("")
            continue
        ln = STRING_RE.sub('""', ln)
        cut = ln.find("//")
        out.append(ln[:cut] if cut >= 0 else ln)
    return tuple(out)


SUPER_RE = re.compile(r"(?<![\w:])((?:super::)+)(?!super\b)([A-Za-z_][A-Za-z0-9_]*)")


def _rust(ctx, tree, infos, rust_all) -> tuple[list[Edge], dict[str, str], dict[str, tuple[str, tuple[str, ...] | None]]]:
    packages = _packages(ctx, tree, infos)
    edges = _cargo_edges(tree, packages)
    table = rust_modules(tree, packages, rust_all)
    graph = Graph(edges=[], packages=packages, modules=table)
    module_files = _module_files(table)
    libs = {_lib_name(tree, d, n): d for n, d in packages.items()}
    for path in (i.path for i in infos if i.language == "rust"):
        loc = rust_module_of(graph, path)
        if loc is None or loc[1] is None:
            continue
        pkg_dir, module = loc
        logical = table[path][1] or ()
        raw = tree.read_text(path) or ""
        code = rust_code_lines(raw)
        blocks = _inline_mods(raw)
        raw_lines = raw.splitlines()
        text = "\n".join(code)
        refs: list[tuple[int, list[str]]] = []
        for m in USE_RE.finditer(text):
            line = _line_of(text, m.start())
            refs.extend((line, [s for s in p.split("::") if s]) for p in expand_use(m.group(1)))
        for i, ln in enumerate(code, 1):
            for m in re.finditer(r"(?<![\w:])([a-z_][a-z0-9_]*)::([A-Za-z_][A-Za-z0-9_]*)", ln):
                refs.append((i, [m.group(1), m.group(2)]))
            for m in SUPER_RE.finditer(ln):
                refs.append((i, ["super"] * m.group(1).count("super") + [m.group(2)]))
        for line, segs in refs:
            if len(segs) < 2:
                continue
            if segs[0] == "super":
                ups = next((k for k, seg in enumerate(segs) if seg != "super"), len(segs))
                here = logical + (blocks[line - 1] if line <= len(blocks) else ())
                if ups != len(here) or ups >= len(segs):
                    continue  # 自分の単位の中か、crate root より上
                tgt_pkg, tgt_mod = pkg_dir, segs[ups]
            elif segs[0] == "crate":
                tgt_pkg, tgt_mod = pkg_dir, segs[1]
            elif segs[0] in libs and libs[segs[0]] != pkg_dir:
                tgt_pkg, tgt_mod = libs[segs[0]], segs[1]
            else:
                continue
            dst = module_files.get((tgt_pkg, tgt_mod))
            if tgt_pkg == pkg_dir:
                if dst is None or tgt_mod == module:
                    continue  # crate root の再 export・自 module
                dst_unit = _unit(pkg_dir, tgt_mod)
            else:
                dst_unit = _unit(tgt_pkg, tgt_mod) if dst is not None else _unit(tgt_pkg, None)
                if dst is None:
                    dst = posixpath.join(tgt_pkg, "Cargo.toml") if tgt_pkg else "Cargo.toml"
            edges.append(Edge(src=path, line=line, dst=dst, src_unit=_unit(pkg_dir, module),
                              dst_unit=dst_unit, kind="use", excerpt=_excerpt(raw_lines, line)))
    return edges, packages, table


# ------------------------------------------------------------------ Python

def _py_resolve(tree, importer: str, module: str, level: int, names: list[str]) -> list[str]:
    parts = module.split(".") if module else []
    if level:
        base = posixpath.dirname(importer)
        for _ in range(level - 1):
            base = posixpath.dirname(base)
        roots = [base]
    else:
        roots = []
        d = posixpath.dirname(importer)
        while True:
            roots.append(d)
            if not d:
                break
            d = posixpath.dirname(d)
    for root in roots:
        stem = posixpath.join(root, *parts) if parts else root
        found = []
        if parts:
            for cand in (stem + ".py", posixpath.join(stem, "__init__.py")):
                if tree.has(cand):
                    found.append(cand)
                    break
        for name in names:  # from pkg import submodule
            for cand in (posixpath.join(stem, name + ".py"), posixpath.join(stem, name, "__init__.py")):
                if tree.has(cand):
                    found.append(cand)
                    break
        if found:
            return found
        if level:
            break
    return []


def _python(tree, infos) -> list[Edge]:
    edges = []
    for info in infos:
        text = tree.read_text(info.path)
        if text is None:
            continue
        try:
            mod = ast.parse(text)
        except (SyntaxError, ValueError):
            continue
        lines = text.splitlines()
        for node in ast.walk(mod):
            targets: list[str] = []
            if isinstance(node, ast.Import):
                for alias in node.names:
                    targets += _py_resolve(tree, info.path, alias.name, 0, [])
            elif isinstance(node, ast.ImportFrom):
                targets += _py_resolve(tree, info.path, node.module or "", node.level,
                                       [a.name for a in node.names if a.name != "*"])
            for dst in sorted(set(targets)):
                if dst != info.path:
                    edges.append(Edge(src=info.path, line=node.lineno, dst=dst, src_unit=info.path,
                                      dst_unit=dst, kind="import", excerpt=_excerpt(lines, node.lineno)))
    return edges


# ------------------------------------------------------------------ TypeScript

TS_IMPORT_RE = re.compile(
    r"""(?:^|[;\n}])\s*(import|export)\s+(type\s+)?(?:[^'";]*?\bfrom\s*)?['"](\.{1,2}/[^'"]*)['"]"""
    r"""|\b(?:import|require)\s*\(\s*['"](\.{1,2}/[^'"]*)['"]\s*\)""",
    re.S,
)


def _ts_resolve(tree, importer: str, spec: str) -> str | None:
    p = posixpath.normpath(posixpath.join(posixpath.dirname(importer), spec))
    cands = [p]
    stem, ext = posixpath.splitext(p)
    if ext in (".js", ".jsx", ".mjs", ".cjs"):
        cands += [stem + e for e in (".ts", ".tsx", ".mts", ".cts")]
    cands += [p + e for e in TS_EXTS] + [posixpath.join(p, "index" + e) for e in TS_EXTS]
    for c in cands:
        if tree.has(c):
            return c
    return None


def _ts_strip_comments(text: str) -> str:
    """// と /* */ の注釈を空白にする（行番号を保つ）。文字列の中の // は URL なので import の判定に影響しない。"""
    text = re.sub(r"/\*.*?\*/", lambda m: re.sub(r"[^\n]", " ", m.group(0)), text, flags=re.S)
    return re.sub(r"(?m)^\s*//[^\n]*", "", text)


def _typescript(tree, infos) -> list[Edge]:
    edges = []
    for info in infos:
        raw = tree.read_text(info.path)
        if raw is None:
            continue
        text = _ts_strip_comments(raw)
        lines = raw.splitlines()
        for m in TS_IMPORT_RE.finditer(text):
            spec = m.group(3) or m.group(4)
            dst = _ts_resolve(tree, info.path, spec)
            if dst is None or dst == info.path:
                continue
            start = m.start(1) if m.group(1) else m.start()
            line = _line_of(text, start)
            edges.append(Edge(src=info.path, line=line, dst=dst, src_unit=info.path, dst_unit=dst, kind="import",
                              excerpt=_excerpt(lines, line), type_only=bool(m.group(2))))
    return edges


# ------------------------------------------------------------------ 強連結成分

def sccs(nodes: set[str], adj: dict[str, set[str]]) -> list[list[str]]:
    """大きさ 2 以上の強連結成分（Tarjan、反復版。決定的な順）。"""
    index: dict[str, int] = {}
    low: dict[str, int] = {}
    on: set[str] = set()
    stack: list[str] = []
    out: list[list[str]] = []
    counter = 0
    for root in sorted(nodes):
        if root in index:
            continue
        work = [(root, iter(sorted(adj.get(root, ()))))]
        index[root] = low[root] = counter
        counter += 1
        stack.append(root)
        on.add(root)
        while work:
            v, it = work[-1]
            advanced = False
            for w in it:
                if w not in index:
                    index[w] = low[w] = counter
                    counter += 1
                    stack.append(w)
                    on.add(w)
                    work.append((w, iter(sorted(adj.get(w, ())))))
                    advanced = True
                    break
                if w in on:
                    low[v] = min(low[v], index[w])
            if advanced:
                continue
            work.pop()
            if work:
                low[work[-1][0]] = min(low[work[-1][0]], low[v])
            if low[v] == index[v]:
                comp = []
                while True:
                    w = stack.pop()
                    on.discard(w)
                    comp.append(w)
                    if w == v:
                        break
                if len(comp) > 1:
                    out.append(sorted(comp))
    return sorted(out)
