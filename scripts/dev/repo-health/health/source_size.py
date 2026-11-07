"""既存の scripts/dev/source-size-report.py（ADR-0083）の分類を読み込む口。

size.responsibility-growth と deps.* は、本体の行・試験の行の分け方を source-size-report.py から import して
使う（二重に実装しない）: path の粗い分類 `classify_path`、外部試験 `is_external_rust_test`・`is_ts_test`、
Rust の文字列・注釈の範囲 `rust_string_mask`、`#[cfg(test)]` の範囲（`analyze_rust_file` と同じ判定で行ごとの mask を作り、行数を照合する）、
rustfmt 前提の block の終わり `block_end`。

読み込むのは検査器と同じ checkout の file（`<検査器の dir>/../source-size-report.py`）。無ければ
`Unsupported("tool_missing:source-size-report.py")` にする（黙って別の分類で数えない）。
"""

from __future__ import annotations

import functools
import importlib.util
import os
import sys

from .rule import Unsupported

PATH = os.path.join(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))),
                    "source-size-report.py")
_MODULE = None

# 構造指標の対象にしない source-size の分類（手書きの本体でないもの）。
NON_PRODUCT = {"vendored", "generated", "schema", "lockfile", "fixture", "migrations", "docs"}


def module():
    global _MODULE
    if _MODULE is None:
        if not os.path.isfile(PATH):
            raise Unsupported("tool_missing:source-size-report.py")
        spec = importlib.util.spec_from_file_location("repo_health_source_size_report", PATH)
        assert spec is not None and spec.loader is not None
        mod = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = mod  # dataclass の解決に要る
        spec.loader.exec_module(mod)
        _MODULE = mod
    return _MODULE


def rust_string_mask(lines: list[str]) -> list[bool]:
    return module().rust_string_mask(lines)


def rust_test_mask(lines: list[str], str_mask: list[bool] | None = None) -> list[bool]:
    """`#[cfg(test)]` の範囲の行の mask。

    source-size-report.py の `analyze_rust_file` は行数だけを返し、行ごとの mask を返さない。この WorkUnit は
    source-size-report.py を変えられないので、同じ判定（`CFG_TEST_RE`・`MOD_DECL_RE`・`block_end` は
    source-size-report.py のもの）で mask を作り、試験の行数が `analyze_rust_file(lines).inline_test_lines`
    と一致することを毎回確かめる。食い違えば Unsupported にして、黙って別の数え方をしない。"""
    mod = module()
    if str_mask is None:
        str_mask = mod.rust_string_mask(lines)
    n = len(lines)
    mask = [False] * n
    for i, l in enumerate(lines):
        if not mod.CFG_TEST_RE.match(l):
            continue
        j = i + 1
        while j < n and (lines[j].strip().startswith("#[") or not lines[j].strip()):
            j += 1
        if j >= n:
            continue
        m = mod.MOD_DECL_RE.match(lines[j])
        if m and m.group(3) == "{":
            end = mod.block_end(lines, j + 1, len(m.group(1)), str_mask)
        elif m and m.group(3) == ";":
            end = j
        else:
            indent = len(lines[j]) - len(lines[j].lstrip())
            end = j if lines[j].rstrip().endswith(";") else mod.block_end(lines, j + 1, indent, str_mask)
        for k in range(i, end + 1):
            mask[k] = True
    if sum(mask) != mod.analyze_rust_file(lines).inline_test_lines:
        raise Unsupported("source_size_mask_mismatch")
    return mask


@functools.lru_cache(maxsize=None)
def rust_masks(text: str) -> tuple[tuple[str, ...], tuple[bool, ...], tuple[bool, ...]]:
    """(行、文字列・注釈の中で始まる行、試験の範囲の行)。base と head で同じ内容の file は 1 回だけ数える。"""
    lines = text.splitlines()
    smask = rust_string_mask(lines)
    return tuple(lines), tuple(smask), tuple(rust_test_mask(lines, smask))


def block_end(lines: list[str], start: int, indent: int, str_mask: list[bool]) -> int:
    return module().block_end(lines, start, indent, str_mask)


def is_product_source(path: str) -> bool:
    """source-size の分類で手書きの本体（試験・生成・vendor・fixture などでない）か。"""
    mod = module()
    if mod.classify_path(path) in NON_PRODUCT:
        return False
    if path.endswith(".rs") and mod.is_external_rust_test(path):
        return False
    if path.endswith((".ts", ".tsx", ".mjs", ".js")) and mod.is_ts_test(path):
        return False
    return True
