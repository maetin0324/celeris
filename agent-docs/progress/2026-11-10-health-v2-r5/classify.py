#!/usr/bin/env python3
"""R5 inventory: classify every Cargo test target and check the bundling leaves.

A test target is a `<member>/tests/<name>.rs` file of a workspace member at
`base_sha` (no member sets `autotests = false` or `[[test]]`; the generator
cross-checks the set with `cargo metadata --no-deps`, kind=test).

Usage (run from the repository root):
  classify.py --write OUT.json [--base REV]
      Build the inventory from the tree at REV (default: git merge-base HEAD main)
      and the R2〜R4 branches as they are now.
  classify.py --check INVENTORY.json
      Exit 0 when every tests/*.rs target of base_sha's tree is listed exactly
      once, every field is present and well typed, the base-only fields
      (support, fixtures, process globals, external references, digits,
      capabilities) equal a re-scan of base_sha, and the leaf rules hold.
  classify.py --verify-leaf LEAF INVENTORY.json
      Exit 0 when every target of LEAF sits at tests/<bundle>/<target>.rs and
      tests/<bundle>/main.rs declares `mod <target>;`, and every keep / r6
      target is still at its original path (working tree). Targets of other
      leaves may be in either state.

Leaf rules (first match wins; the reasons are kept in `leaf_reasons`):
  1. keep  — needs its own binary or cannot move inside the R5 write set:
             a process global operation; a suite named by scripts/ (--test, -E)
             or .config/nextest.toml (binary_id); a digit in the file name
             (B0 R5 write set is tests/[a-z_/]+.rs); a file compiled into another
             target (#[path]) or including one; a crate with no R5 bundle
             leaf; an R2〜R4 conflict in a crate with no adjust leaf.
  2. r6    — named by a table row of docs/architecture-map.md (the paths
             scripts/dev/check-architecture-map.py collects: brace lists
             `a/{b,c}.rs` and the relative paths in `（…）` after a path count)
             or linked from a tracked .md outside this inventory's own progress
             directory (moving it needs a document outside the R5 write set).
             --check re-collects the map and requires r6 for every named
             target that rule 1 does not keep.
  3. adjust-api / adjust-worker — task-api / task-worker targets with an
             R2〜R4 conflict; task-worker targets are bundled only in the final
             stage (R2's write set and interfaces cover them).
  4. bundle-api-a (task-api, name starts with cos_ / chat_ / general_cos /
     project_), bundle-api-b (other task-api), bundle-celeris, bundle-ctl.
  Bundles: same crate, leaf, capability_key and fixture_key; at most
  max_per_bundle targets, chunks balanced. A bundle-* group of one target is
  keep (singleton_bundle_group): no other target shares its capability and
  fixture, so it stays its own binary. adjust-* groups of one stay (the final
  stage regroups them after R2〜R4 land).
"""
from __future__ import annotations

import argparse
import fnmatch
import importlib.util
import json
import os
import posixpath
import re
import subprocess
import sys
import tomllib
from collections import defaultdict

SCHEMA = "celeris.health-v2-r5.inventory/1"
HERE = "agent-docs/progress/2026-11-10-health-v2-r5"
SLICES = "agent-docs/progress/2026-11-07-health-v2-b0/slices.json"
CAPMAP = "agent-docs/progress/2026-11-07-health-v2-b0/capability-map.json"
ARCHMAP = "docs/architecture-map.md"
# The map's own checker owns how a map row names paths; classify.py calls it (no copy of its rules).
ARCHMAP_CHECKER = "scripts/dev/check-architecture-map.py"
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
NEXTEST = ".config/nextest.toml"
R_SLICES = {
    "R2": "01M6TYVJT0Z1CBGNK8VTNK8ZJH",
    "R3": "01M6TYVJT2HG9NY5FW9FMCZS9A",
    "R4": "01M6TYVJT4S29ZMP6P0D4831JG",
}
LEAVES = (
    "bundle-api-a", "bundle-api-b", "bundle-celeris", "bundle-ctl",
    "adjust-api", "adjust-worker", "keep", "r6",
)
BUNDLE_LEAVES = {"bundle-api-a", "bundle-api-b", "bundle-celeris", "bundle-ctl", "adjust-api", "adjust-worker"}
CRATE_LEAF = {"celeris": "bundle-celeris", "celerisctl": "bundle-ctl"}
API_A_PREFIXES = ("cos_", "chat_", "general_cos", "project_")
ADJUST_LEAF = {"task-api": "adjust-api", "task-worker": "adjust-worker"}
MAX_PER_BUNDLE = 8
BUNDLE_RE = re.compile(r"^[a-z_]+$")
# Interfaces R2 changes (its ADR 2026-11-10-worker-capability-ports D1〜D3):
# a target that names them has to change when R2 lands.
R2_TOKENS = re.compile(
    r"\b(EventSink|WorkerAdapter|BrowserPorts|run_with_executable_record|run_with_candidates)\b"
    r"|\.(with_model|with_llm_endpoint|with_permission_mode|with_env|with_container)\("
)

