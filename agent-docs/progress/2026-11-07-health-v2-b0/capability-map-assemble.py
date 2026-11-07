#!/usr/bin/env python3
"""Merge the capability-map fragments and module-deps.json into capability-map.json.

Run from the repo root:
  python3 agent-docs/progress/2026-11-07-health-v2-b0/capability-map-assemble.py

Rules:
- capabilities: union of fragments (tests -> path strings, test_costs -> cost objects), dedup by id. A duplicate id keeps both owners
  (owners/invariants/public_ports/tests are unioned).
- shared_state: module-deps.json entries plus fragment entries, keyed by (kind, name).
  Fragment event names (Rust variant, CamelCase) are mapped to snake_case.
  Fragment table names "a / b / c" are split; a writer is attached to a part only
  if its file mentions the table name. "tasks（... 列）" is merged into "tasks"
  with the qualifier kept in notes.
- multi_writer is recomputed: writers span >= 2 crates or >= 2 top-level modules
  (the first path segment under src/, derived from the path, same as module-deps).
"""
import glob
import json
import os
import re
import sys

BASE = "agent-docs/progress/2026-11-07-health-v2-b0"
SCHEMA = "celeris.health-v2-b0.capability-map/1"


def snake(name):
    return re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower()


def top_module(crate, path):
    parts = path.split("/")
    if parts[0] == "crates" and len(parts) >= 4 and parts[2] == "src":
        seg = parts[3]
        seg = seg[:-3] if seg.endswith(".rs") else seg
        return f"{crate}::{seg}"
    return f"{crate}::{'/'.join(parts[:2])}"


def main():
    deps = json.load(open(f"{BASE}/module-deps.json"))
    frags = [json.load(open(f)) for f in sorted(glob.glob(f"{BASE}/capability-map/*.json"))]
    base = deps["base_sha"]
    for f in frags:
        if f["base_sha"] != base:
            sys.exit(f"base_sha mismatch in {f['group']}")

    caps, order = {}, []
    for f in frags:
        for c in f["capabilities"]:
            c = dict(c)
            if c["id"] not in caps:
                c["groups"] = [f["group"]]
                caps[c["id"]] = c
                order.append(c["id"])
                continue
            cur = caps[c["id"]]
            cur["groups"].append(f["group"])
            for key in ("owners", "invariants", "public_ports", "tests"):
                for item in c[key]:
                    if item not in cur[key]:
                        cur[key].append(item)
    capabilities = [caps[i] for i in order]
    # tests: path strings (the task acceptance reads them as paths);
    # test_costs keeps target and build/link seconds per test.
    for c in capabilities:
        c["test_costs"] = c.pop("tests")
        c["tests"] = [t["path"] for t in c["test_costs"]]

    state = {}
    for s in deps["shared_state"]:
        e = dict(s)
        e["sources"] = ["module-deps"]
        e.setdefault("notes", [])
        if "note" in e:
            e["notes"].append(e.pop("note"))
        state[(s["kind"], s["name"])] = e
    known_tables = {n for (k, n) in state if k == "sqlite_table"}
    known_events = {n for (k, n) in state if k == "event_kind"}

    def add(kind, name, writers, group, note=None, extra=None):
        e = state.setdefault(
            (kind, name),
            {"kind": kind, "name": name, "writers": [], "sources": [], "notes": []},
        )
        for w in writers:
            if not any(x["path"] == w["path"] for x in e["writers"]):
                e["writers"].append(w)
        if group not in e["sources"]:
            e["sources"].append(group)
        if note and note not in e["notes"]:
            e["notes"].append(note)
        for k, v in (extra or {}).items():
            e.setdefault(k, v)

    def mentions(path, table):
        try:
            text = open(path, encoding="utf-8").read()
        except OSError:
            return False
        return re.search(rf"\b{re.escape(table)}\b", text) is not None

    for f in frags:
        g = f["group"]
        for s in f["shared_state"]:
            kind, name, writers = s["kind"], s["name"], s["writers"]
            extra = {k: v for k, v in s.items() if k not in ("kind", "name", "writers", "note")}
            note = s.get("note")
            if kind == "event_kind":
                sn = snake(name)
                if sn not in known_events:
                    note = f"fragment name {name}; not in module-deps event list"
                add(kind, sn, writers, g, note, extra)
            elif kind == "sqlite_table" and " / " in name:
                for part in [p.strip() for p in name.split(" / ")]:
                    ws = [w for w in writers if mentions(w["path"], part)]
                    add(kind, part, ws, g, f"split from fragment entry '{name}'", extra)
            elif kind == "sqlite_table" and "（" in name:
                b, q = name.split("（", 1)
                b = b.strip()
                if b in known_tables:
                    add(kind, b, writers, g, f"fragment qualifier: {q.rstrip('）')}", extra)
                else:
                    add(kind, name, writers, g, note, extra)
            else:
                add(kind, name, writers, g, note, extra)

    shared = []
    for (kind, name), e in sorted(state.items()):
        e["writer_crates"] = sorted({w["crate"] for w in e["writers"]})
        e["writer_modules"] = sorted({top_module(w["crate"], w["path"]) for w in e["writers"]})
        e["multi_writer"] = len(e["writer_crates"]) >= 2 or len(e["writer_modules"]) >= 2
        shared.append(e)
    multi = [
        {
            "kind": e["kind"],
            "name": e["name"],
            "writer_crates": e["writer_crates"],
            "writer_modules": e["writer_modules"],
            "live": e.get("live", True),
        }
        for e in shared
        if e["multi_writer"]
    ]

    diffs, seen = [], set()
    for f in frags:
        for a in f["arch_map_diffs"]:
            key = (a["arch_map_line"], a["kind"], a["observed"])
            if key in seen:
                continue
            seen.add(key)
            diffs.append(dict(a, group=f["group"]))
    diffs.sort(key=lambda a: (a["arch_map_line"], a["kind"]))

    missing = [
        p
        for c in capabilities
        for p in [x["path"] for x in c["owners"]] + c["tests"]
        if not os.path.exists(p)
    ]
    out = {
        "schema": SCHEMA,
        "base_sha": base,
        "generator": f"{BASE}/capability-map-assemble.py",
        "inputs": [f"{BASE}/capability-map/{f['group']}.json" for f in frags]
        + [f"{BASE}/module-deps.json"],
        "summary": {
            "capabilities": len(capabilities),
            "tests": sum(len(c["tests"]) for c in capabilities),
            "tests_without_cost": sum(
                1 for c in capabilities for t in c["test_costs"] if t["build_link_secs"] is None
            ),
            "shared_state": len(shared),
            "multi_writer_state": len(multi),
            "multi_writer_state_live": sum(1 for m in multi if m["live"]),
            "arch_map_diffs": len(diffs),
            "missing_paths": missing,
        },
        "capabilities": capabilities,
        "crate_deps": deps["crate_deps"],
        "module_uses": deps["module_uses"],
        "cycles": deps["cycles"],
        "shared_state": shared,
        "multi_writer_state": multi,
        "arch_map_diffs": diffs,
    }
    with open(f"{BASE}/capability-map.json", "w", encoding="utf-8") as fh:
        json.dump(out, fh, ensure_ascii=False, indent=1)
        fh.write("\n")
    print(json.dumps(out["summary"], ensure_ascii=False))
    return 1 if missing else 0


if __name__ == "__main__":
    sys.exit(main())
