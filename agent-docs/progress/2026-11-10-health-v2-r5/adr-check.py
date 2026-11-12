#!/usr/bin/env python3
"""ADR 2026-11-10-test-target-bundles の束の表と tree の束を照合する（健全性 v2 R5）。

呼び方:
  python3 adr-check.py <ADR の path> [--root <repo の根>]   # --root の既定は cwd
  python3 adr-check.py --self-test

exit: 0 = 一致、1 = 食い違い（1 件ずつ理由を表示）、2 = 引数・読めない入力。

照合の規則:
- 束は task-api・celeris・celerisctl・task-worker の crates/<c>/tests/<束>/main.rs。
  main.rs の最上位の `mod x;` のうち、`#[path = …]` が付かず tests/<束>/x.rs が在るものを target、
  `#[path]` 付きの mod（と x.rs の無い mod）を支援 module とする。
- ADR の表は、見出しに `crate`・『束』で始まる列・`件`・`target` を持つ Markdown の表だけを読む。
  束の欄の最初の backtick の名前が束。backtick を除いた欄が `keep` で始まる行は keep の行。
  target の欄は（…）の注記と backtick を除き、『・』で分ける。
- exit 0 は次をすべて満たす時だけ: 各束が表のちょうど 1 行に載る、その行の件数と target の集合が
  束の target の mod と一致する、表が実在しない束を挙げない、keep の行の target が
  crates/<c>/tests/<t>.rs として在り件数が target の数と一致する、`- 状態:` の行が在り『採用（設計）』を含まない。
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path

CRATES = ("task-api", "celeris", "celerisctl", "task-worker")
DESIGN_STATUS = "採用（設計）"

# main.rs の字句。左端から最初に当たるものを取るので、文字列・comment の中の `mod` は拾わない。
_TOKEN = re.compile(
    r"(?P<line_comment>//[^\n]*)"
    r"|(?P<block_comment>/\*.*?\*/)"
    r'|(?P<inner_attr>#!\s*\[(?:"(?:\\.|[^"\\])*"|[^\]"])*\])'
    r'|(?P<outer_attr>#\s*\[(?P<attr>(?:"(?:\\.|[^"\\])*"|[^\]"])*)\])'
    r"|(?P<mod>\b(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*(?P<end>[;{]))"
    r'|(?P<string>"(?:\\.|[^"\\])*")'
    r"|(?P<punct>[;{}])",
    re.S,
)
_PATH_ATTR = re.compile(r"^\s*path\s*=")
_STATUS_LINE = re.compile(r"^\s*-\s*状態\s*[:：](?P<body>.*)$")
_SEPARATOR = re.compile(r"^\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$")
_HEADING = re.compile(r"^#+\s+(?P<title>.*)$")


class InputError(Exception):
    """引数・読めない入力（exit 2）。"""


@dataclass
class Bundle:
    crate: str
    name: str
    targets: list[str]
    support: list[str]


@dataclass
class Row:
    line: int
    section: str
    crate: str
    bundle: str | None  # keep の行と名前の無い行は None
    keep: bool
    count_text: str
    targets: list[str] = field(default_factory=list)

    def where(self) -> str:
        return f"ADR {self.line} 行（{self.section}）"


def read_text(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as exc:
        raise InputError(f"{path} を読めない: {exc}") from exc


def parse_bundle_main(main_rs: Path, crate: str) -> Bundle:
    """main.rs の最上位の mod を target と支援 module に分ける。"""
    text = read_text(main_rs)
    bundle_dir = main_rs.parent
    targets: list[str] = []
    support: list[str] = []
    pending_path = False
    depth = 0
    for match in _TOKEN.finditer(text):
        kind = match.lastgroup
        if kind in ("line_comment", "block_comment", "inner_attr", "string"):
            continue
        if kind == "outer_attr":
            if depth == 0 and _PATH_ATTR.match(match.group("attr")):
                pending_path = True
            continue
        if kind == "mod":
            name = match.group("name")
            if match.group("end") == "{":
                depth += 1  # 中の mod は別の dir から解決されるので数えない
            elif depth == 0:
                if not pending_path and (bundle_dir / f"{name}.rs").is_file():
                    targets.append(name)
                else:
                    support.append(name)
            pending_path = False
            continue
        token = match.group("punct")
        if token == "{":
            depth += 1
        elif token == "}":
            depth = max(depth - 1, 0)
        pending_path = False
    return Bundle(crate=crate, name=bundle_dir.name, targets=targets, support=support)


def collect_bundles(root: Path) -> dict[tuple[str, str], Bundle]:
    bundles: dict[tuple[str, str], Bundle] = {}
    for crate in CRATES:
        tests = root / "crates" / crate / "tests"
        if not tests.is_dir():
            raise InputError(f"{tests} が無い（--root が repo の根を指していない）")
        for main_rs in sorted(tests.glob("*/main.rs")):
            bundle = parse_bundle_main(main_rs, crate)
            bundles[(crate, bundle.name)] = bundle
    return bundles


def strip_notes(cell: str) -> str:
    """（…）と (…) の注記を入れ子ごと除く。"""
    out: list[str] = []
    depth = 0
    for ch in cell:
        if ch in "（(":
            depth += 1
        elif ch in "）)" and depth > 0:
            depth -= 1
        elif depth == 0:
            out.append(ch)
    return "".join(out)


def split_cells(line: str) -> list[str]:
    body = line.strip()
    if body.startswith("|"):
        body = body[1:]
    if body.endswith("|"):
        body = body[:-1]
    return [cell.strip() for cell in body.split("|")]


def header_columns(cells: list[str]) -> dict[str, int] | None:
    names = [cell.replace("`", "").strip() for cell in cells]
    columns: dict[str, int] = {}
    for index, name in enumerate(names):
        if name == "crate" and "crate" not in columns:
            columns["crate"] = index
        elif name.startswith("束") and "bundle" not in columns:
            columns["bundle"] = index
        elif name == "件" and "count" not in columns:
            columns["count"] = index
        elif name == "target" and "target" not in columns:
            columns["target"] = index
    if len(columns) == 4:
        return columns
    return None


def parse_rows(text: str, problems: list[str]) -> tuple[list[Row], int]:
    """束の表の行を集める。戻り値は（行、読んだ表の数）。"""
    lines = text.splitlines()
    rows: list[Row] = []
    tables = 0
    section = "見出し無し"
    index = 0
    while index < len(lines):
        line = lines[index]
        heading = _HEADING.match(line)
        if heading:
            section = strip_notes(heading.group("title")).strip()
        is_table_start = (
            line.strip().startswith("|")
            and index + 1 < len(lines)
            and _SEPARATOR.match(lines[index + 1].strip())
        )
        if not is_table_start:
            index += 1
            continue
        columns = header_columns(split_cells(line))
        index += 2
        if columns is not None:
            tables += 1
        while index < len(lines) and lines[index].strip().startswith("|"):
            if columns is not None:
                row = parse_row(lines[index], index + 1, section, columns, problems)
                if row is not None:
                    rows.append(row)
            index += 1
    return rows, tables


def parse_row(line: str, lineno: int, section: str, columns: dict[str, int], problems: list[str]) -> Row | None:
    cells = split_cells(line)
    if len(cells) <= max(columns.values()):
        problems.append(f"ADR {lineno} 行（{section}）: 列が {len(cells)} 個で、表の見出しより少ない")
        return None
    crate = cells[columns["crate"]].replace("`", "").strip()
    bundle_cell = cells[columns["bundle"]]
    keep = bundle_cell.replace("`", "").strip().startswith("keep")
    bundle: str | None = None
    if not keep:
        name = re.search(r"`([^`]+)`", bundle_cell)
        if name is None:
            problems.append(
                f"ADR {lineno} 行（{section}）: {crate} の束の欄「{bundle_cell}」に backtick の束の名前も keep も無い"
            )
            return None
        bundle = name.group(1).strip().strip("/")
        if bundle.startswith("tests/"):
            bundle = bundle[len("tests/"):]
    targets_cell = strip_notes(cells[columns["target"]]).replace("`", "")
    targets = [t.strip() for t in targets_cell.split("・") if t.strip()]
    return Row(
        line=lineno,
        section=section,
        crate=crate,
        bundle=bundle,
        keep=keep,
        count_text=cells[columns["count"]].strip(),
        targets=targets,
    )


def check_status(text: str, problems: list[str]) -> None:
    found = False
    for lineno, line in enumerate(text.splitlines(), start=1):
        match = _STATUS_LINE.match(line)
        if match is None:
            continue
        found = True
        if DESIGN_STATUS in match.group("body"):
            problems.append(f"ADR {lineno} 行: 状態の行が『{DESIGN_STATUS}』のまま（実装済みの束と合わない）")
    if not found:
        problems.append("ADR に `- 状態:` の行が無い")


def parse_count(row: Row, problems: list[str]) -> int | None:
    try:
        return int(row.count_text)
    except ValueError:
        problems.append(f"{row.where()}: {row.crate} の件の欄「{row.count_text}」が整数でない")
        return None


def fmt(names: set[str] | list[str]) -> str:
    return "・".join(sorted(names))


def compare(root: Path, text: str) -> tuple[list[str], dict[tuple[str, str], Bundle]]:
    problems: list[str] = []
    check_status(text, problems)
    rows, tables = parse_rows(text, problems)
    if tables == 0:
        problems.append("ADR に見出しが crate・束・件・target の表が無い")
    bundles = collect_bundles(root)

    by_key: dict[tuple[str, str], list[Row]] = {}
    for row in rows:
        if row.crate not in CRATES:
            problems.append(f"{row.where()}: crate「{row.crate}」は照合の対象（{'・'.join(CRATES)}）に無い")
            continue
        if row.keep:
            check_keep_row(root, row, problems)
            continue
        assert row.bundle is not None
        by_key.setdefault((row.crate, row.bundle), []).append(row)

    for key, row_list in sorted(by_key.items()):
        if key not in bundles:
            for row in row_list:
                problems.append(
                    f"{row.where()}: {row.crate} `{row.bundle}` は実在しない束"
                    f"（crates/{row.crate}/tests/{row.bundle}/main.rs が無い）"
                )

    for key, bundle in sorted(bundles.items()):
        crate, name = key
        row_list = by_key.get(key, [])
        if not row_list:
            problems.append(
                f"{crate} `{name}`（target {len(bundle.targets)} 件: {fmt(bundle.targets)}）が表に無い束"
            )
            continue
        if len(row_list) > 1:
            lines = "・".join(str(row.line) for row in row_list)
            problems.append(f"{crate} `{name}` が表の {len(row_list)} 行に載る（ADR {lines} 行）")
            continue
        row = row_list[0]
        count = parse_count(row, problems)
        if count is not None and count != len(bundle.targets):
            problems.append(
                f"{row.where()}: {crate} `{name}` の件 {count} が tree の target {len(bundle.targets)} 件と違う"
            )
        listed = set(row.targets)
        actual = set(bundle.targets)
        if listed - actual:
            problems.append(
                f"{row.where()}: {crate} `{name}` の表だけにある target: {fmt(listed - actual)}"
                f"（main.rs の target の mod に無い）"
            )
        if actual - listed:
            problems.append(
                f"{row.where()}: {crate} `{name}` の tree だけにある target: {fmt(actual - listed)}（表の行に無い）"
            )
    return problems, bundles


def check_keep_row(root: Path, row: Row, problems: list[str]) -> None:
    count = parse_count(row, problems)
    if count is not None and count != len(row.targets):
        problems.append(f"{row.where()}: {row.crate} の keep の行の件 {count} が target {len(row.targets)} 件と違う")
    for target in row.targets:
        path = Path("crates") / row.crate / "tests" / f"{target}.rs"
        if not (root / path).is_file():
            problems.append(f"{row.where()}: {row.crate} の keep の target「{target}」の file {path} が無い")


def run_check(adr: Path, root: Path) -> int:
    text = read_text(adr)
    problems, bundles = compare(root, text)
    if problems:
        for problem in problems:
            print(f"食い違い: {problem}")
        print(f"{adr}: 食い違い {len(problems)} 件")
        return 1
    targets = sum(len(bundle.targets) for bundle in bundles.values())
    print(f"{adr}: 一致（{len(CRATES)} crate、束 {len(bundles)} 個、束の target {targets} 件）")
    return 0


# --- self-test -------------------------------------------------------------

_FIXTURE_FILES = {
    "tests/support/http_async.rs": "",
    "crates/task-api/tests/common/mod.rs": "",
    "crates/task-api/tests/kept_api.rs": "",
    "crates/task-api/tests/suite_alpha/main.rs": (
        '#[path = "../common/mod.rs"]\nmod common;\n\nmod one;\nmod two;\n'
    ),
    "crates/task-api/tests/suite_alpha/one.rs": "",
    "crates/task-api/tests/suite_alpha/two.rs": "",
    # browser_eval_surface_suite と同じ形: #[path] の無い fake_http は target。
    "crates/celeris/tests/surface_suite/main.rs": (
        '#[path = "../../../../tests/support/http_async.rs"]\nmod asynchronous;\n\nmod fake_http;\nmod reap;\n'
    ),
    "crates/celeris/tests/surface_suite/fake_http.rs": "",
    "crates/celeris/tests/surface_suite/reap.rs": "",
    # cos_delivery_suite と同じ形: #[path] 付きの fake_http は支援 module。
    "crates/celeris/tests/delivery_suite/main.rs": (
        '//! 束の root。\n'
        '#[path = "../../../../tests/support/http_async.rs"] mod fake_http;\n'
        "mod helpers;\n"
        "// mod commented_out;\n"
        "#[cfg(unix)]\nmod cfg_target;\n"
        "mod audit;\nmod notify;\n"
        "mod inline {\n    mod nested;\n}\n"
    ),
    "crates/celeris/tests/delivery_suite/helpers/mod.rs": "",
    "crates/celeris/tests/delivery_suite/commented_out.rs": "",
    "crates/celeris/tests/delivery_suite/cfg_target.rs": "",
    "crates/celeris/tests/delivery_suite/audit.rs": "",
    "crates/celeris/tests/delivery_suite/notify.rs": "",
    "crates/celeris/tests/delivery_suite/nested.rs": "",
    "crates/celerisctl/tests/ctl_suite/main.rs": "pub mod pipe;\nmod skills;\n",
    "crates/celerisctl/tests/ctl_suite/pipe.rs": "",
    "crates/celerisctl/tests/ctl_suite/skills.rs": "",
    "crates/task-worker/tests/reap_children.rs": "",
    "crates/task-worker/tests/userns_gate/mod.rs": "",
    "crates/task-worker/tests/worker_suite/main.rs": "mod gate_wire;\nmod live_cross;\n",
    "crates/task-worker/tests/worker_suite/gate_wire.rs": "",
    "crates/task-worker/tests/worker_suite/live_cross.rs": "",
}

_ROW_ALPHA = "| task-api | bundle-api-a | `suite_alpha` | 2 | cap-x | common | one・two |"
_ROW_CTL = "| celerisctl | bundle-ctl | `ctl_suite` | 2 | none | common | pipe・skills |"
_STATUS_OK = "- 状態: 採用・実装済み（fixture）"

_FIXTURE_ADR = f"""---
status: accepted
---
# ADR fixture: 束の表

