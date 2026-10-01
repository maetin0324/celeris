#!/usr/bin/env python3
"""Warn about oversized handwritten source and about untracked modules the
build only sees by accident (ADR-0083).

Classifies every tracked `.rs`/`.ts`/`.tsx` file as handwritten production,
inline test (`#[cfg(test)] mod ... { }` embedded in a production file),
external test (a dedicated `tests.rs` / `*_test*.rs` / `*.test.ts` file, or
anything under a `tests/`, `e2e/` or `benches/` directory), or generated
(codegen output, JSON schema, lockfiles, fixtures, DB migrations, docs) and
reports files that cross a size threshold. It also flags `mod x;` in a
tracked file whose target `.rs` file exists on disk but is both untracked
and matched by `.gitignore` — that combination means `cargo build` only
works because of a leftover local file; a fresh clone would fail to
compile.

Usage:
  scripts/dev/source-size-report.py [--root DIR] [--config FILE]
      [--format text|json] [--strict] [--out FILE]

Exit code is always 0 unless --strict is given and a non-excepted warning
was found (or the tool itself failed to run). This is a warning-only
guardrail (ADR-0083); it is not meant to fail a build by itself.

Heuristics rely on rustfmt formatting: a brace-delimited item ends at the
first later line that is exactly `' ' * indent + '}'`. This is the same
approach used by the one-off audit script from the P0 phase
(wu/audit/artifacts/loc_audit.py); this file is the productionized,
tested, configurable version of it.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass, field

try:
    import tomllib
except ImportError:  # pragma: no cover - repo requires Python >= 3.11
    tomllib = None

DEFAULT_CONFIG_RELPATH = "scripts/dev/source-size-report.toml"
DEFAULT_PRODUCTION_THRESHOLD = 2000
DEFAULT_INLINE_TEST_THRESHOLD = 300

GENERATED_EXACT = {
    "gui/app/celeris/types.ts",
}
GENERATED_PATTERNS = [
    re.compile(r"(^|/)generated/"),
]
SCHEMA_PATTERNS = [
    re.compile(r"(^|/)schemas?/"),
    re.compile(r"\.schema\.json$"),
]
MIGRATIONS_PATTERN = re.compile(r"(^|/)migrations/")
TOOLING_DIR = re.compile(r"^(gui/scripts/|web/scripts/|scripts/|deploy/|tools/)")
LOCKFILES = re.compile(r"(Cargo\.lock|pnpm-lock\.yaml|package-lock\.json|yarn\.lock)$")
FIXTURE = re.compile(r"(^|/)(fixtures?|testdata|mock-celeris)/")
DOCS = re.compile(r"^docs/|(^|/)docs/|\.md$")


def classify_path(path: str) -> str:
    """Coarse, extension/location-based classification, independent of file
    contents. One of: rust, ts, generated, schema, lockfile, fixture,
    migrations, docs, tooling, other."""
    if path in GENERATED_EXACT:
        return "generated"
    if LOCKFILES.search(path):
        return "lockfile"
    if MIGRATIONS_PATTERN.search(path):
        return "migrations"
    if FIXTURE.search(path):
        return "fixture"
    if any(p.search(path) for p in GENERATED_PATTERNS):
        return "generated"
    if any(p.search(path) for p in SCHEMA_PATTERNS):
        return "schema"
    if DOCS.search(path):
        return "docs"
    if TOOLING_DIR.search(path) and path.endswith((".mjs", ".js", ".ts", ".py", ".sh")):
        return "tooling"
    if path.endswith(".rs"):
        return "rust"
    if path.endswith((".ts", ".tsx", ".mjs", ".js")):
        return "ts"
    return "other"


def is_external_rust_test(path: str) -> bool:
    parts = path.split("/")
    name = parts[-1]
    return (
        "tests" in parts[:-1]
        or "benches" in parts[:-1]
        or name == "tests.rs"
        or name.endswith("_tests.rs")
        or name.endswith("_test.rs")
    )


def is_ts_test(path: str) -> bool:
    return (
        path.startswith(("gui/test/", "gui/e2e/", "web/test/", "web/e2e/", "tests/e2e/"))
        or re.search(r"\.(test|spec)\.(ts|tsx|mjs|js)$", path) is not None
    )


MOD_DECL_RE = re.compile(r"^(\s*)(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*(\{|;)")
CFG_TEST_RE = re.compile(r"^\s*#\[cfg\((all\()?test")


def rust_string_mask(lines: list[str]) -> list[bool]:
    """mask[i] is True when line i starts inside a string/raw-string/block comment."""
    text = "\n".join(lines) + "\n"
    mask = [False] * (len(lines) + 1)
    i, n, line = 0, len(text), 0
    state = None  # None | 'str' | ('raw', hashes) | ('block', depth)
    while i < n:
        c = text[i]
        if c == "\n":
            line += 1
            mask[line] = state is not None
            i += 1
            continue
        if state is None:
            if text.startswith("//", i):
                j = text.find("\n", i)
                i = n if j < 0 else j
                continue
            if text.startswith("/*", i):
                state = ("block", 1)
                i += 2
                continue
            m = re.match(r'b?r(#*)"', text[i : i + 70])
            if m and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_")):
                state = ("raw", m.group(1))
                i += m.end()
                continue
            if c == '"':
                state = "str"
                i += 1
                continue
            if c == "'":
                m = re.match(r"'(\\.[^']*|[^\\'\n])'", text[i : i + 16])
                i += m.end() if m else 1
                continue
            i += 1
        elif state == "str":
            if c == "\\":
                i += 1 if (i + 1 < n and text[i + 1] == "\n") else 2
                continue
            if c == '"':
                state = None
            i += 1
        elif state[0] == "raw":
            close = '"' + state[1]
            if text.startswith(close, i):
                state = None
                i += len(close)
            else:
                i += 1
        else:  # block comment
            if text.startswith("/*", i):
                state = ("block", state[1] + 1)
                i += 2
            elif text.startswith("*/", i):
                state = None if state[1] == 1 else ("block", state[1] - 1)
                i += 2
            else:
                i += 1
    return mask


def block_end(lines: list[str], start: int, indent: int, str_mask: list[bool]) -> int:
    """First line index >= start whose first non-space char is '}' at column
    `indent`, skipping lines that start inside a string literal."""
    for j in range(start, len(lines)):
        if str_mask[j]:
            continue
        l = lines[j]
        if len(l) > indent and l[indent] == "}" and not l[:indent].strip():
            return j
    return len(lines) - 1


@dataclass
class RustFileInfo:
    prod_lines: int
    inline_test_lines: int
    inline_test_mods: list[dict] = field(default_factory=list)
    external_mod_decls: list[str] = field(default_factory=list)


def analyze_rust_file(lines: list[str]) -> RustFileInfo:
    str_mask = rust_string_mask(lines)
    test_mask = [False] * len(lines)
    inline_test_mods: list[dict] = []
    external_mod_decls: list[str] = []

    for i, l in enumerate(lines):
        m = MOD_DECL_RE.match(l)
        if m and m.group(3) == ";":
            external_mod_decls.append(m.group(2))

    for i, l in enumerate(lines):
        if not CFG_TEST_RE.match(l):
            continue
        j = i + 1
        while j < len(lines) and (lines[j].strip().startswith("#[") or not lines[j].strip()):
            j += 1
        if j >= len(lines):
            continue
        m = MOD_DECL_RE.match(lines[j])
        if m and m.group(3) == "{":
            indent = len(m.group(1))
            end = block_end(lines, j + 1, indent, str_mask)
            for k in range(i, end + 1):
                test_mask[k] = True
            inline_test_mods.append({"name": m.group(2), "start": i + 1, "end": end + 1, "lines": end - i + 1})
        elif m and m.group(3) == ";":
            # `#[cfg(test)] mod x;` — an external test module declared behind cfg(test).
            # Its own file (if any) is analyzed separately; nothing here counts as inline.
            pass
        else:
            # Some other cfg(test)-gated item embedded in production code: a test-only
            # helper fn/const/struct/impl, not a whole `mod`. Mask its extent too, since
            # it is still test-only content living inside the production file.
            indent = len(lines[j]) - len(lines[j].lstrip())
            if lines[j].rstrip().endswith(";"):
                end = j
            elif "{" in lines[j] or not lines[j].rstrip().endswith(";"):
                end = block_end(lines, j + 1, indent, str_mask)
            else:
                end = j
            for k in range(i, end + 1):
                test_mask[k] = True

    n = len(lines)
    inline_test_total = sum(1 for k in range(n) if test_mask[k])
    return RustFileInfo(
        prod_lines=n - inline_test_total,
        inline_test_lines=inline_test_total,
        inline_test_mods=inline_test_mods,
        external_mod_decls=external_mod_decls,
    )


def resolve_mod_candidates(decl_file: str, mod_name: str) -> list[str]:
    """Rust 2018+ path resolution for `mod mod_name;` declared in decl_file."""
    dir_path, base = os.path.split(decl_file)
    stem, _ext = os.path.splitext(base)
    mod_root = dir_path if stem in ("mod", "lib", "main") else os.path.join(dir_path, stem)
    return [
        os.path.join(mod_root, mod_name + ".rs") if mod_root else mod_name + ".rs",
        os.path.join(mod_root, mod_name, "mod.rs") if mod_root else os.path.join(mod_name, "mod.rs"),
    ]


def git(root: str, *args: str) -> str:
    return subprocess.run(
        ["git", "-C", root, *args], capture_output=True, text=True, check=True
    ).stdout


def git_check_ignore(root: str, paths: list[str]) -> set[str]:
    """Return the subset of `paths` (repo-relative) matched by .gitignore.
    `git check-ignore` exits 1 when nothing matched; that is not an error here."""
    if not paths:
        return set()
    proc = subprocess.run(
        ["git", "-C", root, "check-ignore", "--stdin"],
        input="\n".join(paths),
        capture_output=True,
        text=True,
    )
    if proc.returncode not in (0, 1):
        raise RuntimeError(f"git check-ignore failed: {proc.stderr}")
    return {line for line in proc.stdout.splitlines() if line}


@dataclass
class Exception_:
    path: str
    reason: str
    checks: list[str] | None  # None means "all size checks"


def load_config(config_path: str | None, root: str) -> tuple[int, int, list[Exception_]]:
    prod_threshold = DEFAULT_PRODUCTION_THRESHOLD
    inline_threshold = DEFAULT_INLINE_TEST_THRESHOLD
    exceptions: list[Exception_] = []

    path = config_path or os.path.join(root, DEFAULT_CONFIG_RELPATH)
    if not os.path.isfile(path):
        return prod_threshold, inline_threshold, exceptions
    if tomllib is None:
        raise RuntimeError("tomllib is unavailable; Python >= 3.11 is required to read the config")
    with open(path, "rb") as f:
        data = tomllib.load(f)
    thresholds = data.get("thresholds", {})
    prod_threshold = int(thresholds.get("production_lines", prod_threshold))
    inline_threshold = int(thresholds.get("inline_test_lines", inline_threshold))
    for entry in data.get("exceptions", []):
        reason = str(entry.get("reason", "")).strip()
        entry_path = entry.get("path")
        if not entry_path:
            raise ValueError(f"exception entry missing 'path': {entry!r}")
        if not reason:
            raise ValueError(f"exception for {entry_path!r} is missing a non-empty 'reason'")
        checks = entry.get("checks")
        exceptions.append(Exception_(path=entry_path, reason=reason, checks=checks))
    return prod_threshold, inline_threshold, exceptions


CHECK_PRODUCTION_SIZE = "production_size"
CHECK_INLINE_TEST_SIZE = "inline_test_size"
CHECK_UNTRACKED_MOD = "untracked_gitignored_mod"
# `checks` omitted on an [[exceptions]] entry excepts only the size checks, never the
# untracked/gitignored `mod` check — an exception means "this file is fine being big
# for a documented reason," not "this file's mod-target correctness bug is fine."
DEFAULT_EXCEPTED_CHECKS = {CHECK_PRODUCTION_SIZE, CHECK_INLINE_TEST_SIZE}


def excepted(exceptions: list[Exception_], path: str, check: str) -> Exception_ | None:
    for e in exceptions:
        if e.path != path:
            continue
        allowed = DEFAULT_EXCEPTED_CHECKS if e.checks is None else set(e.checks)
        if check in allowed:
            return e
    return None


def run_report(root: str, prod_threshold: int, inline_threshold: int, exceptions: list[Exception_]) -> dict:
    tracked = [f for f in git(root, "ls-files").split("\n") if f]
    tracked_set = set(tracked)
    totals: dict[str, dict[str, int]] = {}

    def bump(kind: str, lines: int) -> None:
        b = totals.setdefault(kind, {"files": 0, "lines": 0})
        b["files"] += 1
        b["lines"] += lines

    files_report = []
    warnings = []
    mod_check_candidates: list[tuple[str, str, str]] = []  # (decl_file, mod_name, candidate_path)

    for path in tracked:
        full = os.path.join(root, path)
        if not os.path.isfile(full):
            continue
        kind = classify_path(path)
        if kind == "other":
            continue
        try:
            with open(full, encoding="utf-8", errors="replace") as fh:
                lines = fh.read().splitlines()
        except OSError:
            continue
        n = len(lines)

        if kind == "rust":
            if is_external_rust_test(path):
                bump("rust_external_test", n)
                files_report.append({"path": path, "kind": "rust_external_test", "lines": n})
                continue
            info = analyze_rust_file(lines)
            bump("rust_production", info.prod_lines)
            if info.inline_test_lines:
                bump("rust_inline_test", info.inline_test_lines)
            files_report.append(
                {
                    "path": path,
                    "kind": "rust_production",
                    "lines": n,
                    "prod_lines": info.prod_lines,
                    "inline_test_lines": info.inline_test_lines,
                }
            )
            if info.prod_lines > prod_threshold:
                exc = excepted(exceptions, path, CHECK_PRODUCTION_SIZE)
                warnings.append(
                    {
                        "check": CHECK_PRODUCTION_SIZE,
                        "path": path,
                        "detail": f"{info.prod_lines} handwritten production lines (threshold {prod_threshold})",
                        "excepted": exc.reason if exc else None,
                    }
                )
            for mod in info.inline_test_mods:
                if mod["lines"] > inline_threshold:
                    exc = excepted(exceptions, path, CHECK_INLINE_TEST_SIZE)
                    warnings.append(
                        {
                            "check": CHECK_INLINE_TEST_SIZE,
                            "path": path,
                            "detail": (
                                f"inline `mod {mod['name']}` at lines {mod['start']}-{mod['end']} "
                                f"is {mod['lines']} lines (threshold {inline_threshold})"
                            ),
                            "excepted": exc.reason if exc else None,
                        }
                    )
            for mod_name in info.external_mod_decls:
                for candidate in resolve_mod_candidates(path, mod_name):
                    mod_check_candidates.append((path, mod_name, candidate))
        elif kind == "ts":
            if is_ts_test(path):
                bump("ts_test", n)
                files_report.append({"path": path, "kind": "ts_test", "lines": n})
            else:
                bump("ts_production", n)
                files_report.append({"path": path, "kind": "ts_production", "lines": n})
                if n > prod_threshold:
                    exc = excepted(exceptions, path, CHECK_PRODUCTION_SIZE)
                    warnings.append(
                        {
                            "check": CHECK_PRODUCTION_SIZE,
                            "path": path,
                            "detail": f"{n} handwritten production lines (threshold {prod_threshold})",
                            "excepted": exc.reason if exc else None,
                        }
                    )
        else:
            bump(kind, n)
            files_report.append({"path": path, "kind": kind, "lines": n})

    # Untracked-and-gitignored mod target check. Only candidates that exist on disk and
    # are not tracked are worth a `git check-ignore` round-trip.
    untracked_existing = sorted(
        {candidate for _decl, _mod, candidate in mod_check_candidates
         if candidate not in tracked_set and os.path.isfile(os.path.join(root, candidate))}
    )
    ignored = git_check_ignore(root, untracked_existing)
    seen_pairs = set()
    for decl_file, mod_name, candidate in mod_check_candidates:
        if candidate not in ignored:
            continue
        key = (decl_file, mod_name)
        if key in seen_pairs:
            continue
        seen_pairs.add(key)
        exc = excepted(exceptions, decl_file, CHECK_UNTRACKED_MOD)
        warnings.append(
            {
                "check": CHECK_UNTRACKED_MOD,
                "path": decl_file,
                "detail": (
                    f"`mod {mod_name};` resolves to {candidate}, which exists on disk but is "
                    "untracked and matched by .gitignore (a fresh clone would not compile this)"
                ),
                "excepted": exc.reason if exc else None,
            }
        )

    active_warnings = [w for w in warnings if not w["excepted"]]
    return {
        "root": os.path.abspath(root),
        "thresholds": {"production_lines": prod_threshold, "inline_test_lines": inline_threshold},
        "totals": totals,
        "warnings": warnings,
        "warning_count": len(active_warnings),
        "excepted_count": len(warnings) - len(active_warnings),
        "files": files_report,
    }


def render_text(report: dict) -> str:
    lines = []
    lines.append("source-size-report (ADR-0083)")
    lines.append(f"root: {report['root']}")
    lines.append(
        "thresholds: production > {production_lines} lines, inline test mod > {inline_test_lines} lines".format(
            **report["thresholds"]
        )
    )
    lines.append("")
    lines.append("totals:")
    for kind, v in sorted(report["totals"].items()):
        lines.append(f"  {kind:<20} {v['files']:>5} files  {v['lines']:>10} lines")
    lines.append("")
    if not report["warnings"]:
        lines.append("no warnings.")
    else:
        for w in report["warnings"]:
            tag = "warning" if not w["excepted"] else "excepted"
            lines.append(f"[{tag}] {w['check']}: {w['path']}")
            lines.append(f"    {w['detail']}")
            if w["excepted"]:
                lines.append(f"    reason: {w['excepted']}")
    lines.append("")
    lines.append(f"{report['warning_count']} active warning(s), {report['excepted_count']} excepted")
    return "\n".join(lines) + "\n"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--root", default=".", help="repository root (default: cwd)")
    parser.add_argument("--config", default=None, help=f"config TOML (default: <root>/{DEFAULT_CONFIG_RELPATH})")
    parser.add_argument("--format", choices=["text", "json"], default="text")
    parser.add_argument("--out", default=None, help="write report to this file instead of stdout")
    parser.add_argument("--strict", action="store_true", help="exit 1 if any non-excepted warning was found")
    args = parser.parse_args(argv)

    root = os.path.abspath(args.root)
    try:
        prod_threshold, inline_threshold, exceptions = load_config(args.config, root)
        report = run_report(root, prod_threshold, inline_threshold, exceptions)
    except Exception as exc:  # noqa: BLE001 - surface any failure as a clear non-zero exit
        print(f"source-size-report: error: {exc}", file=sys.stderr)
        return 2

    out = json.dumps(report, ensure_ascii=False, indent=2) if args.format == "json" else render_text(report)
    if args.out:
        with open(args.out, "w", encoding="utf-8") as fh:
            fh.write(out if out.endswith("\n") else out + "\n")
    else:
        sys.stdout.write(out if out.endswith("\n") else out + "\n")

    if args.strict and report["warning_count"] > 0:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
