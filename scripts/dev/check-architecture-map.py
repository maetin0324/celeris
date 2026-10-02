#!/usr/bin/env python3
"""check-architecture-map.py — docs/architecture-map.md が指す全パスの実在を検査する。

対象は表の行（`|` で始まる行）にある backtick 区切りのパスと、Markdown リンク `[text](path)`
の相対パス。表の外の説明文にある backtick は検査しない（`crates/*/src/` のような一般的な
言及で、特定のパスを指さないため）。

パスの表記規則（このファイル自身が前提にする）:
- `{a,b,c}` はシェル風 brace expansion（ネストなし）。`a/{b,c}.rs` → `a/b.rs`, `a/c.rs`。
- `*` を含むパスは glob として、1 件以上マッチすれば OK とする。
- 直前の backtick パスにすぐ続く全角括弧 `（... `sub/path`, ... ）` の中の backtick パスは、
  直前パスと同じディレクトリからの相対パスとして展開する（Rust の親ファイル + 子 module の慣習）。

ADR / SPEC の見出しアンカー（`#...`）はレンダラ依存のため検査しない。
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
DOC_PATH = REPO_ROOT / "docs" / "architecture-map.md"

TABLE_ROW_RE = re.compile(r"^\s*\|.*\|\s*$")
BACKTICK_RE = re.compile(r"`([^`]+)`")
MD_LINK_RE = re.compile(r"\[[^\]]*\]\(([^)]+)\)")
BRACE_RE = re.compile(r"\{([^{}]*)\}")


def expand_braces(pattern: str) -> list[str]:
    m = BRACE_RE.search(pattern)
    if not m:
        return [pattern]
    prefix, choices, suffix = pattern[: m.start()], m.group(1).split(","), pattern[m.end() :]
    out: list[str] = []
    for choice in choices:
        out.extend(expand_braces(prefix + choice + suffix))
    return out


def looks_like_path(token: str) -> bool:
    token = token.strip()
    if not token or " " in token or token.startswith("http"):
        return False
    return "/" in token or token.endswith((".rs", ".tsx", ".ts", ".md", ".py", ".sh"))


def collect_backtick_paths(line: str) -> set[str]:
    """1 つの表行から backtick パスを集める（親パス直後の（...）内は相対展開）。"""
    paths: set[str] = set()
    spans = list(BACKTICK_RE.finditer(line))
    consumed_idx: set[int] = set()

    for i, m in enumerate(spans):
        base = m.group(1)
        tail = line[m.end() : m.end() + 1]
        if tail != "（" or not looks_like_path(base):
            continue
        close = line.find("）", m.end())
        if close == -1:
            continue
        paren_body = line[m.end() : close]
        base_dir = "/".join(base.split("/")[:-1])
        for j, sub_m in enumerate(BACKTICK_RE.finditer(paren_body)):
            sub = sub_m.group(1)
            if not looks_like_path(sub):
                continue
            for expanded in expand_braces(sub):
                paths.add(f"{base_dir}/{expanded}" if base_dir else expanded)
            # 括弧内の backtick パスは全体スキャンから除外する（下の全体パスと二重にしない）
            abs_start = m.end() + sub_m.start()
            for k, span in enumerate(spans):
                if span.start() == abs_start:
                    consumed_idx.add(k)

    for i, m in enumerate(spans):
        if i in consumed_idx:
            continue
        token = m.group(1)
        if not looks_like_path(token):
            continue
        paths.update(expand_braces(token))

    return paths


def collect_md_link_paths(line: str) -> set[str]:
    paths: set[str] = set()
    for m in MD_LINK_RE.finditer(line):
        target = m.group(1).split("#", 1)[0].strip()
        if not target or target.startswith(("http://", "https://")):
            continue
        paths.add(target)
    return paths


def path_exists(rel: str, doc_dir: Path) -> bool:
    if "*" in rel:
        if list(REPO_ROOT.glob(rel)):
            return True
        if list(doc_dir.glob(rel)):
            return True
        return False
    return (REPO_ROOT / rel).exists() or (doc_dir / rel).exists()


def main() -> int:
    if not DOC_PATH.exists():
        print(f"NG: {DOC_PATH} が存在しない", file=sys.stderr)
        return 1

    doc_dir = DOC_PATH.parent
    candidates: set[str] = set()
    for line in DOC_PATH.read_text(encoding="utf-8").splitlines():
        if not TABLE_ROW_RE.match(line):
            continue
        candidates |= collect_backtick_paths(line)
        candidates |= collect_md_link_paths(line)

    missing: list[str] = []
    checked = 0
    for rel in sorted(candidates):
        if path_exists(rel, doc_dir):
            checked += 1
        else:
            missing.append(rel)

    if missing:
        print(f"NG: {len(missing)} 件のパスが存在しない（{DOC_PATH.relative_to(REPO_ROOT)}）:", file=sys.stderr)
        for rel in missing:
            print(f"  - {rel}", file=sys.stderr)
        return 1

    print(f"OK: {checked} 件のパスを確認した（{DOC_PATH.relative_to(REPO_ROOT)}）")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
