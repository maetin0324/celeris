#!/usr/bin/env python3
"""module-deps.json を作る（健全性 v2 B0 capability-map / deps-shared-state）。

読むだけ: `cargo metadata --format-version 1 --no-deps` と crates/ 以下の source の grep。
build・試験は走らせない。

使い方（repo の root で）:
  python3 agent-docs/progress/2026-11-07-health-v2-b0/module-deps-measure.py <base_sha> > \
      agent-docs/progress/2026-11-07-health-v2-b0/module-deps.json

方法の要点（推定の限界は JSON の method に書く）:
- 試験の code は書き手・use から除く（tests/、*_tests.rs、tests.rs、test_support.rs、
  `#[cfg(test)]` の後の部分）。
- module は crate の src/ 直下の第 1 要素（src/store/x.rs → store、lib.rs → lib）。
- SQL の書き手は文字列の中の INSERT [OR …] INTO / REPLACE INTO / UPDATE … SET / DELETE FROM
  の直後の表名で数える。format! で表名を組み立てる SQL は拾えない。
- event の書き手は `Event::<Variant>` を構築する箇所（match の pattern・`matches!`・
  `if let` は除く）。構築した event が実際に append されるかは追わない。
"""

import json
import os
import re
import subprocess
import sys
from collections import defaultdict

ROOT = os.getcwd()
CRATES_DIR = os.path.join(ROOT, "crates")
MAIN_CRATES = ["task-core", "task-dispatch", "task-ops", "task-worker", "task-api", "celeris"]


def rel(p):
    return os.path.relpath(p, ROOT)


def is_test_file(path):
    base = os.path.basename(path)
    parts = path.split(os.sep)
    return (
        "tests" in parts
        or "benches" in parts
        or "examples" in parts
        or base.endswith("_tests.rs")
        or base == "tests.rs"
        or base == "test_support.rs"
        or base.endswith("_fixture.rs")
    )


CFG_TEST = re.compile(r"^\s*#\[cfg\(test\)\]\s*$", re.M)


def strip_tests(text):
    m = CFG_TEST.search(text)
    if not m:
        return text
    # `#[cfg(test)]` の後が mod なら、その後ろは全部試験とみなす（慣習: 末尾に置く）。
    after = text[m.end():].lstrip()
    if after.startswith("mod ") or after.startswith("pub mod ") or after.startswith("pub(crate) mod "):
        return text[: m.start()]
    # mod 以外（fn・use）に付いた cfg(test) は次の項目だけ落とす: 単純化して行ごと残す。
    return text


def crate_of(path):
    r = rel(path).split(os.sep)
    return r[1]


def top_module(path):
    r = rel(path).split(os.sep)
    # crates/<crate>/src/<first>...
    if len(r) < 4 or r[2] != "src":
        return None
    first = r[3]
    if first.endswith(".rs"):
        first = first[:-3]
    return first


def rust_files(crate_dir):
    out = []
    src = os.path.join(crate_dir, "src")
    for dp, dn, fn in os.walk(src):
        for f in fn:
            if f.endswith(".rs"):
                out.append(os.path.join(dp, f))
    return sorted(out)


def all_crate_dirs():
    return sorted(
        os.path.join(CRATES_DIR, d)
        for d in os.listdir(CRATES_DIR)
        if os.path.isfile(os.path.join(CRATES_DIR, d, "Cargo.toml"))
    )


SOURCES = {}  # path -> non-test text
for cd in all_crate_dirs():
    for f in rust_files(cd):
        if is_test_file(f):
            continue
        with open(f, encoding="utf-8", errors="replace") as fh:
            # 行頭の // コメント（doc comment 含む）を落とす。行の途中の // は文字列（URL）と区別できないので残す。
            SOURCES[f] = re.sub(r"(?m)^\s*//[^\n]*$", "", strip_tests(fh.read()))

# ---------------------------------------------------------------- crate_deps
meta = json.loads(
    subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
)
members = {p["name"] for p in meta["packages"]}
crate_dir_of = {p["name"]: os.path.dirname(p["manifest_path"]) for p in meta["packages"]}
lib_name_of = {}
for p in meta["packages"]:
    for t in p["targets"]:
        if "lib" in t["kind"] or "rlib" in t["kind"]:
            lib_name_of[p["name"]] = t["name"].replace("-", "_")