# Process global operations. Each kind is something two tests in one binary
# share: the environment, the working directory, the process itself, static
# state, signal disposition, the umask, the executable, or a fixed address.
PG_PATTERNS = [
    ("env_mutation", re.compile(r"\b(?:env::)?(?:set_var|remove_var)\s*\(")),
    ("set_current_dir", re.compile(r"\bset_current_dir\s*\(")),
    ("process_exit", re.compile(r"\bprocess::exit\s*\(")),
    ("mutable_static", re.compile(
        r"\bstatic\s+mut\s+\w+"
        r"|\bstatic\s+\w+\s*:\s*[^=;]*\b(?:Mutex|RwLock|Cell|RefCell|Atomic[A-Za-z0-9]+)\b")),
    ("signal", re.compile(
        r"\b(?:libc::(?:kill|signal|sigaction|raise|killpg)|nix::sys::signal|signal_hook|tokio::signal"
        r"|SIG(?:STOP|CONT|TERM|KILL|INT|HUP|CHLD|USR1|USR2|PIPE|ALRM))\b")),
    ("umask", re.compile(r"\bumask\s*\(")),
    ("current_exe", re.compile(r"\bcurrent_exe\s*\(")),
    ("fixed_port", re.compile(
        r"(?:127\.0\.0\.1|localhost|0\.0\.0\.0|\[::1\]):(?!0\b)([0-9]{2,5})\b")),
    ("fixed_path", re.compile(r"\"/(?:tmp|var/tmp|dev/shm|run)/[^\"]*\"")),
]
# A fixed address or path is shared only when something binds, connects, listens
# or touches the file system with it; the same literal as request data (a Host
# header, a workspace path in a JSON body, a config view) is not process state.
PG_LINE_CONTEXT = {
    "fixed_port": re.compile(r"\b(?:bind|connect|listen|TcpListener|TcpStream|UdpSocket)\b"),
    "fixed_path": re.compile(
        r"\b(?:fs::|File::|OpenOptions|create_dir|remove_file|remove_dir|UnixListener|UnixStream|bind|connect"
        r"|Path::new|PathBuf::from|const|static)\b"),
}
MOD_RE = re.compile(r"^\s*(?:#\[path\s*=\s*\"([^\"]+)\"\]\s*)?(?:pub(?:\([^)]*\))?\s+)?mod\s+([a-z_][a-z0-9_]*)\s*;", re.M)
PATH_ATTR_LINE = re.compile(r"^\s*#\[path\s*=\s*\"([^\"]+)\"\]\s*$")
INCLUDE_RE = re.compile(r"\binclude(?:_str|_bytes)?!\s*\(\s*\"([^\"]+)\"")
FIXTURE_STR_RE = re.compile(r"\"((?:\.\./)*(?:tests/)?(?:fixtures|testdata)/[A-Za-z0-9_./-]+|tests/[A-Za-z0-9_./-]+)\"")
MD_LINK_RE = re.compile(r"\]\(([^)\s]+)\)")


def git(*args: str, input: str | None = None) -> str:
    return subprocess.run(["git", *args], check=True, capture_output=True, text=True, input=input).stdout


class Tree:
    """Read-only view of one commit's tree."""

    def __init__(self, rev: str):
        self.rev = git("rev-parse", f"{rev}^{{commit}}").strip()
        self.blobs: dict[str, str] = {}
        for line in git("ls-tree", "-r", "--full-tree", self.rev).splitlines():
            meta, path = line.split("\t", 1)
            _mode, kind, sha = meta.split()
            if kind == "blob":
                self.blobs[path] = sha
        self._cache: dict[str, str] = {}

    def has(self, path: str) -> bool:
        return path in self.blobs

    def read(self, path: str) -> str:
        if path not in self._cache:
            self._cache[path] = git("cat-file", "blob", self.blobs[path]) if path in self.blobs else ""
        return self._cache[path]

    def read_many(self, paths: list[str]) -> None:
        todo = [p for p in paths if p in self.blobs and p not in self._cache]
        if not todo:
            return
        out = subprocess.run(["git", "cat-file", "--batch"], check=True, capture_output=True,
                             input="".join(self.blobs[p] + "\n" for p in todo).encode())
        data, pos = out.stdout, 0
        for p in todo:
            nl = data.index(b"\n", pos)
            size = int(data[pos:nl].split()[2])
            self._cache[p] = data[nl + 1: nl + 1 + size].decode("utf-8", "replace")
            pos = nl + 1 + size + 1


def members(tree: Tree) -> list[tuple[str, str]]:
    """(package name, member dir) of every workspace member."""
    ws = tomllib.loads(tree.read("Cargo.toml"))["workspace"]
    out = []
    for m in ws.get("members", []):
        dirs = [m] if "*" not in m else sorted({p.split("/Cargo.toml")[0] for p in tree.blobs
                                                if fnmatch.fnmatch(p, m + "/Cargo.toml")})
        for d in dirs:
            pkg = tomllib.loads(tree.read(f"{d}/Cargo.toml"))
            p = pkg.get("package", {})
            if p.get("autotests") is False or pkg.get("test"):
                raise SystemExit(f"{d}: autotests=false / [[test]] is not handled by this inventory")
            out.append((p["name"], d))
    return out


