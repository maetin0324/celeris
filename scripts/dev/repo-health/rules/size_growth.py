"""rule size.responsibility-growth（ADR 2026-11-07-repo-health-contract D5）: 巨大責務の悪化。

構造指標を base と head で同じ方法で数え、閾値を超えた単位を finding にする。base にも同じ単位の finding が
あれば existing、値が増えていれば worsened、base で閾値以下（か無かった）なら new（engine の D6）。

指標（値が閾値を超えたら finding。閾値は profile の rule 設定、0 でその指標を数えない）:
    file_lines    = 2000   # file の本体の行（Rust は #[cfg(test)] の範囲を除く）。既定は source-size の production 閾値
    module_lines  = 8000   # Rust の module（package の src 直下の第 1 要素）の本体の行の合計。空行・注釈・
                           # use・mod 宣言は数えない（分割で増える宣言を悪化と数えない）
    type_methods  = 60     # 型の method の数（Rust は同じ module の impl block 全部、Python は class の def）
    type_fields   = 40     # 型の field の数（Rust の struct、Python の class 本体の代入・注釈）
    fn_lines      = 200    # 関数 1 つの行
- file 分割・行の付け替えだけでは減らない: module・型・関数の鍵は file の path でなく module（Rust は
  `<pkg>/src/<module>`、Python は dir）と名前で作る。`foo.rs` を `foo/a.rs`・`foo/b.rs` に分けても module foo の
  合計と型の method 数は変わらず、base と同じ finding（existing）のまま残る。
- 対象: adapters の種類が product で、source-size-report.py の分類（health/source_size.py）でも手書きの本体の
  Rust・Python・TypeScript（TypeScript は file_lines だけ）。Rust の本体の行・試験の範囲・block の終わりは
  source-size-report.py の関数で数える（rustfmt の書式が前提）。
"""

from __future__ import annotations

import ast
import posixpath
import re

from health import lang_deps, source_size
from health.rule import Hit, Rule, Unknown

DEFAULTS = {"file_lines": None, "module_lines": 8000, "type_methods": 60, "type_fields": 40, "fn_lines": 200}

FN_RE = re.compile(r"^(\s*)(?:pub(?:\([^)]*\))?\s+)?(?:default\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?"
                   r"(?:extern\s+\"[^\"]*\"\s+)?fn\s+([A-Za-z_]\w*)")
STRUCT_RE = re.compile(r"^(\s*)(?:pub(?:\([^)]*\))?\s+)?struct\s+([A-Za-z_]\w*)[^;(]*\{\s*$")
IMPL_RE = re.compile(r"^(\s*)(?:unsafe\s+)?impl\b(.*)$")
FIELD_RE = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:r#)?[a-z_][A-Za-z0-9_]*\s*:(?!:)")


def _thresholds(cfg: dict) -> dict[str, int]:
    bad = sorted(set(cfg) - set(DEFAULTS) - {"enabled", "required"})
    if bad:
        raise Unknown(f"config_invalid: rules.size.responsibility-growth に未知の欄 {bad}")
    out = {}
    for k, default in DEFAULTS.items():
        v = cfg.get(k, default)
        if v is None:  # file_lines の既定は source-size の production 閾値
            v = source_size.module().DEFAULT_PRODUCTION_THRESHOLD
        if not isinstance(v, int) or isinstance(v, bool) or v < 0:
            raise Unknown(f"config_invalid: rules.size.responsibility-growth.{k} は 0 以上の整数")
        out[k] = v
    return out


class _Acc:
    """鍵ごとの値と evidence。同じ鍵が複数の場所にあれば sum（型の method・module の行）か max（関数）。"""

    def __init__(self) -> None:
        self.items: dict[tuple[str, str], dict] = {}

    def add(self, metric: str, key: str, value: int, ev: dict, scope: tuple[str, str], how: str = "sum") -> None:
        cur = self.items.setdefault((metric, key), {"value": 0, "evidence": [], "scope": scope})
        if how == "sum":
            cur["value"] += value
            cur["evidence"].append((value, ev))
        elif value > cur["value"]:
            cur["value"] = value
            cur["evidence"] = [(value, ev)]


