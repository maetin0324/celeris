#!/usr/bin/env python3
"""健全性 v2 R2 の計測台本（ADR 2026-11-10-worker-capability-ports）。

repo の根で実行する。python3 の標準ライブラリだけを使い、crates/ の .rs を読むだけで何も書かない。

呼び方:
  --json                    計測値を JSON で出す（常に exit 0）
  --check-browser-split     Browser の能力の分離（ADR D1・D2）が済んだときだけ exit 0
  --check-adapter-settings  WorkerAdapter の設定の一本化（ADR D3）が済んだときだけ exit 0

check が落ちたときは、外れた値を `FAIL <項目>: <値>` の行で出して exit 1 にする。

Rust の解析は字句の近似である: comment・文字列・文字の literal を空白に置き換えてから、
`trait <名前> {…}`・`impl … for <型> {…}` の括弧の対応で block を取り、block の直下の `fn` を数える。
`#[cfg(test)] mod … {…}` の中と、試験の file（path に `/tests/` を含む、名前が `tests.rs`・`_tests.rs`
で終わる）の impl は「試験」に数える。
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys

ADAPTER_RS = "crates/task-worker/src/adapter.rs"
# ADR D2: Browser の能力の trait（名前、置く file）。既定実装を持たない。
BROWSER_TRAITS = [
    ("BrowserAuthority", "crates/task-worker/src/browser_policy.rs"),
    ("BrowserControl", "crates/task-worker/src/browser_live.rs"),
]
# ADR D1: Browser の能力に触れない sink。BrowserSink は harness に渡す包みで、D2 で転送を消す。
UNRELATED_SINKS = ["ReviewerSink", "ChatRunSink", "PrintSink", "NullSink", "BrowserSink"]
# ADR D3: 置き換える with_* 5 つと、置き換え先の 1 method。
SETTING_FNS = [
    "with_model",
    "with_llm_endpoint",
    "with_permission_mode",
    "with_env",
    "with_container",
]
SETTINGS_METHOD = "configure"
BROWSER_SPECIALIST_RS = "crates/task-worker/src/browser_specialist.rs"
BROWSER_SPECIALIST_TYPE = "BrowserSpecialistAdapter"
WORKER_TASK_RS = "crates/task-dispatch/src/dispatcher/worker_task.rs"
# `.with_env(`・`.with_container(` を LocalWorkspace の builder に使う file（WorkerAdapter ではない）。
NON_ADAPTER_BUILDERS = {
    "crates/task-worker/src/workspace.rs",
    "crates/task-worker/src/task_repos.rs",
}

RAW_STR = re.compile(r'b?r(#*)"')
CHAR_LIT = re.compile(r"'(?:\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F]+\}|.)|[^\\'\n])'")
FN_RE = re.compile(r"\bfn\s+([A-Za-z_]\w*)")
CFG_TEST_MOD = re.compile(
    r"#\[cfg\(test\)\]\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{"
)
IMPL_RE = re.compile(r"\bimpl\b")
IDENT_START = re.compile(r"[A-Za-z_0-9]")


def blank(s: str) -> str:
    return "".join("\n" if ch == "\n" else " " for ch in s)


def strip_code(src: str) -> str:
    """comment・文字列・文字の literal を同じ長さの空白にする（改行は残す）。"""
    out = []
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        if src.startswith("//", i):
            j = src.find("\n", i)
            j = n if j < 0 else j
            out.append(" " * (j - i))
            i = j
            continue
        if src.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if src.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif src.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            out.append(blank(src[i:j]))
            i = j
            continue
        prev_ident = i > 0 and IDENT_START.match(src[i - 1]) is not None
        if not prev_ident:
            m = RAW_STR.match(src, i)
            if m:
                hashes = m.group(1)
                end = src.find('"' + hashes, m.end())
                j = n if end < 0 else end + 1 + len(hashes)
                out.append(blank(src[i:j]))
                i = j
                continue
            if c == '"' or src.startswith('b"', i):
                j = i + (2 if c == "b" else 1)
                while j < n and src[j] != '"':
                    j += 2 if src[j] == "\\" else 1
                j = min(j + 1, n)
                out.append(blank(src[i:j]))
                i = j
                continue
        if c == "'":
            m = CHAR_LIT.match(src, i)
            if m:
                out.append(blank(m.group(0)))
                i = m.end()
                continue
        out.append(c)
        i += 1
    return "".join(out)


def block_end(code: str, open_idx: int) -> int:
    """`code[open_idx] == '{'` の対の `}` の位置。"""
    depth = 0
    for j in range(open_idx, len(code)):
        ch = code[j]
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                return j
    return len(code) - 1


def top_level_fns(body: str) -> list[tuple[str, bool]]:
    """block の中身の直下の `fn` を (名前, 本体を持つか) で返す。"""
    depth_at = []
    depth = 0
    for ch in body:
        depth_at.append(depth)
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
    fns = []
    for m in FN_RE.finditer(body):
        if depth_at[m.start()] != 0:
            continue
        paren = 0
        has_body = False
        for j in range(m.end(), len(body)):
            ch = body[j]
            if ch in "([":
                paren += 1
            elif ch in ")]":
                paren -= 1
            elif paren == 0 and ch in "{;":
                has_body = ch == "{"
                break
        fns.append((m.group(1), has_body))
    return fns


def strip_leading_generics(header: str) -> str:
    h = header.lstrip()
    if not h.startswith("<"):
        return h
    depth = 0
    for j, ch in enumerate(h):
        if ch == "<":
            depth += 1
        elif ch == ">":
            depth -= 1
            if depth == 0:
                return h[j + 1 :]
    return h


def last_ident(path: str) -> str | None:
    path = path.split("<", 1)[0]
    idents = re.findall(r"[A-Za-z_]\w*", path)
    return idents[-1] if idents else None


def is_test_path(path: str) -> bool:
    return (
        "/tests/" in path
        or path.endswith("/tests.rs")
        or path.endswith("_tests.rs")
        or path.endswith("_test.rs")
    )


class RsFile:
    def __init__(self, path: str, text: str):
        self.path = path
        self.code = strip_code(text)
        self.test_file = is_test_path(path)
        self.test_ranges = []
        if not self.test_file:
            for m in CFG_TEST_MOD.finditer(self.code):
                open_idx = m.end() - 1
                self.test_ranges.append((m.start(), block_end(self.code, open_idx)))

    def is_test_at(self, idx: int) -> bool:
        return self.test_file or any(a <= idx <= b for a, b in self.test_ranges)

    def nontest_code(self) -> str:
        if self.test_file:
            return ""
        code = self.code
        for a, b in self.test_ranges:
            code = code[:a] + blank(code[a : b + 1]) + code[b + 1 :]
        return code

    def trait_fns(self, name: str) -> list[tuple[str, bool]] | None:
        m = re.search(r"\btrait\s+" + re.escape(name) + r"\b[^{;]*\{", self.code)
        if not m:
            return None
        open_idx = m.end() - 1
        return top_level_fns(self.code[open_idx + 1 : block_end(self.code, open_idx)])

    def impls(self):
        """(trait 名, 型名, [(fn 名, 本体あり)], 試験か) を返す。trait の無い impl は除く。"""
        for m in IMPL_RE.finditer(self.code):
            j = m.end()
            # header は次の `{` か `;` まで。
            k = j
            while k < len(self.code) and self.code[k] not in "{;":
                k += 1
            if k >= len(self.code) or self.code[k] != "{":
                continue
            header = strip_leading_generics(self.code[j:k])
            hm = re.match(r"(.*?)\bfor\s+([A-Za-z_][\w:]*)", header, re.S)
            if not hm:
                continue
            trait = last_ident(hm.group(1))
            ty = hm.group(2).split("::")[-1]
            if not trait:
                continue
            body = self.code[k + 1 : block_end(self.code, k)]
            yield trait, ty, top_level_fns(body), self.is_test_at(m.start())


def list_rs_files() -> list[str]:
    try:
        out = subprocess.run(
            ["git", "ls-files", "--", "crates/*.rs", "crates/**/*.rs"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout
        files = sorted({p for p in out.splitlines() if p.endswith(".rs")})
        if files:
            return files
    except (OSError, subprocess.CalledProcessError):
        pass
    files = []
    for root, _dirs, names in os.walk("crates"):
        if "/target" in root:
            continue
        files.extend(os.path.join(root, n) for n in names if n.endswith(".rs"))
    return sorted(files)


# 解析する file の目印。どれも含まない file は読み飛ばす（字句の解析が遅いので）。
KEYWORDS = ("EventSink", "WorkerAdapter") + tuple(n for n, _ in BROWSER_TRAITS) + tuple(
    "." + n for n in SETTING_FNS
)


def load(paths: list[str]) -> dict[str, RsFile]:
    files = {}
    fixed = {ADAPTER_RS, BROWSER_SPECIALIST_RS, WORKER_TASK_RS}
    for p in paths:
        try:
            with open(p, encoding="utf-8") as f:
                text = f.read()
        except (OSError, UnicodeDecodeError):
            continue
        if p in fixed or any(k in text for k in KEYWORDS):
            files[p] = RsFile(p, text)
    return files


def measure() -> dict:
    if not os.path.isfile(ADAPTER_RS):
        sys.exit(f"measure.py: {ADAPTER_RS} が無い（repo の根で実行する）")
    files = load(list_rs_files())
    adapter = files[ADAPTER_RS]

    sink_fns = adapter.trait_fns("EventSink") or []
    sink_browser = [n for n, _ in sink_fns if n.startswith("browser_")]

    browser_traits = {}
    trait_names = {name for name, _ in BROWSER_TRAITS}
    for name, expected in BROWSER_TRAITS:
        found_in, fns = None, None
        for path in [expected] + [p for p in files if p != expected]:
            f = files.get(path)
            if f is None or f.test_file:
                continue
            fns = f.trait_fns(name)
            if fns is not None:
                found_in = path
                break
        browser_traits[name] = {
            "expected_file": expected,
            "found_in": found_in,
            "methods": [n for n, _ in fns] if fns else [],
            "default_impls": sum(1 for _, b in fns if b) if fns else 0,
        }

    browser_sinks_nontest = []
    unrelated = {
        s: {"impls": [], "browser_fns": 0, "browser_trait_impls": 0} for s in UNRELATED_SINKS
    }
    adapter_impls_nontest = []
    adapter_impls_test = {"impls": 0, "with_impls": 0, "with_fns": 0}
    specialist_forwards = None
    for path, f in files.items():
        for trait, ty, fns, test in f.impls():
            names = [n for n, _ in fns]
            browser_fns = [n for n in names if n.startswith("browser_")]
            if trait in {"EventSink"} | trait_names:
                if not test and browser_fns:
                    browser_sinks_nontest.append(
                        {"type": ty, "trait": trait, "file": path, "browser_fns": len(browser_fns)}
                    )
                if not test and ty in unrelated:
                    u = unrelated[ty]
                    u["impls"].append(f"{trait}@{path}")
                    u["browser_fns"] += len(browser_fns)
                    if trait in trait_names:
                        u["browser_trait_impls"] += 1
            if trait == "WorkerAdapter":
                with_fns = [n for n in names if n in SETTING_FNS]
                if test:
                    adapter_impls_test["impls"] += 1
                    adapter_impls_test["with_fns"] += len(with_fns)
                    if with_fns:
                        adapter_impls_test["with_impls"] += 1
                else:
                    adapter_impls_nontest.append(
                        {
                            "type": ty,
                            "file": path,
                            "with_fns": len(with_fns),
                            "with": with_fns,
                            "configure": SETTINGS_METHOD in names,
                        }
                    )
                if path == BROWSER_SPECIALIST_RS and ty == BROWSER_SPECIALIST_TYPE:
                    specialist_forwards = sum(
                        1 for n in names if n in SETTING_FNS or n == SETTINGS_METHOD
                    )

    worker_fns = adapter.trait_fns("WorkerAdapter") or []
    worker_task = files.get(WORKER_TASK_RS)
    worker_task_with_env = (
        len(re.findall(r"\.with_env\(", worker_task.nontest_code())) if worker_task else None
    )
    call_re = re.compile(r"\.(" + "|".join(SETTING_FNS) + r")\(")
    setting_calls = {}
    for path, f in files.items():
        if path in NON_ADAPTER_BUILDERS:
            continue
        n = len(call_re.findall(f.nontest_code()))
        if n:
            setting_calls[path] = n

    return {
        "schema": "health-v2-r2-measure/1",
        "event_sink": {
            "file": ADAPTER_RS,
            "methods": [n for n, _ in sink_fns],
            "method_count": len(sink_fns),
            "browser_methods": sink_browser,
            "browser_count": len(sink_browser),
            "default_impls": sum(1 for _, b in sink_fns if b),
        },
        "browser_traits": browser_traits,
        "browser_trait_default_impls": sum(t["default_impls"] for t in browser_traits.values()),
        "browser_sinks_nontest": sorted(browser_sinks_nontest, key=lambda d: (d["file"], d["type"])),
        "unrelated_sinks": unrelated,
        "worker_adapter": {
            "methods": [n for n, _ in worker_fns],
            "with_fns": [n for n, _ in worker_fns if n in SETTING_FNS],
            "with_count": sum(1 for n, _ in worker_fns if n in SETTING_FNS),
            "has_configure": any(n == SETTINGS_METHOD for n, _ in worker_fns),
        },
        "adapter_impls_nontest": sorted(adapter_impls_nontest, key=lambda d: (d["file"], d["type"])),
        "adapter_impls_nontest_with_fns": sum(d["with_fns"] for d in adapter_impls_nontest),
        "adapter_impls_test": adapter_impls_test,
        "browser_specialist_setting_forwards": specialist_forwards,
        "worker_task_with_env_calls": worker_task_with_env,
        "adapter_setting_calls_nontest": dict(sorted(setting_calls.items())),
        "adapter_setting_calls_nontest_total": sum(setting_calls.values()),
    }


def check_browser_split(m: dict) -> list[str]:
    fails = []
    if m["event_sink"]["browser_count"]:
        fails.append(f"event_sink.browser_methods: {m['event_sink']['browser_methods']}")
    for name, t in m["browser_traits"].items():
        if t["found_in"] is None:
            fails.append(f"browser_traits.{name}: trait が無い（ADR D2 は {t['expected_file']}）")
        elif t["default_impls"]:
            fails.append(f"browser_traits.{name}.default_impls: {t['default_impls']}")
    for sink, u in m["unrelated_sinks"].items():
        if sink != "BrowserSink" and not u["impls"]:
            fails.append(f"unrelated_sinks.{sink}: 非試験の impl が見つからない")
        if u["browser_fns"] or u["browser_trait_impls"]:
            fails.append(
                f"unrelated_sinks.{sink}: browser_fns={u['browser_fns']} "
                f"browser_trait_impls={u['browser_trait_impls']} ({', '.join(u['impls'])})"
            )
    return fails


def check_adapter_settings(m: dict) -> list[str]:
    fails = []
    w = m["worker_adapter"]
    if w["with_count"]:
        fails.append(f"worker_adapter.with_fns: {w['with_fns']}")
    with_impls = [d for d in m["adapter_impls_nontest"] if d["with_fns"]]
    if with_impls:
        listed = ", ".join(f"{d['type']}={d['with_fns']}" for d in with_impls)
        fails.append(f"adapter_impls_nontest with_*: {listed}")
    if m["adapter_impls_test"]["with_fns"]:
        t = m["adapter_impls_test"]
        fails.append(f"adapter_impls_test with_*: {t['with_fns']} fn / {t['with_impls']} impl")
    fwd = m["browser_specialist_setting_forwards"]
    if fwd is None:
        fails.append(f"browser_specialist_setting_forwards: {BROWSER_SPECIALIST_TYPE} の impl が無い")
    elif fwd > 1:
        fails.append(f"browser_specialist_setting_forwards: {fwd}")
    if m["worker_task_with_env_calls"] is None:
        fails.append(f"worker_task_with_env_calls: {WORKER_TASK_RS} が無い")
    elif m["worker_task_with_env_calls"]:
        fails.append(f"worker_task_with_env_calls: {m['worker_task_with_env_calls']}")
    return fails


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawTextHelpFormatter)
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--json", action="store_true")
    g.add_argument("--check-browser-split", action="store_true")
    g.add_argument("--check-adapter-settings", action="store_true")
    args = ap.parse_args()
    m = measure()
    if args.json:
        json.dump(m, sys.stdout, ensure_ascii=False, indent=1)
        sys.stdout.write("\n")
        return 0
    fails = check_browser_split(m) if args.check_browser_split else check_adapter_settings(m)
    name = "check-browser-split" if args.check_browser_split else "check-adapter-settings"
    if fails:
        for line in fails:
            print(f"FAIL {line}")
        return 1
    print(f"{name}: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