def test_targets(tree: Tree) -> list[dict]:
    out = []
    for crate, d in members(tree):
        prefix = f"{d}/tests/"
        for path in sorted(tree.blobs):
            if path.startswith(prefix) and path.endswith(".rs") and "/" not in path[len(prefix):]:
                out.append({"crate": crate, "crate_dir": d, "target": path[len(prefix):-3], "path": path})
    return out


def strip_comments(src: str) -> str:
    return "\n".join("" if ln.lstrip().startswith("//") else ln for ln in src.splitlines())


def module_files(tree: Tree, root: str) -> list[str]:
    """Files compiled into the target rooted at `root` (mod declarations, #[path])."""
    seen, out, stack = set(), [], [(root, True)]
    while stack:
        path, is_mod_rs = stack.pop()
        if path in seen or not tree.has(path):
            continue
        seen.add(path)
        out.append(path)
        here = posixpath.dirname(path)
        base = here if is_mod_rs else posixpath.join(here, posixpath.basename(path)[:-3])
        for m in MOD_RE.finditer(strip_comments(tree.read(path))):
            attr, name = m.group(1), m.group(2)
            if attr:
                stack.append((posixpath.normpath(posixpath.join(here, attr)), attr.endswith("mod.rs")))
                continue
            for cand, mrs in ((f"{base}/{name}.rs", False), (f"{base}/{name}/mod.rs", True)):
                if tree.has(cand):
                    stack.append((cand, mrs))
                    break
    return out


def scan_target(tree: Tree, t: dict) -> dict:
    files = module_files(tree, t["path"])
    tests_dir = f"{t['crate_dir']}/tests/"
    support = sorted(f[len(tests_dir):] for f in files[1:] if f.startswith(tests_dir))
    outside = sorted(f for f in files[1:] if not f.startswith(tests_dir))
    fixtures: set[str] = set()
    pg: dict[str, list[str]] = defaultdict(list)
    r2_hits: set[str] = set()
    for f in files:
        src = strip_comments(tree.read(f))
        here = posixpath.dirname(f)
        for inc in INCLUDE_RE.findall(src):
            fixtures.add(posixpath.normpath(posixpath.join(here, inc)))
        for s in FIXTURE_STR_RE.findall(src):
            s = s[3:] if s.startswith("../") else s
            if s.startswith("tests/"):
                fixtures.add(posixpath.normpath(posixpath.join(t["crate_dir"], s)))
            elif s.startswith(("fixtures/", "testdata/")):
                fixtures.add(posixpath.normpath(posixpath.join(tests_dir, s)))
        lines = src.splitlines()
        for kind, rx in PG_PATTERNS:
            ctx = PG_LINE_CONTEXT.get(kind)
            for m in rx.finditer(src):
                line = src.count("\n", 0, m.start()) + 1
                if ctx is None or ctx.search(lines[line - 1]):
                    pg[kind].append(f"{f}:{line}")
        for m in R2_TOKENS.finditer(src):
            r2_hits.add(m.group(1) or m.group(2))
    # Fixture dirs (first two components under tests/) are what a bundle has to share.
    fixture_dirs = sorted({
        "/".join(p[len(tests_dir):].split("/")[:2]) if p.startswith(tests_dir) else p
        for p in fixtures if p not in files
    })
    return {
        "support_modules": support,
        "support_outside_tests_dir": outside,
        "fixtures": sorted(p for p in fixtures if p not in files),
        "fixture_key": "+".join(sorted({s.split("/")[0] for s in support} | set(fixture_dirs))) or "none",
        "process_global": {k: v[:5] for k, v in sorted(pg.items())},
        "_r2_tokens": sorted(r2_hits),
        "_files": files,
    }


def archmap_checker():
    """scripts/dev/check-architecture-map.py loaded as a module (its file name is not importable)."""
    name = "check_architecture_map"
    if name not in sys.modules:
        spec = importlib.util.spec_from_file_location(name, os.path.join(REPO_ROOT, ARCHMAP_CHECKER))
        if spec is None or spec.loader is None:
            raise SystemExit(f"cannot load {ARCHMAP_CHECKER}")
        mod = importlib.util.module_from_spec(spec)
        # @dataclass looks the defining module up in sys.modules while the body runs.
        sys.modules[name] = mod
        try:
            spec.loader.exec_module(mod)
        except BaseException:
            del sys.modules[name]
            raise
    return sys.modules[name]


def archmap_paths(amap: str) -> dict[str, list[int]]:
    """Repo path -> map lines naming it: every table row, collected as check-architecture-map.py does.

    The checker reports wrong_path for each such path that does not exist, so a target named here
    cannot move without changing the map. Relative links are resolved from the map's directory.
    """
    cam = archmap_checker()
    out: dict[str, list[int]] = defaultdict(list)
    for i, ln in enumerate(amap.splitlines(), 1):
        if not cam.TABLE_ROW_RE.match(ln):
            continue
        paths = cam.collect_backtick_paths(ln) | {cam.resolve_link(x, ARCHMAP) for x in cam.collect_md_link_paths(ln)}
        for p in sorted({posixpath.normpath(p) for p in paths}):
            out[p].append(i)
    return out