def _impl_type(rest: str) -> str | None:
    rest = rest.split(" where ")[0].rstrip("{ ").strip()
    if rest.startswith("<"):  # impl<T> の generic を飛ばす
        depth = 0
        for i, ch in enumerate(rest):
            depth += ch == "<"
            depth -= ch == ">"
            if depth == 0:
                rest = rest[i + 1:]
                break
    if " for " in rest:
        rest = rest.rsplit(" for ", 1)[1]
    m = re.match(r"\s*(?:&\s*(?:'\w+\s+)?)?(?:mut\s+)?(?:dyn\s+)?((?:[A-Za-z_]\w*::)*)([A-Za-z_]\w*)", rest)
    return m.group(2) if m else None


def _block_start(lines: list[str], i: int, indent: int) -> int | None:
    """宣言の行 i から本体の `{` の行を探す（`;` が先なら本体なし）。"""
    for j in range(i, min(len(lines), i + 40)):
        s = lines[j].rstrip()
        if s.endswith("{"):
            return j
        if s.endswith(";") or (j > i and s == " " * indent + "}"):
            return None
    return None


DECL_RE = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:use|mod)\s")


def _logic_lines(lines: list[str], live: list[bool]) -> int:
    """本体の行のうち、空行・// 注釈・use 文（複数行も）・`mod x;` を除いた行の数。"""
    n, in_use = 0, False
    for i, ln in enumerate(lines):
        s = ln.strip()
        if not live[i]:
            continue
        if in_use:
            in_use = not s.endswith(";")
            continue
        if not s or s.startswith("//"):
            continue
        if DECL_RE.match(ln) and (s.endswith(";") or not s.endswith("{")):
            in_use = not s.endswith(";")
            continue
        n += 1
    return n


def _rust(acc: _Acc, path: str, text: str, unit: str, th: dict) -> tuple[int, int]:
    lines, smask, tmask = source_size.rust_masks(text)
    live = [not (smask[i] or tmask[i]) for i in range(len(lines))]
    impls: list[tuple[int, int, str]] = []
    for i, ln in enumerate(lines):
        if not live[i]:
            continue
        m = IMPL_RE.match(ln)
        if m and (ln.rstrip().endswith("{") or "where" in ln or ln.rstrip().endswith(">")):
            indent = len(m.group(1))
            start = _block_start(lines, i, indent)
            name = _impl_type(m.group(2))
            if start is None or name is None:
                continue
            end = source_size.block_end(lines, start + 1, indent, smask)
            impls.append((i, end, name))
            methods = sum(1 for j in range(start + 1, end) if live[j] and (fm := FN_RE.match(lines[j]))
                          and len(fm.group(1)) == indent + 4)
            if th["type_methods"]:
                acc.add("type_methods", f"{unit}::{name}", methods,
                        {"path": path, "line": i + 1, "end_line": end + 1, "symbol": f"impl {name}"},
                        ("module", unit))
            continue
        m = STRUCT_RE.match(ln)
        if m and th["type_fields"]:
            indent = len(m.group(1))
            end = source_size.block_end(lines, i + 1, indent, smask)
            fields = sum(1 for j in range(i + 1, end) if live[j] and FIELD_RE.match(lines[j])
                         and len(lines[j]) - len(lines[j].lstrip()) == indent + 4)
            acc.add("type_fields", f"{unit}::{m.group(2)}", fields,
                    {"path": path, "line": i + 1, "end_line": end + 1, "symbol": f"struct {m.group(2)}"},
                    ("module", unit), how="max")
    if th["fn_lines"]:
        for i, ln in enumerate(lines):
            m = FN_RE.match(ln) if live[i] else None
            if not m:
                continue
            indent = len(m.group(1))
            start = _block_start(lines, i, indent)
            if start is None:
                continue
            end = source_size.block_end(lines, start + 1, indent, smask)
            owner = next((n for a, b, n in impls if a < i <= b), "")
            name = f"{owner}::{m.group(2)}" if owner else m.group(2)
            acc.add("fn_lines", f"{unit}::{name}", end - i + 1,
                    {"path": path, "line": i + 1, "end_line": end + 1, "symbol": f"fn {name}"},
                    ("module", unit), how="max")
    return sum(live), _logic_lines(lines, live)