crate_deps = []
seen = set()
for p in meta["packages"]:
    for d in p["dependencies"]:
        if d["name"] not in members and (d.get("rename") or "") not in members:
            continue
        kind = d["kind"] or "normal"
        key = (p["name"], d["name"], kind)
        if key in seen:
            continue
        seen.add(key)
        crate_deps.append({"from": p["name"], "to": d["name"], "kind": kind})
crate_deps.sort(key=lambda x: (x["from"], x["to"], x["kind"]))

# ---------------------------------------------------------------- module_uses
USE_RE = re.compile(r"\buse\s+([^;]+);", re.S)


def expand_use(tree):
    """`a::{b, c::{d}}` → ["a::b", "a::c::d"]."""
    tree = re.sub(r"\s+", "", tree)
    out = []

    def rec(prefix, s):
        i = s.find("{")
        if i < 0:
            out.append(prefix + s)
            return
        head = s[:i]
        body = s[i + 1 : s.rfind("}")]
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

    rec("", tree)
    return out


crate_by_libname = {v: k for k, v in lib_name_of.items()}
module_counts = defaultdict(int)
module_names = {}
for c in MAIN_CRATES:
    names = set()
    for f in rust_files(crate_dir_of[c]):
        m = top_module(f)
        if m:
            names.add(m)
    module_names[c] = names

for c in MAIN_CRATES:
    for f in rust_files(crate_dir_of[c]):
        if f not in SOURCES:
            continue
        frm = top_module(f)
        if frm is None:
            continue
        for m in USE_RE.finditer(SOURCES[f]):
            for path in expand_use(m.group(1)):
                path = path.split("as")[0] if "::" not in path else path
                segs = [s for s in path.split("::") if s]
                if not segs:
                    continue
                if segs[0] == "crate":
                    if len(segs) < 2:
                        continue
                    tgt = segs[1] if segs[1] in module_names[c] else "lib"
                    if tgt == frm:
                        continue
                    module_counts[(c, frm, tgt)] += 1
                elif segs[0] in crate_by_libname and crate_by_libname[segs[0]] != c:
                    other = crate_by_libname[segs[0]]
                    if len(segs) >= 2 and other in module_names and segs[1] in module_names[other]:
                        tgt = f"{other}::{segs[1]}"
                    else:
                        tgt = f"{other}::lib"
                    module_counts[(c, frm, tgt)] += 1

module_uses = [
    {"crate": c, "from_module": a, "to_module": b, "count": n}
    for (c, a, b), n in sorted(module_counts.items())
]

# ---------------------------------------------------------------- cycles (Tarjan)
def sccs(nodes, edges):
    index, low, stack, on, out = {}, {}, [], set(), []
    counter = [0]
    sys.setrecursionlimit(10000)

    def strong(v):
        index[v] = low[v] = counter[0]
        counter[0] += 1
        stack.append(v)
        on.add(v)
        for w in edges.get(v, ()):
            if w not in index:
                strong(w)
                low[v] = min(low[v], low[w])
            elif w in on:
                low[v] = min(low[v], index[w])
        if low[v] == index[v]:
            comp = []
            while True:
                w = stack.pop()
                on.discard(w)
                comp.append(w)
                if w == v:
                    break
            out.append(sorted(comp))

    for v in sorted(nodes):
        if v not in index:
            strong(v)
    return out


cycles = []
for c in MAIN_CRATES:
    edges = defaultdict(set)
    nodes = set()
    for (cc, a, b), _ in module_counts.items():
        if cc != c or "::" in b:
            continue
        edges[a].add(b)
        nodes.update([a, b])
    for comp in sccs(nodes, edges):
        if len(comp) < 2:
            continue
        comp_set = set(comp)
        pairs = sorted(
            [a, b]
            for a in comp
            for b in edges[a]
            if b in comp_set and a < b and a in edges[b]
        )
        cycles.append(
            {
                "level": "module",
                "crate": c,
                "modules": comp,
                "size": len(comp),
                "mutual_pairs": pairs,
                "note": "lib は crate root の再 export（crate::Type）を指す。lib を含む循環は再 export 経由の可能性がある",
            }
        )
# crate level (normal deps only)
cedges = defaultdict(set)
for d in crate_deps:
    if d["kind"] == "normal":
        cedges[d["from"]].add(d["to"])
for comp in sccs(members, cedges):
    if len(comp) > 1:
        cycles.append({"level": "crate", "crates": comp, "size": len(comp)})

# ---------------------------------------------------------------- shared_state
writer_sources = {f: t for f, t in SOURCES.items()}