def external_refs(tree: Tree, targets: list[dict]) -> dict[str, dict]:
    by_path = {t["path"]: t for t in targets}
    names = defaultdict(list)
    for t in targets:
        names[t["target"]].append(t)
    refs = {t["path"]: {"architecture_map": [], "md_links": [], "scripts": [], "nextest": []} for t in targets}
    # docs/architecture-map.md: the rows naming the path (check-architecture-map.py checks they exist).
    for p, lines in archmap_paths(tree.read(ARCHMAP)).items():
        if p in refs:
            refs[p]["architecture_map"] = [f"{ARCHMAP}:{i}" for i in lines]
    # Markdown links from tracked .md files (check-doc-links.sh checks they resolve).
    mds = [p for p in tree.blobs if p.endswith(".md")]
    tree.read_many(mds)
    for md in mds:
        if md.startswith(HERE + "/"):
            continue
        here = posixpath.dirname(md)
        for i, ln in enumerate(tree.read(md).splitlines(), 1):
            for link in MD_LINK_RE.findall(ln):
                if "://" in link or link.startswith("#"):
                    continue
                dest = posixpath.normpath(posixpath.join(here, link.split("#", 1)[0]))
                if dest in by_path:
                    refs[dest]["md_links"].append(f"{md}:{i}")
    # scripts/: `--test <name>` (crate from `-p` on the same line when present) and -E filters.
    scripts = [p for p in tree.blobs if p.startswith("scripts/")]
    tree.read_many(scripts)
    for sp in scripts:
        for i, ln in enumerate(tree.read(sp).splitlines(), 1):
            pkg = re.search(r"(?:-p|--package)[= ]+([A-Za-z0-9_-]+)", ln)
            for name in re.findall(r"--test[= ]+([A-Za-z0-9_]+)", ln):
                for t in names.get(name, []):
                    if pkg is None or pkg.group(1) == t["crate"]:
                        refs[t["path"]]["scripts"].append(f"{sp}:{i}")
            for expr in re.findall(r"-E\s+['\"]([^'\"]+)['\"]", ln):
                for t in filter_hits(expr, targets):
                    refs[t["path"]]["scripts"].append(f"{sp}:{i}")
    for i, ln in enumerate(tree.read(NEXTEST).splitlines(), 1):
        for t in filter_hits(ln, targets):
            refs[t["path"]]["nextest"].append(f"{NEXTEST}:{i}")
    return refs


def filter_hits(expr: str, targets: list[dict]) -> list[dict]:
    """Targets a nextest filter names exactly (binary(name) / binary_id(crate::name)); globs do not count."""
    hits = []
    for kind, arg in re.findall(r"\b(binary_id|binary)\(([^)]+)\)", expr):
        arg = arg.strip()
        arg = arg.lstrip("=")
        if any(c in arg for c in "*?[/~"):
            continue
        for t in targets:
            if (kind == "binary_id" and arg == f"{t['crate']}::{t['target']}") or (kind == "binary" and arg == t["target"]):
                hits.append(t)
    return hits


GENERIC_TOKENS = {"store", "dispatcher", "lib", "main", "server", "api", "admin", "common", "mod", "tests",
                  "e2e", "bin", "src", "config", "handlers", "commands", "rs", "the", "and", "run", "runs"}


def capabilities(tree: Tree) -> tuple[dict[str, list[dict]], dict[str, dict]]:
    """(B0 capability-map tests path -> capabilities, capability id -> {group, tokens})."""
    cmap = json.loads(tree.read(CAPMAP))
    out: dict[str, list[dict]] = defaultdict(list)
    index = {}
    for c in cmap["capabilities"]:
        toks = set(c["id"].split("-"))
        for o in c.get("owners", []):
            toks |= set(re.split(r"[_:/ .-]+", o.get("module", "")))
        index[c["id"]] = {"group": (c.get("groups") or ["none"])[0], "tokens": toks - GENERIC_TOKENS - {""}}
        for p in c.get("tests", []):
            out[p].append({
                "id": c["id"],
                "group": index[c["id"]]["group"],
                "invariants": [{"kind": i["kind"], "text": i["text"]} for i in c.get("invariants", [])],
            })
    return out, index


def capability_key(target: str, caps: list[dict], index: dict[str, dict]) -> tuple[str, str]:
    """(capability_key, capability_source). The key is the B0 capability group (cap-*)."""
    if caps:
        c = sorted(caps, key=lambda c: c["id"])[0]
        return f"group:{c['group']}", f"capability-map:{c['id']}"
    toks = [x for x in target.split("_") if x not in GENERIC_TOKENS]
    best, score = None, 0
    for cid in sorted(index):
        sc = sum((2 if i == 0 else 1) for i, x in enumerate(toks) if x in index[cid]["tokens"])
        if sc > score:
            best, score = cid, sc
    if best is None:
        return "group:none", "none"
    return f"group:{index[best]['group']}", f"name_token:{best}"


