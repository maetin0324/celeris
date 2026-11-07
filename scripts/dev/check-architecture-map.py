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

表の列の形も検査する: 全節の表は `COLUMNS` の 6 列で、各行の「関連試験」は backtick の
パスを 1 つ以上、「不変条件」は空でない文を持つ。

食い違いの種類別の判定（`find_diffs`）もこの file が正本で、repo health の rule
`scripts/dev/repo-health/rules/arch_map.py` はこれを呼ぶだけ（判定を二重に持たない）。
種類（`KINDS`）と決め方:
- wrong_path: 表の path・相対リンクが実在しない（下の `main` の実在検査と同じ判定）
- missing_test: 行の「関連試験」に backtick の path が無い。対照（reference）があれば、capability の
  試験がその capability の行の「関連試験」に無い
- missing_invariant: 行の「不変条件」が空。対照があれば、capability の不変条件の根拠 path
  （evidence_path）がその capability の行のどこにも無い
- missing_row: 対照の capability の owner path がどの行の owner・entry point にも無い
- wrong_owner: 対照の capability の行はあるが、owner path の一部がどの行の owner・entry point にも無い
- stale: 行がリンクする ADR の状態行が `stale_status`（廃止・置き換えの語）に当たる

対照は JSON `{"capabilities": [{"id", "owners": [{"path"}], "tests": [path], "invariants":
[{"evidence_path"}]}]}`。どの file を対照にするかは呼び手（repo health の profile か `--reference`）が決める。
食い違いの一覧は `--diffs [--reference <path>]` で JSON 行に出す（exit は 0。合否は既定の検査と rule が決める）。
"""

from __future__ import annotations

import argparse
import json
import posixpath
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

REPO_ROOT = Path(__file__).resolve().parents[2]
DOC_PATH = REPO_ROOT / "docs" / "architecture-map.md"

TABLE_ROW_RE = re.compile(r"^\s*\|.*\|\s*$")
BACKTICK_RE = re.compile(r"`([^`]+)`")
MD_LINK_RE = re.compile(r"\[[^\]]*\]\(([^)]+)\)")
BRACE_RE = re.compile(r"\{([^{}]*)\}")
COLUMNS = ["subsystem", "owner crate/module", "entry point", "ADR / 設計", "関連試験", "不変条件"]
KINDS = ("missing_row", "missing_test", "wrong_owner", "missing_invariant", "stale", "wrong_path")
STALE_STATUS = r"(?i)superseded|deprecated|withdrawn|廃止|置き換え済み|置き換えられた|取り下げ|撤回"
STATUS_LINE_RE = re.compile(r"^\W*(状態|status)\W*[:：]", re.IGNORECASE)


def split_cells(line: str) -> list[str]:
    """表の行を列に分ける（backtick の中の `|` は区切りにしない）。"""
    cells, cur, in_code = [], "", False
    for ch in line.strip().strip("|"):
        if ch == "`":
            in_code = not in_code
        if ch == "|" and not in_code:
            cells.append(cur.strip())
            cur = ""
            continue
        cur += ch
    cells.append(cur.strip())
    return cells


def check_columns(lines: list[str]) -> list[str]:
    """表の列の形の不備を返す（行番号つき）。"""
    errors: list[str] = []
    for no, line in enumerate(lines, 1):
        if not TABLE_ROW_RE.match(line) or line.lstrip().startswith("|---"):
            continue
        cells = split_cells(line)
        if cells[0] == "subsystem":
            if cells != COLUMNS:
                errors.append(f"{no}: 見出しの列が {COLUMNS} と違う")
            continue
        if len(cells) != len(COLUMNS):
            errors.append(f"{no}: 列の数が {len(cells)}（{len(COLUMNS)} のはず）")
            continue
        tests, invariant = cells[4], cells[5]
        if not any(looks_like_path(m.group(1)) for m in BACKTICK_RE.finditer(tests)):
            errors.append(f"{no}: 関連試験に backtick のパスが無い")
        if not invariant:
            errors.append(f"{no}: 不変条件が空")
    return errors


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


@dataclass(frozen=True)
class Row:
    line: int
    cells: tuple[str, ...]
    owner_paths: frozenset[str]  # owner・entry point の列の path
    test_paths: frozenset[str]  # 関連試験の列の path
    paths: frozenset[str]  # 行の全 path（相対リンクは doc の dir から repo の path に直したもの）
    links: frozenset[str]  # 相対リンクの repo の path


@dataclass(frozen=True)
class Diff:
    kind: str  # KINDS のどれか
    line: int  # 地図の行（0 は行が無い）
    key: str  # base と head で突き合わせる鍵（行番号を含めない）
    observed: str
    paths: tuple[str, ...] = ()  # 証拠の path（地図の外）


def resolve_link(target: str, doc_rel: str) -> str:
    """doc からの相対リンクを repo の path に直す。"""
    return posixpath.normpath(posixpath.join(posixpath.dirname(doc_rel), target))


def parse_rows(lines: list[str], doc_rel: str) -> list[Row]:
    """6 列の行（見出しと区切りを除く）を読む。列の数が違う行は check_columns が不備にする。"""
    rows: list[Row] = []
    for no, line in enumerate(lines, 1):
        if not TABLE_ROW_RE.match(line) or line.lstrip().startswith("|---"):
            continue
        cells = split_cells(line)
        if cells[0] == "subsystem" or len(cells) != len(COLUMNS):
            continue
        links = frozenset(resolve_link(t, doc_rel) for t in collect_md_link_paths(line))
        rows.append(Row(
            line=no,
            cells=tuple(cells),
            owner_paths=frozenset(collect_backtick_paths(cells[1]) | collect_backtick_paths(cells[2])),
            test_paths=frozenset(collect_backtick_paths(cells[4])),
            paths=frozenset(collect_backtick_paths(line)) | links,
            links=links,
        ))
    return rows


def _glob_re(pattern: str) -> re.Pattern:
    out = ""
    i = 0
    while i < len(pattern):
        if pattern.startswith("**", i):
            out += ".*"
            i += 2
        elif pattern[i] == "*":
            out += "[^/]*"
            i += 1
        else:
            out += re.escape(pattern[i])
            i += 1
    return re.compile(out + "$")


def covers(listed: str, target: str) -> bool:
    """地図の path `listed` が repo の path `target` を指すか（同じ file・dir の下・glob・同名 module の dir）。"""
    listed = listed.rstrip("/")
    if "*" in listed:
        return bool(_glob_re(listed).match(target))
    if target == listed or target.startswith(listed + "/"):
        return True
    stem = posixpath.splitext(target)[0]
    return listed.startswith(stem + "/")  # foo.rs の子 module foo/bar.rs を挙げた行は foo.rs も指す


def _covered(paths, target: str) -> bool:
    return any(covers(p, target) for p in paths)


def _stale_adrs(rows: list[Row], read_text: Callable[[str], str | None], stale_status: str) -> list[Diff]:
    pat = re.compile(stale_status)
    out: list[Diff] = []
    seen: set[tuple[int, str]] = set()
    for row in rows:
        for link in sorted(row.links):
            if not link.endswith(".md") or (row.line, link) in seen:
                continue
            seen.add((row.line, link))
            text = read_text(link)
            if text is None:
                continue
            status = next((ln for ln in text.splitlines()[:40] if STATUS_LINE_RE.match(ln)), None)
            if status and pat.search(status):
                out.append(Diff("stale", row.line, f"{row.cells[0]}|{link}",
                                f"行がリンクする {link} の状態が廃止・置き換え: {status.strip()[:120]}", (link,)))
    return out


def _reference_diffs(rows: list[Row], reference: dict) -> list[Diff]:
    out: list[Diff] = []
    owner_all = [p for r in rows for p in r.owner_paths]
    for cap in reference.get("capabilities", []):
        cid = str(cap.get("id", ""))
        owners = sorted({o["path"] for o in cap.get("owners", []) if o.get("path")})
        matched = [r for r in rows if any(_covered(r.owner_paths, o) for o in owners)]
        if not matched:
            if owners:
                out.append(Diff("missing_row", 0, cid,
                                f"capability {cid} の owner path がどの行の owner・entry point にも無い: {', '.join(owners)}",
                                tuple(owners)))
            continue
        first = min(r.line for r in matched)
        lost = [o for o in owners if not _covered(owner_all, o)]
        if lost:
            out.append(Diff("wrong_owner", first, cid,
                            f"capability {cid} の行（{first} 行目ほか）の owner・entry point に無い owner path: {', '.join(lost)}",
                            tuple(lost)))
        tests = [p for r in matched for p in r.test_paths]
        for test in sorted(set(cap.get("tests", []))):
            if not _covered(tests, test):
                out.append(Diff("missing_test", first, f"{cid}|{test}",
                                f"capability {cid} の試験 {test} が行（{first} 行目ほか）の関連試験に無い", (test,)))
        refs = [p for r in matched for p in r.paths]
        for ev in sorted({i["evidence_path"] for i in cap.get("invariants", []) if i.get("evidence_path")}):
            if not _covered(refs, ev):
                out.append(Diff("missing_invariant", first, f"{cid}|{ev}",
                                f"capability {cid} の不変条件の根拠 {ev} が行（{first} 行目ほか）のどこにも無い", (ev,)))
    return out


def find_diffs(lines: list[str], doc_rel: str, exists: Callable[[str], bool],
               read_text: Callable[[str], str | None] = lambda _p: None, reference: dict | None = None,
               stale_status: str = STALE_STATUS) -> list[Diff]:
    """地図の食い違いを種類別に返す。exists・read_text は repo の path を受ける（file system でも git tree でも）。"""
    rows = parse_rows(lines, doc_rel)
    diffs: list[Diff] = []
    seen: set[str] = set()
    for row in rows:
        if not any(looks_like_path(m.group(1)) for m in BACKTICK_RE.finditer(row.cells[4])):
            diffs.append(Diff("missing_test", row.line, f"row:{row.cells[0]}", f"{row.line} 行目: 関連試験に backtick のパスが無い"))
        if not row.cells[5]:
            diffs.append(Diff("missing_invariant", row.line, f"row:{row.cells[0]}", f"{row.line} 行目: 不変条件が空"))
        for rel in sorted(row.paths):
            if rel in seen:
                continue
            seen.add(rel)
            if not exists(rel):
                diffs.append(Diff("wrong_path", row.line, rel, f"{row.line} 行目の path {rel} が存在しない"))
    diffs.extend(_stale_adrs(rows, read_text, stale_status))
    if reference is not None:
        diffs.extend(_reference_diffs(rows, reference))
    return sorted(diffs, key=lambda d: (KINDS.index(d.kind), d.line, d.key))


def _fs_exists(rel: str) -> bool:
    return path_exists(rel, REPO_ROOT)


def _fs_read(rel: str) -> str | None:
    try:
        return (REPO_ROOT / rel).read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return None


def print_diffs(lines: list[str], reference_path: str | None) -> int:
    reference = None
    if reference_path is not None:
        reference = json.loads((REPO_ROOT / reference_path).read_text(encoding="utf-8"))
    doc_rel = DOC_PATH.relative_to(REPO_ROOT).as_posix()
    diffs = find_diffs(lines, doc_rel, _fs_exists, _fs_read, reference)
    for d in diffs:
        print(json.dumps({"kind": d.kind, "line": d.line, "key": d.key, "observed": d.observed,
                          "paths": list(d.paths)}, ensure_ascii=False))
    counts = {k: sum(1 for d in diffs if d.kind == k) for k in KINDS}
    print(json.dumps({"counts": counts, "total": len(diffs)}, ensure_ascii=False), file=sys.stderr)
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--diffs", action="store_true", help="食い違いを種類別に JSON 行で出す（exit 0）")
    ap.add_argument("--reference", help="--diffs の対照（repo の path の JSON）")
    args = ap.parse_args(argv)
    if not DOC_PATH.exists():
        print(f"NG: {DOC_PATH} が存在しない", file=sys.stderr)
        return 1

    doc_dir = DOC_PATH.parent
    lines = DOC_PATH.read_text(encoding="utf-8").splitlines()
    if args.diffs:
        return print_diffs(lines, args.reference)
    column_errors = check_columns(lines)
    if column_errors:
        print(f"NG: 表の列の形の不備 {len(column_errors)} 件（{DOC_PATH.relative_to(REPO_ROOT)}）:", file=sys.stderr)
        for err in column_errors:
            print(f"  - {err}", file=sys.stderr)
        return 1

    candidates: set[str] = set()
    for line in lines:
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