def writer_entry(f):
    return {"crate": crate_of(f), "module": top_module(f) or "?", "path": rel(f)}


def finalize(kind, name, files, extra=None):
    ws = [writer_entry(f) for f in sorted(files)]
    owners = {(w["crate"], w["module"]) for w in ws}
    crates_ = {w["crate"] for w in ws}
    row = {
        "kind": kind,
        "name": name,
        "writers": ws,
        "writer_crates": sorted(crates_),
        "writer_modules": sorted(f"{a}::{b}" for a, b in owners),
        "multi_writer": len(crates_) > 1 or len(owners) > 1,
    }
    if extra:
        row.update(extra)
    return row


# tables
MIG = os.path.join(crate_dir_of["task-core"], "migrations")
created, dropped, renamed_to = {}, set(), {}
CREATE_RE = re.compile(r"create\s+(?:virtual\s+)?table\s+(?:if\s+not\s+exists\s+)?\"?([a-z_0-9]+)", re.I)
DROP_RE = re.compile(r"drop\s+table\s+(?:if\s+exists\s+)?\"?([a-z_0-9]+)", re.I)
RENAME_RE = re.compile(r"alter\s+table\s+\"?([a-z_0-9]+)\"?\s+rename\s+to\s+\"?([a-z_0-9]+)", re.I)
for fn in sorted(os.listdir(MIG)):
    if not fn.endswith(".sql"):
        continue
    text = open(os.path.join(MIG, fn), encoding="utf-8").read()
    text = re.sub(r"--[^\n]*", "", text)
    # 出現順に処理（同じ file で drop→create があり得る）
    evs = []
    for m in CREATE_RE.finditer(text):
        evs.append((m.start(), "c", m.group(1).lower(), None))
    for m in DROP_RE.finditer(text):
        evs.append((m.start(), "d", m.group(1).lower(), None))
    for m in RENAME_RE.finditer(text):
        evs.append((m.start(), "r", m.group(1).lower(), m.group(2).lower()))
    for _, k, a, b in sorted(evs):
        if k == "c":
            created.setdefault(a, fn)
            dropped.discard(a)
        elif k == "d":
            dropped.add(a)
        else:
            created.setdefault(b, fn)
            dropped.discard(b)
            dropped.add(a)
            renamed_to[a] = b

# migration の trigger（表 X の変化で表 Y を書く）。最後の定義を採る（DROP TRIGGER で消えたものは除く）。
TRIG_RE = re.compile(
    r"create\s+trigger\s+(?:if\s+not\s+exists\s+)?\"?([a-z_0-9]+)\"?(.*?)\bon\s+\"?([a-z_0-9]+)\"?(.*?)\bend\s*;",
    re.I | re.S,
)
DROP_TRIG_RE = re.compile(r"drop\s+trigger\s+(?:if\s+exists\s+)?\"?([a-z_0-9]+)", re.I)
TRIG_WRITE_RE = re.compile(r"(?:insert\s+(?:or\s+\w+\s+)?into|replace\s+into|delete\s+from|update(?:\s+or\s+\w+)?)\s+\"?([a-z_0-9]+)", re.I)
triggers = {}
for fn in sorted(os.listdir(MIG)):
    if not fn.endswith(".sql"):
        continue
    text = re.sub(r"--[^\n]*", "", open(os.path.join(MIG, fn), encoding="utf-8").read())
    evs = [(m.start(), "d", m.group(1).lower(), None) for m in DROP_TRIG_RE.finditer(text)]
    evs += [(m.start(), "c", m.group(1).lower(), m) for m in TRIG_RE.finditer(text)]
    for _, k, name, m in sorted(evs, key=lambda e: e[0]):
        if k == "d":
            triggers.pop(name, None)
        else:
            on = m.group(3).lower()
            targets = {w.group(1).lower() for w in TRIG_WRITE_RE.finditer(m.group(4))}
            triggers[name] = {"trigger": name, "on_table": on, "targets": sorted(targets), "migration": fn}
trigger_writers = defaultdict(list)
for tr in triggers.values():
    for tgt in tr["targets"]:
        trigger_writers[tgt].append({"trigger": tr["trigger"], "on_table": tr["on_table"], "migration": tr["migration"]})

# 表名を format! で組み立てる書き手（grep で拾えない。source を読んで確かめた）。
DYNAMIC_SQL_WRITERS = {
    "run_write_sets": ["crates/task-core/src/store/write_sets.rs"],
    "work_unit_write_sets": ["crates/task-core/src/store/write_sets.rs"],
}