def base_fields(tree: Tree, targets: list[dict]) -> dict[str, dict]:
    """Fields that depend only on base_sha's tree (re-computed by --check)."""
    tree.read_many([p for p in tree.blobs if "/tests/" in p and p.endswith(".rs")])
    refs = external_refs(tree, targets)
    caps, cindex = capabilities(tree)
    out = {}
    paths = {t["path"] for t in targets}
    scans = {t["path"]: scan_target(tree, t) for t in targets}
    included_by = defaultdict(list)
    for p, s in scans.items():
        for f in s["_files"][1:]:
            if f in paths:
                included_by[f].append(p)
    for t in targets:
        s = scans[t["path"]]
        out[t["path"]] = {
            "crate": t["crate"],
            "target": t["target"],
            "support_modules": s["support_modules"],
            "support_outside_tests_dir": s["support_outside_tests_dir"],
            "fixtures": s["fixtures"],
            "fixture_key": s["fixture_key"],
            "process_global": s["process_global"],
            "external_refs": refs[t["path"]],
            "digit_in_name": bool(re.search(r"[0-9]", t["target"])),
            # Another target's file compiled in as a module (#[path]) or this file compiled into another target.
            "shares_source_with": sorted({f for f in s["_files"][1:] if f in paths} | set(included_by[t["path"]])),
            "capabilities": caps.get(t["path"], []),
            "capability_key": capability_key(t["target"], caps.get(t["path"], []), cindex)[0],
            "capability_source": capability_key(t["target"], caps.get(t["path"], []), cindex)[1],
            "_r2_tokens": s["_r2_tokens"],
            "_files": s["_files"],
        }
    return out


def branch_changes(base_main: str) -> dict:
    """Changed files of every R2〜R4 branch (main...branch). Superseded WU branches are listed, not counted."""
    out = {}
    for key, tid in R_SLICES.items():
        refs = git("for-each-ref", "--format=%(refname:short)",
                   f"refs/heads/celeris/{tid}", f"refs/heads/celeris-wu/{tid}/*").split()
        for ref in sorted(refs):
            files = git("diff", "--name-only", f"{base_main}...{ref}").split()
            out[ref] = {"slice": key, "superseded": "-superseded-" in ref,
                        "head": git("rev-parse", ref).strip(), "files": files}
    return out


def planned_paths(branches: dict, targets: list[dict]) -> dict[str, list[str]]:
    """Test targets an R slice's ADR (changed on its branches) names as files it will change."""
    out: dict[str, list[str]] = defaultdict(list)
    for ref, b in branches.items():
        if b["superseded"]:
            continue
        for f in b["files"]:
            if not (f.startswith("agent-docs/adr/") and f.endswith(".md")):
                continue
            try:
                text = git("show", f"{ref}:{f}")
            except subprocess.CalledProcessError:
                continue
            for i, ln in enumerate(text.splitlines(), 1):
                for t in targets:
                    rel = f"tests/{t['target']}.rs"
                    if t["path"] in ln or (f"`{rel}`" in ln and t["crate"] in ln):
                        out[t["path"]].append(f"{b['slice']}:{ref}:{f}:{i}")
    return out


def conflicts(t: dict, fields: dict, slices: dict, branches: dict, planned: dict) -> dict:
    files = fields["_files"]
    res = {}
    for key in R_SLICES:
        ws = [re.compile(r) for r in slices[key]["write_set"]]
        ws_hit = sorted(f for f in files if any(r.search(f) for r in ws))
        actual = sorted({f"{ref}:{f}" for ref, b in branches.items() if b["slice"] == key and not b["superseded"]
                         for f in b["files"] if f in files})
        superseded = sorted({f"{ref}:{f}" for ref, b in branches.items() if b["slice"] == key and b["superseded"]
                             for f in b["files"] if f in files})
        plan = sorted(p for p in planned.get(t["path"], []) if p.startswith(key + ":"))
        tokens = fields["_r2_tokens"] if key == "R2" and ws_hit else []
        res[key] = {"write_set": ws_hit, "branch_changes": actual, "planned_by_adr": plan,
                    "interface_tokens": tokens, "superseded_branch_changes": superseded,
                    "conflict": bool(actual or plan or tokens)}
    return res


def keep_reasons(rec: dict) -> list[str]:
    """Rule 1 (keep) reasons of a record; empty when rule 1 does not apply."""
    reasons = []
    pg = rec["process_global"]
    refs = rec["external_refs"]
    conflict = [k for k, v in rec["conflicts"].items() if isinstance(v, dict) and v.get("conflict")]
    if pg:
        reasons.append("process_global:" + ",".join(sorted(pg)))
    if refs["scripts"] or refs["nextest"]:
        reasons.append("named_suite")
    if rec["digit_in_name"]:
        reasons.append("digit_in_name_outside_write_set")
    if rec["shares_source_with"]:
        reasons.append("shares_source_with_target")
    if rec["crate"] not in CRATE_LEAF and rec["crate"] not in ADJUST_LEAF:
        reasons.append("crate_without_r5_bundle_leaf")
    elif conflict and rec["crate"] not in ADJUST_LEAF:
        reasons.append("conflict_without_adjust_leaf:" + ",".join(conflict))
    return reasons