- 日付: 2026-11-12
{_STATUS_OK}

## 束ねの表

### 段 bundle
| crate | leaf | 束（`tests/<bundle>/`） | 件 | capability group | fixture | target |
|---|---|---|---|---|---|---|
{_ROW_ALPHA}
| celeris | bundle-celeris | `surface_suite` | 2 | cap-y | none | fake_http・reap |
| celeris | bundle-celeris | `delivery_suite` | 3 | cap-z | none | audit・cfg_target（`#[cfg]` 付き）・notify |
{_ROW_CTL}

### 段 final
| crate | leaf | 束（案） | 件 | target |
|---|---|---|---|---|
| task-worker | adjust-worker | `worker_suite`（注記の `other_suite` は読まない） | 2 | `gate_wire`・live_cross（fixture `x.py`・`y`） |
| task-worker | adjust-worker | keep（1 件だけの組） | 1 | reap_children（waitpid(-1)） |
| task-api | adjust-api | `keep` | 1 | kept_api |

### 読まない表（target の列が無い）
| crate | 束 | 件 |
|---|---|---|
| celeris | `ghost_suite` | 9 |
"""


def _mutations() -> list[tuple[str, str, str | None, str]]:
    """（名前、ADR の本文、tree から消す file、出力に期待する語）。"""
    adr = _FIXTURE_ADR
    extra_row = "| celerisctl | bundle-ctl | `ghost_suite` | 1 | none | common | ghost |"
    duplicate_row = "| celeris | adjust-celeris | `surface_suite` | 2 | fake_http・reap |"
    final_header = "| crate | leaf | 束（案） | 件 | target |\n|---|---|---|---|---|\n"
    return [
        ("件数", adr.replace("| `suite_alpha` | 2 |", "| `suite_alpha` | 3 |"), None, "の件 3 が tree の target 2 件と違う"),
        ("target", adr.replace("one・two |", "one・three |"), None, "表だけにある target: three"),
        ("状態", adr.replace(_STATUS_OK, f"- 状態: {DESIGN_STATUS}。実装は後の段"), None, "状態の行"),
        ("実在しない束", adr.replace(_ROW_CTL, _ROW_CTL + "\n" + extra_row), None, "`ghost_suite` は実在しない束"),
        ("表に無い束", adr.replace(_ROW_CTL + "\n", ""), None, "`ctl_suite`（target 2 件: pipe・skills）が表に無い束"),
        ("2 行に載る束", adr.replace(final_header, final_header + duplicate_row + "\n"), None, "`surface_suite` が表の 2 行に載る"),
        ("keep の file 欠け", adr, "crates/task-worker/tests/reap_children.rs", "keep の target「reap_children」の file"),
    ]


def _write_tree(root: Path) -> None:
    for rel, content in _FIXTURE_FILES.items():
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")


def _run_child(adr: Path, root: Path) -> tuple[int, str]:
    proc = subprocess.run(
        [sys.executable, str(Path(__file__).resolve()), str(adr), "--root", str(root)],
        capture_output=True,
        text=True,
        check=False,
    )
    return proc.returncode, proc.stdout + proc.stderr


def self_test() -> int:
    base = os.environ.get("TMPDIR") or str(Path(__file__).resolve().parent)
    work = Path(tempfile.mkdtemp(prefix="adr-check-selftest.", dir=base))
    failures = 0
    try:
        tree = work / "tree"
        _write_tree(tree)
        cases: list[tuple[str, Path, Path, int, str]] = []
        adr_ok = work / "adr-ok.md"
        adr_ok.write_text(_FIXTURE_ADR, encoding="utf-8")
        cases.append(("一致", adr_ok, tree, 0, "一致"))
        for index, (name, text, remove, expect) in enumerate(_mutations()):
            adr = work / f"adr-broken-{index}.md"
            adr.write_text(text, encoding="utf-8")
            case_root = tree
            if remove is not None:
                case_root = work / f"tree-broken-{index}"
                shutil.copytree(tree, case_root)
                (case_root / remove).unlink()
            cases.append((name, adr, case_root, 1, expect))
        cases.append(("読めない ADR", work / "missing.md", tree, 2, "を読めない"))
        for name, adr, case_root, want, expect in cases:
            code, output = _run_child(adr, case_root)
            ok = code == want and expect in output
            if not ok:
                failures += 1
            lines = output.strip().splitlines()
            first = next((line for line in lines if expect in line), lines[0] if lines else "（出力なし）")
            print(f"self-test {name}: exit {code}（期待 {want}）{'ok' if ok else 'NG'} — {first}")
    finally:
        shutil.rmtree(work, ignore_errors=True)
    total = len(_mutations()) + 2
    print(f"self-test: {total - failures}/{total} ok")
    return 0 if failures == 0 else 1


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("adr", nargs="?", help="ADR の path")
    parser.add_argument("--root", default=".", help="repo の根（既定は cwd）")
    parser.add_argument("--self-test", action="store_true", help="fixture の tree で台本を確かめる")
    args = parser.parse_args(argv)
    if args.self_test:
        if args.adr is not None:
            parser.error("--self-test は ADR の path を取らない")
        return self_test()
    if args.adr is None:
        parser.error("ADR の path が要る")
    try:
        return run_check(Path(args.adr), Path(args.root))
    except InputError as exc:
        print(f"adr-check: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