shared_state = []
for t in sorted(created):
    pat = re.compile(
        r"(?:\binsert\s+(?:or\s+\w+\s+)?into|\breplace\s+into|\bdelete\s+from|\bupdate(?:\s+or\s+\w+)?)\s+\"?"
        + re.escape(t)
        + r"\b(?!\.)",
        re.I,
    )
    files = set()
    for f, text in writer_sources.items():
        if t not in text:
            continue
        for m in pat.finditer(text):
            # UPDATE は直後に SET が要る（英文の "update tasks" を除く）
            if m.group(0).lower().lstrip().startswith("update"):
                tail = text[m.end(): m.end() + 80]
                if not re.match(r"[\s\\\"]*(?:AS\s+\w+[\s\\\"]*)?SET\b", tail, re.I):
                    continue
            files.add(f)
            break
    dyn = DYNAMIC_SQL_WRITERS.get(t, [])
    for d in dyn:
        files.add(os.path.join(ROOT, d))
    live = t not in dropped
    extra = {"created_in": created[t], "live": live}
    if dyn:
        extra["dynamic_sql_writers"] = dyn
    if trigger_writers.get(t):
        extra["trigger_writers"] = trigger_writers[t]
    if not live and t in renamed_to:
        extra["renamed_to"] = renamed_to[t]
    shared_state.append(finalize("sqlite_table", t, files, extra))

# event kinds
EV_FILE = os.path.join(crate_dir_of["task-core"], "src", "model", "events.rs")
ev_text = open(EV_FILE, encoding="utf-8").read()
start = ev_text.index("pub enum Event {")
body = ev_text[start:]
depth, end = 0, None
for i, ch in enumerate(body):
    if ch == "{":
        depth += 1
    elif ch == "}":
        depth -= 1
        if depth == 0:
            end = i
            break
body = body[: end]
variants = re.findall(r"^    ([A-Z][A-Za-z0-9]*)\s*[{,(]", body, re.M)


def snake(name):
    return re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower()


def is_pattern(text, s, e):
    """`Event::X ...` が pattern かどうか（構築でないか）。"""
    line_start = text.rfind("\n", 0, s) + 1
    before = text[line_start:s]
    if re.search(r"\b(if|while)\s+let\b|matches!\(|\blet\s*$|\blet\s+[A-Za-z_]+\s*=\s*$", before):
        if "matches!(" in before or re.search(r"\b(if|while)\s+let\s*$", before) or re.search(r"\blet\s*$", before):
            return True
    j = e
    n = len(text)
    while j < n and text[j] in " \t\n":
        j += 1
    if j < n and text[j] in "{(":
        opener = text[j]
        closer = "}" if opener == "{" else ")"
        d, k = 0, j
        while k < n:
            if text[k] == opener:
                d += 1
            elif text[k] == closer:
                d -= 1
                if d == 0:
                    break
            k += 1
        inner = text[j + 1 : k]
        if re.search(r"(^|[,{\s])\.\.\s*$", inner) or re.search(r"\.\.\s*[,}]?\s*$", inner.strip()):
            return True
        j = k + 1
        while j < n and text[j] in " \t\n":
            j += 1
    rest = text[j : j + 4]
    if rest.startswith("=>") or rest.startswith("|") or rest.startswith(" if ") or rest.startswith("if "):
        return True
    if rest.startswith("=") and not rest.startswith("=="):
        return True
    if "matches!(" in before:
        return True
    return False


for v in variants:
    pat = re.compile(r"\bEvent::" + v + r"\b")
    files = set()
    for f, text in writer_sources.items():
        if os.path.samefile(f, EV_FILE):
            continue
        if ("Event::" + v) not in text:
            continue
        for m in pat.finditer(text):
            if not is_pattern(text, m.start(), m.end()):
                files.add(f)
                break
    shared_state.append(finalize("event_kind", snake(v), files, {"variant": v}))

# files（人が source を読んで確かめた書き手。path は下の検査で実在を確かめる）
FILES = json.load(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "module-deps-files.json"), encoding="utf-8"))
for row in FILES:
    for w in row["writers"]:
        assert os.path.exists(os.path.join(ROOT, w["path"])), w["path"]
    crates_ = {w["crate"] for w in row["writers"]}
    owners = {(w["crate"], w["module"]) for w in row["writers"]}
    row = dict(row)
    row["kind"] = "file"
    row["writer_crates"] = sorted(crates_)
    row["writer_modules"] = sorted(f"{a}::{b}" for a, b in owners)
    row["multi_writer"] = len(crates_) > 1 or len(owners) + len(row.get("external_writers", [])) > 1
    shared_state.append(row)