def _python(acc: _Acc, path: str, text: str, th: dict) -> None:
    try:
        tree = ast.parse(text)
    except (SyntaxError, ValueError):
        return
    unit = posixpath.dirname(path) or "."

    def walk(node, owner: str | None) -> None:
        for child in ast.iter_child_nodes(node):
            if isinstance(child, ast.ClassDef):
                body_fns = [c for c in child.body if isinstance(c, (ast.FunctionDef, ast.AsyncFunctionDef))]
                fields = [c for c in child.body if isinstance(c, (ast.Assign, ast.AnnAssign))]
                ev = {"path": path, "line": child.lineno, "end_line": child.end_lineno, "symbol": f"class {child.name}"}
                if th["type_methods"]:
                    acc.add("type_methods", f"{unit}::{child.name}", len(body_fns), ev, ("module", unit))
                if th["type_fields"]:
                    acc.add("type_fields", f"{unit}::{child.name}", len(fields), ev, ("module", unit), how="max")
                walk(child, child.name)
            elif isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef)):
                if th["fn_lines"]:
                    name = f"{owner}::{child.name}" if owner else child.name
                    key = f"{unit}::{name}" if owner else f"{path}::{name}"
                    acc.add("fn_lines", key, (child.end_lineno or child.lineno) - child.lineno + 1,
                            {"path": path, "line": child.lineno, "end_line": child.end_lineno, "symbol": f"def {name}"},
                            ("module", unit), how="max")
                walk(child, owner)
            else:
                walk(child, owner)

    walk(tree, None)


LABEL = {
    "file_lines": "file の本体の行",
    "module_lines": "module の本体の行（空行・注釈・use・mod 宣言を除く）の合計",
    "type_methods": "型の method の数",
    "type_fields": "型の field の数",
    "fn_lines": "関数の行",
}


def evaluate(ctx, tree):
    th = _thresholds(ctx.config)
    acc = _Acc()
    enabled = ctx.profile.enabled_adapters
    pkg_dirs = None
    for info in ctx.target_files(tree, languages=("rust", "python", "typescript"), classes=("product",)):
        if info.language not in enabled or not source_size.is_product_source(info.path):
            continue
        text = tree.read_text(info.path)
        if text is None:
            continue
        if info.language == "rust":
            if pkg_dirs is None:
                pkg_dirs = sorted(lang_deps.build(ctx, tree).packages.values())
            loc = lang_deps.rust_module_of(info.path, pkg_dirs)
            unit = (f"{loc[0] or '.'}/src/{loc[1]}" if loc[1] else (loc[0] or ".")) if loc else posixpath.dirname(info.path)
            prod, logic = _rust(acc, info.path, text, unit, th)
            if loc and loc[1] and th["module_lines"]:
                acc.add("module_lines", unit, logic, {"path": info.path, "line": 1, "end_line": max(1, len(text.splitlines()))},
                        ("module", unit))
        else:
            prod = len(text.splitlines())
            if info.language == "python":
                _python(acc, info.path, text, th)
        if th["file_lines"]:
            acc.add("file_lines", info.path, prod, {"path": info.path, "line": 1, "end_line": max(1, prod)},
                    ("file", info.path))
    hits = []
    for (metric, key), item in sorted(acc.items.items()):
        limit = th[metric]
        if not limit or item["value"] <= limit:
            continue
        evs = sorted(item["evidence"], key=lambda t: (-t[0], t[1]["path"], t[1].get("line", 0)))[:20]
        evidence = [dict(e, ref=tree.side) for _, e in evs]
        if metric == "file_lines":
            evidence = [{"path": key, "line": 1, "ref": tree.side}]
        why = f"{LABEL[metric]} {item['value']} が閾値 {limit} を超えた（単位 {key}）。"
        if metric in ("module_lines", "type_methods"):
            why += f" {len(item['evidence'])} 箇所の合計で、file を分けても減らない。"
        hits.append(Hit(evidence=evidence, scope=item["scope"], rationale=why, key=f"{metric}:{key}",
                        value=float(item["value"]), threshold=float(limit)))
    return hits


RULES = [Rule(id="size.responsibility-growth", kind="structure", evaluate=evaluate, adapter="generic",
              languages=("rust", "python", "typescript"))]
