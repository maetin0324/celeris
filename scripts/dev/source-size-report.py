#!/usr/bin/env python3
"""Warn about oversized handwritten source and about untracked modules the
build only sees by accident (ADR-0083).

Classifies tracked and non-ignored untracked `.rs`/`.ts`/`.tsx` files as handwritten production,
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
  scripts/dev/source-size-report.py --exclusive-classes [--rev REV] [--files]
      [--format text|json] [--out FILE]

--exclusive-classes assigns every line of every tracked file to exactly one
class (production, external_test, inline_test, generated, vendor, fixture,
evaluation_infra, tooling, docs, schema, migration, lockfile) with language as
a second axis; rules live in [exclusive_classes] of the config. It exits 3 if
the class totals do not add up to the tracked total. It does not touch the
classification the gate (check-source-size.py) freezes.

Exit code is always 0 unless --strict is given and a non-excepted warning
was found (or the tool itself failed to run). This is a warning-only
inventory (ADR-0083). check-source-size.py supplies the mandatory differential gate.

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
}
GENERATED_PATTERNS = [
    re.compile(r"(^|/)generated/"),
]
SCHEMA_PATTERNS = [
    re.compile(r"(^|/)schemas?/"),
    re.compile(r"\.schema\.json$"),
]
MIGRATIONS_PATTERN = re.compile(r"(^|/)migrations/")
TOOLING_DIR = re.compile(r"^(web/scripts/|scripts/|deploy/|tools/)")
LOCKFILES = re.compile(r"(Cargo\.lock|pnpm-lock\.yaml|package-lock\.json|yarn\.lock)$")
FIXTURE = re.compile(r"(^|/)(fixtures?|testdata|mock-celeris)/")
# 外部 crate の写し（[patch.crates-io]）。手書きの code ではないので大きさを測らない
# （ADR agent-docs/adr/2026-11-02-deno-core-vendored-serde-json-features.md）。
VENDORED = re.compile(r"^crates/[^/]+/vendor/")
DOCS = re.compile(r"^docs/|(^|/)docs/|\.md$")


def classify_path(path: str) -> str:
    """Coarse, extension/location-based classification, independent of file
    contents. One of: rust, ts, generated, schema, lockfile, fixture,
    migrations, docs, tooling, vendored, other."""
    if VENDORED.search(path):
        return "vendored"
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
        path.startswith(("web/test/", "web/e2e/", "tests/e2e/"))
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
            # The declaration is test-only too; moving a module must not add
            # production debt merely by leaving its cfg/path declaration behind.
            for k in range(i, j + 1):
                test_mask[k] = True
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
    tracked = sorted(set(f for f in git(root, "ls-files", "--cached", "--others", "--exclude-standard").split("\n") if f))
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
                        "value": info.prod_lines,
                        "threshold": prod_threshold,
                        "kind": "rust_production",
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
                            "value": mod["lines"],
                            "module": mod["name"],
                            "threshold": inline_threshold,
                            "kind": "rust_inline_test",
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
                            "value": n,
                            "threshold": prod_threshold,
                            "kind": "ts_production",
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


# --- exclusive class report (health v2 B0) ---------------------------------
#
# Separate from run_report above: run_report is what check-source-size.py
# freezes in its baseline ("source-size-report/1") and must not change. This
# mode assigns every line of every tracked file to exactly one class, with the
# language as a second axis, so the class totals add up to the tracked total.

EXCLUSIVE_SCHEMA = "celeris.source-size-exclusive/1"
EXCLUSIVE_CLASSES = [
    "production",
    "external_test",
    "inline_test",
    "generated",
    "vendor",
    "fixture",
    "evaluation_infra",
    "tooling",
    "docs",
    "schema",
    "migration",
    "lockfile",
]
LANGUAGES = ["rust", "typescript", "javascript", "python", "shell", "sql", "markdown", "json", "toml", "yaml", "other"]
_LANG_BY_EXT = {
    ".rs": "rust",
    ".ts": "typescript",
    ".tsx": "typescript",
    ".mts": "typescript",
    ".cts": "typescript",
    ".js": "javascript",
    ".mjs": "javascript",
    ".cjs": "javascript",
    ".jsx": "javascript",
    ".py": "python",
    ".sh": "shell",
    ".bash": "shell",
    ".sql": "sql",
    ".md": "markdown",
    ".json": "json",
    ".jsonl": "json",
    ".toml": "toml",
    ".yaml": "yaml",
    ".yml": "yaml",
}
BINARY_SNIFF_BYTES = 8000  # same window git uses to call a blob binary


@dataclass
class ExclusiveRule:
    cls: str
    pattern: re.Pattern
    source: str


@dataclass
class ExclusiveConfig:
    version: str
    rules: list[ExclusiveRule]
    markers: list[str]
    marker_lines: int
    before_markers: set[str]
    views: dict[str, dict]
    sha256: str


def load_exclusive_config(config_path: str | None, root: str) -> ExclusiveConfig:
    import hashlib

    path = config_path or os.path.join(root, DEFAULT_CONFIG_RELPATH)
    if tomllib is None:
        raise RuntimeError("tomllib is unavailable; Python >= 3.11 is required to read the config")
    with open(path, "rb") as f:
        raw = f.read()
    data = tomllib.loads(raw.decode("utf-8")).get("exclusive_classes")
    if not data:
        raise ValueError(f"{path}: no [exclusive_classes] section")
    version = str(data.get("version", "")).strip()
    if not version:
        raise ValueError("[exclusive_classes] needs a non-empty version")
    rules = []
    for entry in data.get("rules", []):
        cls = entry.get("class")
        if cls not in EXCLUSIVE_CLASSES or cls == "inline_test":
            raise ValueError(f"exclusive rule has unknown or non-path class {cls!r}")
        if not entry.get("path"):
            raise ValueError(f"exclusive rule for {cls!r} is missing 'path'")
        rules.append(ExclusiveRule(cls=cls, pattern=re.compile(entry["path"]), source=entry["path"]))
    before = set(data.get("before_markers", []))
    unknown = before - set(EXCLUSIVE_CLASSES)
    if unknown:
        raise ValueError(f"before_markers has unknown classes {sorted(unknown)}")
    views = {}
    for key, view in data.get("views", {}).items():
        classes = list(view.get("classes", []))
        bad = [c for c in classes if c not in EXCLUSIVE_CLASSES]
        if bad or not classes:
            raise ValueError(f"view {key!r} has unknown or no classes: {bad}")
        views[key] = {"label": view.get("label", key), "classes": classes}
    return ExclusiveConfig(
        version=version,
        rules=rules,
        markers=[str(m) for m in data.get("generated_markers", [])],
        marker_lines=int(data.get("generated_marker_lines", 10)),
        before_markers=before,
        views=views,
        sha256=hashlib.sha256(raw).hexdigest(),
    )


def detect_language(path: str, first_line: str) -> str:
    ext = os.path.splitext(path)[1].lower()
    if ext in _LANG_BY_EXT:
        return _LANG_BY_EXT[ext]
    if first_line.startswith("#!"):
        if re.search(r"\b(ba|da|z)?sh\b", first_line):
            return "shell"
        if "python" in first_line:
            return "python"
        if "node" in first_line:
            return "javascript"
    return "other"


def split_lines(text: str) -> list[str]:
    """Lines as `wc -l` sees them, plus an unterminated last line. Only '\\n'
    separates lines (str.splitlines would also split on \\f, \\x1c, U+2028 ...)."""
    if not text:
        return []
    lines = text.split("\n")
    if lines[-1] == "":
        lines.pop()
    return lines


def path_class(cfg: ExclusiveConfig, path: str, head: list[str]) -> tuple[str, str]:
    """(class, reason) for a file before the Rust inline-test split."""
    marker_checked = False

    def marker() -> str | None:
        window = "\n".join(head[: cfg.marker_lines])
        for m in cfg.markers:
            if m in window:
                return m
        return None

    for rule in cfg.rules:
        if not marker_checked and rule.cls not in cfg.before_markers:
            marker_checked = True
            m = marker()
            if m:
                return "generated", f"marker:{m}"
        if rule.pattern.search(path):
            return rule.cls, f"rule:{rule.source}"
    if not marker_checked:
        m = marker()
        if m:
            return "generated", f"marker:{m}"
    return "production", "default"


def classify_file_exclusive(cfg: ExclusiveConfig, path: str, data: bytes | None) -> dict:
    """Per-file record: {path, language, binary, lines, classes: {cls: n}, reason}.
    `data` None means a symlink/submodule entry: counted as a file with 0 lines."""
    if data is None or b"\0" in data[:BINARY_SNIFF_BYTES]:
        cls, reason = path_class(cfg, path, [])
        return {"path": path, "language": detect_language(path, ""), "binary": True,
                "lines": 0, "classes": {cls: 0}, "reason": reason}
    text = data.decode("utf-8", errors="replace")
    lines = split_lines(text)
    lang = detect_language(path, lines[0] if lines else "")
    cls, reason = path_class(cfg, path, lines)
    n = len(lines)
    classes = {cls: n}
    if cls == "production" and lang == "rust":
        info = analyze_rust_file(lines)
        classes = {"production": info.prod_lines}
        if info.inline_test_lines:
            classes["inline_test"] = info.inline_test_lines
    return {"path": path, "language": lang, "binary": False, "lines": n, "classes": classes, "reason": reason}


def _iter_tracked_rev(root: str, rev: str):
    """(path, bytes|None) for every entry of `rev`'s tree, read from the object store."""
    listing = subprocess.run(["git", "-C", root, "ls-tree", "-r", "-z", "--full-tree", rev],
                             capture_output=True, check=True).stdout
    entries = []
    for rec in listing.split(b"\0"):
        if not rec:
            continue
        meta, path = rec.split(b"\t", 1)
        mode, kind, oid = meta.split(b" ")
        entries.append((path.decode("utf-8", errors="surrogateescape"), mode.decode(), kind.decode(), oid.decode()))
    blobs = [e for e in entries if e[2] == "blob" and e[1] != "120000"]
    proc = subprocess.Popen(["git", "-C", root, "cat-file", "--batch"], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    contents: dict[str, bytes] = {}
    try:
        assert proc.stdin and proc.stdout
        for path, _mode, _kind, oid in blobs:
            proc.stdin.write(oid.encode() + b"\n")
            proc.stdin.flush()
            header = proc.stdout.readline().split()
            size = int(header[2])
            contents[path] = proc.stdout.read(size)
            proc.stdout.read(1)  # trailing LF
    finally:
        if proc.stdin:
            proc.stdin.close()
        if proc.stdout:
            proc.stdout.close()
        proc.wait()
    for path, _mode, _kind, _oid in sorted(entries):
        yield path, contents.get(path)


def _iter_tracked_worktree(root: str):
    listing = subprocess.run(["git", "-C", root, "ls-files", "-z", "--cached"], capture_output=True, check=True).stdout
    for raw in sorted(set(p for p in listing.split(b"\0") if p)):
        path = raw.decode("utf-8", errors="surrogateescape")
        full = os.path.join(root, path)
        if os.path.islink(full) or not os.path.isfile(full):
            yield path, None
            continue
        with open(full, "rb") as fh:
            yield path, fh.read()


def run_exclusive_report(root: str, cfg: ExclusiveConfig, rev: str | None = None, with_files: bool = False) -> dict:
    import platform
    import socket

    if rev:
        sha = git(root, "rev-parse", "--verify", rev + "^{commit}").strip()
        source = "rev"
        items = _iter_tracked_rev(root, sha)
        dirty = None
    else:
        sha = git(root, "rev-parse", "HEAD").strip()
        source = "worktree"
        items = _iter_tracked_worktree(root)
        dirty = bool(git(root, "status", "--porcelain", "--untracked-files=no").strip())

    matrix = {c: {l: {"files": 0, "lines": 0} for l in LANGUAGES} for c in EXCLUSIVE_CLASSES}
    tracked_lines = files = binary = 0
    reason_hits: dict[str, int] = {}
    records = []
    for path, data in items:
        rec = classify_file_exclusive(cfg, path, data)
        files += 1
        binary += rec["binary"]
        tracked_lines += rec["lines"]
        reason_hits[rec["reason"]] = reason_hits.get(rec["reason"], 0) + 1
        for cls, n in rec["classes"].items():
            cell = matrix[cls][rec["language"]]
            cell["files"] += 1
            cell["lines"] += n
        if with_files:
            records.append(rec)

    class_totals = {c: sum(v["lines"] for v in matrix[c].values()) for c in EXCLUSIVE_CLASSES}
    lang_totals = {l: sum(matrix[c][l]["lines"] for c in EXCLUSIVE_CLASSES) for l in LANGUAGES}
    classified = sum(class_totals.values())
    views = {}
    for key, view in cfg.views.items():
        views[key] = {
            "label": view["label"],
            "classes": view["classes"],
            "lines": sum(class_totals[c] for c in view["classes"]),
            "rust_lines": sum(matrix[c]["rust"]["lines"] for c in view["classes"]),
        }
    report = {
        "schema": EXCLUSIVE_SCHEMA,
        "classification": cfg.version,
        "gate_classification": "source-size-report/1",
        "config_sha256": cfg.sha256,
        "sha": sha,
        "source": source,
        "worktree_dirty": dirty,
        "environment": {
            "host": socket.gethostname(),
            "python": platform.python_version(),
            "platform": platform.platform(),
            "git": git(root, "--version").strip(),
        },
        "line_rule": "count of '\\n' plus an unterminated last line; binary (NUL in the first 8000 bytes) and symlink entries count 0",
        "classes": EXCLUSIVE_CLASSES,
        "languages": LANGUAGES,
        "matrix": matrix,
        "class_totals": class_totals,
        "language_totals": lang_totals,
        "totals": {
            "files": files,
            "binary_or_symlink_files": binary,
            "tracked_lines": tracked_lines,
            "classified_lines": classified,
            "consistent": classified == tracked_lines and sum(lang_totals.values()) == tracked_lines,
        },
        "views": views,
        "reason_file_counts": dict(sorted(reason_hits.items())),
    }
    if with_files:
        report["files"] = records
    return report


def render_exclusive_text(report: dict) -> str:
    out = [f"source-size-report --exclusive-classes ({report['classification']})",
           f"sha: {report['sha']} ({report['source']})", ""]
    langs = [l for l in report["languages"] if report["language_totals"][l]]
    out.append(f"{'class':<18}" + "".join(f"{l:>12}" for l in langs) + f"{'total':>12}")
    for c in report["classes"]:
        row = report["matrix"][c]
        out.append(f"{c:<18}" + "".join(f"{row[l]['lines']:>12}" for l in langs) + f"{report['class_totals'][c]:>12}")
    out.append(f"{'total':<18}" + "".join(f"{report['language_totals'][l]:>12}" for l in langs)
               + f"{report['totals']['classified_lines']:>12}")
    out.append("")
    for key, v in report["views"].items():
        out.append(f"{key:<28} {v['lines']:>10} lines (rust {v['rust_lines']})  {v['label']}")
    t = report["totals"]
    out.append("")
    out.append(f"tracked lines {t['tracked_lines']}, classified {t['classified_lines']}, consistent={t['consistent']}")
    return "\n".join(out) + "\n"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--root", default=".", help="repository root (default: cwd)")
    parser.add_argument("--config", default=None, help=f"config TOML (default: <root>/{DEFAULT_CONFIG_RELPATH})")
    parser.add_argument("--format", choices=["text", "json"], default="text")
    parser.add_argument("--out", default=None, help="write report to this file instead of stdout")
    parser.add_argument("--strict", action="store_true", help="exit 1 if any non-excepted warning was found")
    parser.add_argument(
        "--exclusive-classes",
        action="store_true",
        help="report every tracked line in exactly one class x language ([exclusive_classes] in the config)",
    )
    parser.add_argument("--rev", default=None, help="with --exclusive-classes: read this commit's tree instead of the worktree")
    parser.add_argument("--files", action="store_true", help="with --exclusive-classes: include per-file records")
    args = parser.parse_args(argv)

    root = os.path.abspath(args.root)
    if args.exclusive_classes:
        try:
            report = run_exclusive_report(root, load_exclusive_config(args.config, root), args.rev, args.files)
        except Exception as exc:  # noqa: BLE001 - surface any failure as a clear non-zero exit
            print(f"source-size-report: error: {exc}", file=sys.stderr)
            return 2
        out = json.dumps(report, ensure_ascii=False, indent=2) if args.format == "json" else render_exclusive_text(report)
        if args.out:
            with open(args.out, "w", encoding="utf-8") as fh:
                fh.write(out if out.endswith("\n") else out + "\n")
        else:
            sys.stdout.write(out if out.endswith("\n") else out + "\n")
        return 0 if report["totals"]["consistent"] else 3
    if args.rev or args.files:
        parser.error("--rev and --files need --exclusive-classes")
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