for row in shared_state:
    for w in row["writers"]:
        assert os.path.exists(os.path.join(ROOT, w["path"])), w["path"]

summary = {
    "crate_deps": len(crate_deps),
    "module_uses": len(module_uses),
    "module_cycles": sum(1 for c in cycles if c["level"] == "module"),
    "crate_cycles": sum(1 for c in cycles if c["level"] == "crate"),
    "tables": sum(1 for r in shared_state if r["kind"] == "sqlite_table"),
    "tables_live": sum(1 for r in shared_state if r["kind"] == "sqlite_table" and r["live"]),
    "tables_multi_writer": sum(1 for r in shared_state if r["kind"] == "sqlite_table" and r["multi_writer"]),
    "tables_multi_crate": sum(1 for r in shared_state if r["kind"] == "sqlite_table" and len(r["writer_crates"]) > 1),
    "tables_live_without_writer": sorted(r["name"] for r in shared_state if r["kind"] == "sqlite_table" and r["live"] and not r["writers"] and not r.get("trigger_writers")),
    "tables_written_by_triggers": sorted(r["name"] for r in shared_state if r["kind"] == "sqlite_table" and r.get("trigger_writers")),
    "event_kinds": sum(1 for r in shared_state if r["kind"] == "event_kind"),
    "event_kinds_multi_writer": sum(1 for r in shared_state if r["kind"] == "event_kind" and r["multi_writer"]),
    "event_kinds_multi_crate": sum(1 for r in shared_state if r["kind"] == "event_kind" and len(r["writer_crates"]) > 1),
    "event_kinds_without_writer": sorted(r["name"] for r in shared_state if r["kind"] == "event_kind" and not r["writers"]),
    "files": sum(1 for r in shared_state if r["kind"] == "file"),
    "files_multi_writer": sum(1 for r in shared_state if r["kind"] == "file" and r["multi_writer"]),
}

out = {
    "schema": "celeris.health-v2-b0.module-deps/1",
    "base_sha": sys.argv[1],
    "generator": rel(os.path.abspath(__file__)),
    "method": {
        "crate_deps": "cargo metadata --format-version 1 --no-deps の workspace member 間の依存（kind null → normal）",
        "module_uses": "6 crate の非試験 source の use 文（pub use 含む、{} を展開）を src/ 直下の module 単位で数える。crate::X の X が module でなければ crate root の再 export として lib に寄せる。自 module への use は数えない。他 crate は <crate>::<module>",
        "cycles": "module_uses の crate 内の辺（他 crate 宛を除く）の強連結成分（大きさ 2 以上）と、normal 依存の crate 循環",
        "sqlite_table": "task-core/migrations/*.sql の CREATE TABLE 全部（migration の trigger が書く表は trigger_writers、format! の表名は dynamic_sql_writers に別記）（後で DROP・RENAME されたものは live=false）。書き手は crates/ の非試験 .rs で INSERT [OR x] INTO / REPLACE INTO / UPDATE … SET / DELETE FROM の直後にその表名がある file。format! で表名を組む SQL・migration の SQL file は数えない",
        "event_kind": "task-core model/events.rs の enum Event の全 variant（serde tag type、snake_case）。書き手は Event::<Variant> を構築する非試験 file（match・matches!・if let の pattern を除く。append まで追ってはいない）",
        "file": "daemon と worker（または run の agent）がともに書く file。source を読んで書き手を選んだ（module-deps-files.json）",
        "multi_writer": "書き手が 2 crate 以上、または 2 つ以上の top-level module にまたがる",
        "comments": "行頭が // の行（doc comment を含む）を除いてから数える",
        "test_exclusion": "tests/・*_tests.rs・tests.rs・test_support.rs・*_fixture.rs と、#[cfg(test)] mod の後ろを除く",
    },
    "summary": summary,
    "crate_deps": crate_deps,
    "module_uses": module_uses,
    "cycles": cycles,
    "shared_state": shared_state,
}
json.dump(out, sys.stdout, ensure_ascii=False, indent=1)
sys.stdout.write("\n")