def assign(rec: dict) -> tuple[str, list[str]]:
    if reasons := keep_reasons(rec):
        return "keep", reasons
    refs = rec["external_refs"]
    conflict = [k for k, v in rec["conflicts"].items() if v["conflict"]]
    if refs["architecture_map"] or refs["md_links"]:
        return "r6", ["architecture_map" if refs["architecture_map"] else "md_link"]
    if rec["crate"] in ADJUST_LEAF and (conflict or rec["crate"] == "task-worker"):
        return ADJUST_LEAF[rec["crate"]], (["conflict:" + ",".join(conflict)] if conflict
                                           else ["task_worker_bundled_after_r2"])
    if rec["crate"] == "task-api":
        a = rec["target"].startswith(API_A_PREFIXES)
        return ("bundle-api-a" if a else "bundle-api-b"), ["task_api_prefix_a" if a else "task_api_other"]
    return CRATE_LEAF[rec["crate"]], ["crate_bundle"]


def make_bundles(records: list[dict], crate_names: dict[str, set[str]], crate_dirs: dict[str, set[str]]) -> None:
    groups = defaultdict(list)
    used: dict[str, set[str]] = defaultdict(set)
    for r in records:
        if r["leaf"] in BUNDLE_LEAVES:
            key = (r["crate"], r["leaf"], r["capability_key"], r["fixture_key"])
            groups[key].append(r)
    for (crate, leaf, ckey, fkey), rs in sorted(groups.items()):
        rs.sort(key=lambda r: r["target"])
        if len(rs) == 1 and leaf.startswith("bundle-"):
            # Nothing shares its capability group and fixture: it stays its own binary.
            rs[0]["leaf"], rs[0]["leaf_reasons"] = "keep", [f"singleton_bundle_group:{leaf}"]
            continue
        group = ckey.split(":", 1)[1].removeprefix("cap-").replace("-", "_")
        if group == "none":
            group = "unmapped"
        extra = [x for x in fkey.split("+") if x not in ("none", "common", "support")]
        stem = group + "".join("_" + re.sub(r"[^a-z_]", "_", x) for x in extra)
        n_chunks = -(-len(rs) // MAX_PER_BUNDLE)
        size = -(-len(rs) // n_chunks)  # balanced: 18 -> 6+6+6, not 8+8+2
        chunks = [rs[i:i + size] for i in range(0, len(rs), size)]
        base = re.sub(r"_+", "_", f"{stem}_suite")
        for chunk in chunks:
            # Unique per crate across leaves; never an existing target or tests/ dir name.
            taken = used[crate] | crate_names[crate] | crate_dirs[crate]
            name = base if len(chunks) == 1 and base not in taken else next(
                f"{base}_{ch}" for ch in "abcdefghijklmnopqrstuvwxyz" if f"{base}_{ch}" not in taken)
            used[crate].add(name)
            for r in chunk:
                r["bundle"] = name


def build(base: str) -> dict:
    tree = Tree(base)
    targets = test_targets(tree)
    meta = json.loads(subprocess.run(["cargo", "metadata", "--no-deps", "--format-version", "1"],
                                     check=True, capture_output=True, text=True).stdout)
    root = meta["workspace_root"]
    meta_set = {os.path.relpath(t["src_path"], root) for p in meta["packages"]
                for t in p["targets"] if "test" in t["kind"]}
    ls_set = {t["path"] for t in targets}
    if meta_set != ls_set:
        raise SystemExit(f"cargo metadata and ls-tree disagree: {sorted(meta_set ^ ls_set)[:10]}")
    fields = base_fields(tree, targets)
    slices = {s["key"]: s for s in json.loads(tree.read(SLICES))["slices"]}
    main = git("rev-parse", "main").strip()
    branches = branch_changes(main)
    planned = planned_paths(branches, targets)
    crate_names = defaultdict(set)
    crate_dirs = defaultdict(set)
    for t in targets:
        crate_names[t["crate"]].add(t["target"])
    for p in tree.blobs:
        for t in targets:
            pre = f"{t['crate_dir']}/tests/"
            if p.startswith(pre) and "/" in p[len(pre):]:
                crate_dirs[t["crate"]].add(p[len(pre):].split("/")[0])
    records = []
    for t in targets:
        f = fields[t["path"]]
        rec = {"path": t["path"], **{k: v for k, v in f.items() if not k.startswith("_")}}
        rec["conflicts"] = conflicts(t, f, slices, branches, planned)
        rec["leaf"], rec["leaf_reasons"] = assign(rec)
        rec["bundle"] = None
        records.append(rec)
    make_bundles(records, crate_names, crate_dirs)
    summary = defaultdict(lambda: defaultdict(int))
    for r in records:
        summary[r["crate"]][r["leaf"]] += 1
    return {
        "schema": SCHEMA,
        "base_sha": tree.rev,
        "base_rule": "git merge-base HEAD main at generation",
        "generator": f"python3 {HERE}/classify.py --write {HERE}/inventory.json",
        "main_at_generation": main,
        "rules": {
            "leaf_order": __doc__.split("Leaf rules")[1].strip(),
            "max_per_bundle": MAX_PER_BUNDLE,
            "bundle_key": "crate + leaf + capability_key + fixture_key",
            "capability_key": "group:<B0 capability group (cap-*)> of the target's capability: the capability-map "
                              "tests entry when the path is listed there, otherwise the capability whose id / owner "
                              "module tokens best match the file name tokens (first token counts double; "
                              "capability_source says which); group:none when nothing matches",
            "conflict": "an R2〜R4 branch (celeris/<id>, celeris-wu/<id>/*) changed a file of the target "
                        "(main...branch), or the slice ADR on its branch names the target, or (R2) the target "
                        "names an interface R2 changes; write_set matches are recorded but are not a conflict by "
                        "themselves (R3's write set covers every crates/*/tests file). Branches named "
                        "*-superseded-* are retired WU branches: listed, not counted.",
            "verify_leaf": "tests/<bundle>/<target>.rs + `mod <target>;` in tests/<bundle>/main.rs; keep/r6 stay put",
        },
        "branches": {ref: {k: (v if k != "files" else len(v)) for k, v in b.items()} for ref, b in branches.items()},
        "summary": {c: dict(sorted(v.items())) for c, v in sorted(summary.items())},
        "targets": records,
    }


REQUIRED = {
    "path": str, "crate": str, "target": str, "support_modules": list, "support_outside_tests_dir": list,
    "fixtures": list, "fixture_key": str, "process_global": dict, "external_refs": dict,
    "digit_in_name": bool, "capabilities": list, "conflicts": dict, "capability_key": str,
    "capability_source": str, "shares_source_with": list,
    "leaf": str, "leaf_reasons": list,
}


def map_rule_errors(records: list[dict], named: dict[str, list[int]]) -> list[str]:
    """Rule 2 against the map itself: a target the map names is r6 unless rule 1 keeps it."""
    errs = []
    for r in records:
        lines = named.get(r.get("path"))
        if not lines:
            continue
        want = "keep" if keep_reasons(r) else "r6"
        if r.get("leaf") != want:
            errs.append(f"{r['path']}: named by {ARCHMAP}:{','.join(map(str, lines))} but leaf {r.get('leaf')} "
                        f"(want {want}; reasons {r.get('leaf_reasons')})")
    return errs


def check(inv_path: str) -> int:
    inv = json.load(open(inv_path, encoding="utf-8"))
    errs: list[str] = []
    if inv.get("schema") != SCHEMA:
        errs.append(f"schema != {SCHEMA}")
    tree = Tree(inv["base_sha"])
    targets = test_targets(tree)
    want = [t["path"] for t in targets]
    have = [r.get("path") for r in inv.get("targets", [])]
    dup = sorted({p for p in have if have.count(p) > 1})
    if dup:
        errs.append(f"listed more than once: {dup}")
    if missing := sorted(set(want) - set(have)):
        errs.append(f"missing targets: {missing}")
    if extra := sorted(set(have) - set(want)):
        errs.append(f"not a test target at base_sha: {extra}")
    fresh = base_fields(tree, targets)
    bundles = defaultdict(list)
    for r in inv.get("targets", []):
        p = r.get("path")
        for k, ty in REQUIRED.items():
            if not isinstance(r.get(k), ty):
                errs.append(f"{p}: field {k} missing or not {ty.__name__}")
        if "bundle" not in r:
            errs.append(f"{p}: field bundle missing")
        if p not in fresh or any(not isinstance(r.get(k), ty) for k, ty in REQUIRED.items()):
            continue
        f = fresh[p]
        for k in ("crate", "target", "support_modules", "support_outside_tests_dir", "fixtures", "fixture_key",
                  "process_global", "external_refs", "digit_in_name", "shares_source_with", "capabilities",
                  "capability_key", "capability_source"):
            if r[k] != f[k]:
                errs.append(f"{p}: {k} differs from a re-scan of base_sha")
        for key in R_SLICES:
            c = r["conflicts"].get(key)
            if not isinstance(c, dict) or not isinstance(c.get("conflict"), bool):
                errs.append(f"{p}: conflicts.{key}.conflict missing")
        leaf = r["leaf"]
        if leaf not in LEAVES:
            errs.append(f"{p}: leaf {leaf!r} not in {LEAVES}")
            continue
        if not r["leaf_reasons"]:
            errs.append(f"{p}: leaf_reasons is empty")
        refs = r["external_refs"]
        conflict = any(isinstance(v, dict) and v.get("conflict") for v in r["conflicts"].values())
        named = refs["scripts"] or refs["nextest"]
        if (r["process_global"] or named or r["digit_in_name"] or r["shares_source_with"]) and leaf != "keep":
            errs.append(f"{p}: process global / named suite / digit in name but leaf {leaf}")
        if (refs["architecture_map"] or refs["md_links"]) and leaf not in ("keep", "r6"):
            errs.append(f"{p}: referenced by architecture map / .md link but leaf {leaf}")
        if conflict and leaf not in ("keep", "r6", ADJUST_LEAF.get(r["crate"], "keep")):
            errs.append(f"{p}: R2〜R4 conflict but leaf {leaf}")
        if leaf in BUNDLE_LEAVES:
            if not (isinstance(r["bundle"], str) and BUNDLE_RE.match(r["bundle"])):
                errs.append(f"{p}: bundle {r['bundle']!r} is not [a-z_]+")
            else:
                bundles[(r["crate"], r["bundle"])].append(r)
            if leaf.startswith("bundle-api") or leaf == "adjust-api":
                if r["crate"] != "task-api":
                    errs.append(f"{p}: {leaf} is for task-api")
            if leaf in ("bundle-api-a", "bundle-api-b"):
                a = r["target"].startswith(API_A_PREFIXES)
                if (leaf == "bundle-api-a") != a:
                    errs.append(f"{p}: task-api prefix rule puts it in {'bundle-api-a' if a else 'bundle-api-b'}")
            if leaf == "bundle-celeris" and r["crate"] != "celeris":
                errs.append(f"{p}: bundle-celeris is for celeris")
            if leaf == "bundle-ctl" and r["crate"] != "celerisctl":
                errs.append(f"{p}: bundle-ctl is for celerisctl")
            if leaf == "adjust-worker" and r["crate"] != "task-worker":
                errs.append(f"{p}: adjust-worker is for task-worker")
        elif r["bundle"] is not None:
            errs.append(f"{p}: {leaf} target must not have a bundle")
    well_formed = [r for r in inv.get("targets", []) if r.get("path") in fresh and r.get("leaf") in LEAVES
                   and all(isinstance(r.get(k), ty) for k, ty in REQUIRED.items())]
    errs += map_rule_errors(well_formed, archmap_paths(tree.read(ARCHMAP)))
    names = defaultdict(set)
    for t in targets:
        names[t["crate"]].add(t["target"])
    cap = inv.get("rules", {}).get("max_per_bundle", MAX_PER_BUNDLE)
    for (crate, b), rs in sorted(bundles.items()):
        if len(rs) > cap:
            errs.append(f"{crate}/{b}: {len(rs)} targets > max_per_bundle {cap}")
        if b in names[crate]:
            errs.append(f"{crate}/{b}: bundle name equals an existing target")
        if tree.has(f"{posixpath.dirname(rs[0]['path'])}/{b}/main.rs") or any(
                f.startswith(f"{posixpath.dirname(rs[0]['path'])}/{b}/") for f in tree.blobs):
            errs.append(f"{crate}/{b}: bundle name equals an existing tests/ directory")
        for k in ("leaf", "capability_key", "fixture_key"):
            if len({r[k] for r in rs}) != 1:
                errs.append(f"{crate}/{b}: targets differ in {k}")
    if len(bundles) == 1:
        errs.append("all bundled targets are in one bundle")
    for e in errs:
        print(f"ERROR {e}", file=sys.stderr)
    n = len(inv.get("targets", []))
    print(f"check: {n} targets (base_sha {inv['base_sha'][:12]}: {len(want)}), {len(bundles)} bundles, "
          f"{len(errs)} error(s)")
    return 1 if errs else 0


def verify_leaf(leaf: str, inv_path: str) -> int:
    if leaf not in LEAVES:
        print(f"ERROR unknown leaf {leaf!r}; one of {LEAVES}", file=sys.stderr)
        return 2
    inv = json.load(open(inv_path, encoding="utf-8"))
    top = git("rev-parse", "--show-toplevel").strip()
    errs, ok, stay = [], 0, 0
    mod_cache: dict[str, str] = {}
    for r in inv["targets"]:
        orig = os.path.join(top, r["path"])
        tests_dir = posixpath.dirname(r["path"])
        if r["leaf"] in ("keep", "r6"):
            stay += 1
            if not os.path.isfile(orig):
                errs.append(f"{r['path']}: {r['leaf']} target moved or removed")
            continue
        if r["leaf"] != leaf:
            continue
        dest = posixpath.join(tests_dir, r["bundle"], f"{r['target']}.rs")
        main_rs = posixpath.join(tests_dir, r["bundle"], "main.rs")
        if os.path.exists(orig):
            errs.append(f"{r['path']}: still at the original path")
        if not os.path.isfile(os.path.join(top, dest)):
            errs.append(f"{dest}: missing")
        if main_rs not in mod_cache:
            p = os.path.join(top, main_rs)
            mod_cache[main_rs] = strip_comments(open(p, encoding="utf-8").read()) if os.path.isfile(p) else ""
        if not re.search(rf"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+{re.escape(r['target'])}\s*;", mod_cache[main_rs], re.M):
            errs.append(f"{main_rs}: no `mod {r['target']};`")
        ok += 1
    for e in errs:
        print(f"ERROR {e}", file=sys.stderr)
    print(f"verify-leaf {leaf}: {ok} moved target(s) of the leaf, {stay} keep/r6 target(s) checked in place, "
          f"{len(errs)} error(s)")
    return 1 if errs else 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--write", metavar="OUT")
    g.add_argument("--check", action="store_true")
    g.add_argument("--verify-leaf", metavar="LEAF")
    ap.add_argument("inventory", nargs="?")
    ap.add_argument("--base")
    a = ap.parse_args(argv)
    if a.write:
        base = a.base or git("merge-base", "HEAD", "main").strip()
        inv = build(base)
        with open(a.write, "w", encoding="utf-8") as f:
            json.dump(inv, f, ensure_ascii=False, indent=1)
            f.write("\n")
        print(json.dumps(inv["summary"], ensure_ascii=False))
        return 0
    if not a.inventory:
        ap.error("INVENTORY.json is required")
    return check(a.inventory) if a.check else verify_leaf(a.verify_leaf, a.inventory)


if __name__ == "__main__":
    sys.exit(main())
