#!/usr/bin/env python3
"""健全性v2 B0 slices.json の検査。

使い方:
  python3 slices-check.py <slices.json>
      11 工程の存在・必須欄・write_set の正規表現・依存順・R3/R4 の history・
      loc_views と metrics の値を raw JSON と照合する。違えば理由を出して exit 1。
  git diff --name-only <base> | python3 slices-check.py --write-set <KEY> <slices.json>
      stdin の path がすべて工程 KEY の write_set に入るかを確かめる。外れた path を出して exit 1。

raw は slices.json と同じ dir（loc-baseline.json・test-targets.json・capability-map.json・
dup-candidates.json・module-deps.json）と、inputs に書かれた親 task の task-history.json を読む。
"""
import json
import os
import re
import sys

KEYS = ["G1", "G2", "G3", "G4", "R1", "R2", "R3", "R4", "R5", "R6", "V0"]
DEPS = {
    "G1": ["B0"], "G2": ["G1"], "G3": ["G2"], "G4": ["G3"],
    "R1": ["G3"], "R2": ["R1"], "R3": ["G3"], "R4": ["R2", "R3"],
    "R5": ["R1"], "R6": ["R2", "R3", "R4", "R5"], "V0": ["G4", "R6"],
}
REQUIRED = ["key", "title", "scope", "write_set", "checks", "stop_conditions", "depends_on", "baseline_metrics"]
STOP_KINDS = {
    "invariant_test": "不変条件の試験",
    "outside_write_set": "write_set の外",
    "weaken_checker": "検査器・policy・baseline",
}
CANCELLED = {"R3": "01M5KWPW976PBWXG4B37TQ55XC", "R4": "01M5KWQ3WME71Q0ATMPTJA7XE7"}
STARTING = {"R3": {"task_store_fn_decls": 193, "task_store_supertraits": 16}, "R4": {"dispatcher_fields": 115}}
BASE_CHECKS = ["test-affected.sh", "cargo clippy", "cargo fmt", "check-health.sh", "check-doc-links.sh"]


def load(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def write_set_mode(key, doc):
    slices = {s.get("key"): s for s in doc.get("slices", [])}
    if key not in slices:
        print(f"unknown slice key: {key}")
        return 1
    pats = [re.compile(p) for p in slices[key]["write_set"]]
    outside = [p for p in (line.strip() for line in sys.stdin) if p and not any(r.search(p) for r in pats)]
    for p in outside:
        print(f"outside write_set of {key}: {p}")
    print(f"write-set {key}: {'ok' if not outside else f'{len(outside)} path(s) outside'}")
    return 1 if outside else 0


def check(doc, raw_dir):
    errs = []
    e = errs.append
    loc = load(os.path.join(raw_dir, "loc-baseline.json"))
    tt = load(os.path.join(raw_dir, "test-targets.json"))
    cm = load(os.path.join(raw_dir, "capability-map.json"))
    dup = load(os.path.join(raw_dir, "dup-candidates.json"))
    md = load(os.path.join(raw_dir, "module-deps.json"))
    th_path = next((p for p in doc.get("inputs", []) if p.endswith("task-history.json")), None)
    if not th_path or not os.path.isfile(th_path):
        e(f"inputs に読める task-history.json が無い: {th_path}")
        return errs
    th = load(th_path)

    # base_sha
    base = doc.get("base_sha")
    for name, val in [("loc-baseline.sha", loc.get("sha")), ("test-targets.base.sha", tt["base"]["sha"]),
                      ("capability-map.base_sha", cm.get("base_sha")), ("dup-candidates.base_sha", dup.get("base_sha")),
                      ("module-deps.base_sha", md.get("base_sha")), ("task-history.base_sha", th.get("base_sha"))]:
        if val != base:
            e(f"base_sha {base} != {name} {val}")

    # slices
    slices = doc.get("slices", [])
    by_key = {}
    for s in slices:
        k = s.get("key")
        if k in by_key:
            e(f"重複した key: {k}")
        by_key[k] = s
    for k in KEYS:
        if k not in by_key:
            e(f"工程 {k} が無い")
    for k in by_key:
        if k not in KEYS:
            e(f"想定外の工程 key: {k}")
    metric_names = set(doc.get("metrics", {})) | set(doc.get("loc_views", {}).get("views", {})) | set(doc.get("structural_metrics", {}))
    children = {c["title"].split("]")[0].split()[-1]: c["id"] for c in th["parent"]["children"]}
    for k in KEYS:
        s = by_key.get(k)
        if not s:
            continue
        for f in REQUIRED:
            v = s.get(f)
            if f == "depends_on":
                if not isinstance(v, list) or not v:
                    e(f"{k}.depends_on が空")
            elif not v:
                e(f"{k}.{f} が空")
        if s.get("task_id") and s["task_id"] != children.get(k):
            e(f"{k}.task_id {s['task_id']} != task-history の子 {children.get(k)}")
        sc = s.get("scope") or {}
        if not isinstance(sc, dict) or not sc.get("summary"):
            e(f"{k}.scope.summary が空")
        for p in s.get("write_set") or []:
            try:
                re.compile(p)
            except re.error as ex:
                e(f"{k}.write_set の正規表現 {p!r} が compile できない: {ex}")
        if sorted(s.get("depends_on") or []) != sorted(DEPS[k]):
            e(f"{k}.depends_on {s.get('depends_on')} != 期待 {DEPS[k]}")
        stops = " ".join(s.get("stop_conditions") or [])
        for kind, needle in STOP_KINDS.items():
            if needle not in stops:
                e(f"{k}.stop_conditions に {kind}（{needle}）が無い")
        if "replan" not in stops and "repair" not in stops:
            e(f"{k}.stop_conditions に repair/replan の行き先が無い")
        chk = " ".join(s.get("checks") or [])
        for c in BASE_CHECKS:
            if c not in chk:
                e(f"{k}.checks に {c} が無い")
        for m in s.get("baseline_metrics") or []:
            if m not in metric_names:
                e(f"{k}.baseline_metrics の {m} が metrics・loc_views・structural_metrics に無い")
    if doc.get("dependency_order") and doc["dependency_order"] != {k: by_key[k]["depends_on"] for k in KEYS if k in by_key}:
        e("dependency_order が各 slice の depends_on と食い違う")

    # history of R3/R4
    old = {o["id"]: o for o in th.get("old_health", [])}
    for k, tid in CANCELLED.items():
        h = (by_key.get(k) or {}).get("history")
        if not isinstance(h, dict):
            e(f"{k}.history が無い")
            continue
        if h.get("cancelled_task_id") != tid:
            e(f"{k}.history.cancelled_task_id {h.get('cancelled_task_id')} != {tid}")
        o = old.get(tid)
        if not o:
            e(f"task-history.json に {tid} が無い")
            continue
        runs = sum(v.get("runs", 0) for part in ("reviewer", "worker_planner", "planner") for v in o.get(part, {}).values())
        if h.get("status") != o["status"] or h.get("reason") != o["last_transition"][3] or h.get("runs") != runs:
            e(f"{k}.history（{h.get('status')}/{h.get('reason')}/runs {h.get('runs')}）!= raw（{o['status']}/{o['last_transition'][3]}/runs {runs}）")
        sp = h.get("starting_point") or {}
        for mk, mv in STARTING[k].items():
            if sp.get(mk) != mv:
                e(f"{k}.history.starting_point.{mk} {sp.get(mk)} != {mv}")
            if doc.get("structural_metrics", {}).get(mk, {}).get("value") != mv:
                e(f"structural_metrics.{mk} != {mv}")
        p9 = h.get("p9_mitigation") or {}
        if p9.get("recommended") != "A" or not set(p9.get("placement") or []) & {"G3", "R6"}:
            e(f"{k}.history.p9_mitigation に推奨案 A と置き場（G3 か R6）が無い")

    # loc_views
    lv = doc.get("loc_views", {})
    if lv.get("reduction_target_is_acceptance") is not False:
        e("loc_views.reduction_target_is_acceptance が false でない")
    if not lv.get("target_update_procedure"):
        e("loc_views.target_update_procedure が空")
    est = lv.get("user_estimate_lines", {})
    if est != {"low": 200000, "high": 300000}:
        e(f"loc_views.user_estimate_lines {est} != 20〜30 万行")
    raw_views = loc["views"]
    if set(lv.get("views", {})) != set(raw_views):
        e(f"loc_views.views の見方 {sorted(lv.get('views', {}))} != raw {sorted(raw_views)}")
    for name, rv in raw_views.items():
        v = lv.get("views", {}).get(name, {})
        if v.get("lines") != rv["lines"] or v.get("rust_lines") != rv["rust_lines"]:
            e(f"loc_views.{name} の行数 {v.get('lines')}/{v.get('rust_lines')} != raw {rv['lines']}/{rv['rust_lines']}")
        if v.get("gap_to_200k") != rv["lines"] - 200000 or v.get("gap_to_300k") != rv["lines"] - 300000:
            e(f"loc_views.{name} の差が lines - 20万/30万 と合わない")

    # metrics
    m = doc.get("metrics", {})
    rw = th["reviewer_window"]
    rc = m.get("review_cost", {})
    for key, val in [("window", rw["window"]), ("claude_runs", rw["by_family"]["claude"]["runs"]),
                     ("claude_cost_usd_median", rw["by_family"]["claude"]["cost_usd_median"]),
                     ("claude_cost_usd_p90", rw["by_family"]["claude"]["cost_usd_p90"]),
                     ("claude_tokens_median", rw["by_family"]["claude"]["tokens_median"]),
                     ("claude_tokens_p90", rw["by_family"]["claude"]["tokens_p90"]),
                     ("gpt_tokens_median", rw["by_family"]["gpt"]["tokens_median"])]:
        if rc.get(key) != val:
            e(f"metrics.review_cost.{key} {rc.get(key)} != raw {val}")
    ev = tt["edit_verify"]["round3_after_settle"]
    got = m.get("edit_verify_secs", {}).get("by_crate", {})
    if set(got) != set(ev):
        e(f"metrics.edit_verify_secs の crate {sorted(got)} != raw {sorted(ev)}")
    for c, v in ev.items():
        g = got.get(c, {})
        if g.get("secs") != v["edit_to_verify_secs"] or g.get("affected_crates") != v["affected_crates"]:
            e(f"metrics.edit_verify_secs.{c} {g} != raw {v['edit_to_verify_secs']}/{v['affected_crates']}")
    t = m.get("test_targets", {})
    if t.get("test_targets") != tt["inventory_totals"]["test_targets"]:
        e(f"metrics.test_targets.test_targets {t.get('test_targets')} != raw {tt['inventory_totals']['test_targets']}")
    if t.get("expected_test_binaries") != tt["inventory_totals"]["expected_test_binaries"]:
        e("metrics.test_targets.expected_test_binaries が raw と違う")
    if t.get("test_binaries_by_crate") != {k: v["test_binaries"] for k, v in tt["build_no_run"].items()}:
        e("metrics.test_targets.test_binaries_by_crate が raw と違う")
    for name in ("review_cost", "edit_verify_secs", "test_targets"):
        if not m.get(name, {}).get("source"):
            e(f"metrics.{name}.source（出典 path）が無い")
    sm = doc.get("structural_metrics", {})
    for name, val in [("multi_writer_state", cm["summary"]["multi_writer_state"]),
                      ("arch_map_diffs", cm["summary"]["arch_map_diffs"]),
                      ("module_cycles", len(cm["cycles"])),
                      ("dup_candidates_open", dup["summary"]["by_decision"]["delete"] + dup["summary"]["by_decision"]["merge"])]:
        if sm.get(name, {}).get("value") != val:
            e(f"structural_metrics.{name} {sm.get(name, {}).get('value')} != raw {val}")

    # capability owners: every capability once, slice is a key or "none"
    caps = {c["id"]: c for c in doc.get("capability_owners", [])}
    raw_caps = {c["id"]: c for c in cm["capabilities"]}
    if set(caps) != set(raw_caps):
        e(f"capability_owners の id が capability-map と違う: 欠け {sorted(set(raw_caps) - set(caps))} 余分 {sorted(set(caps) - set(raw_caps))}")
    for cid, c in caps.items():
        owners = {o["path"] for o in raw_caps.get(cid, {}).get("owners", [])}
        if c.get("canonical_owner") not in owners:
            e(f"capability {cid} の canonical_owner {c.get('canonical_owner')} が capability-map の owners に無い")
        if c.get("slice") not in KEYS + ["none"]:
            e(f"capability {cid} の slice {c.get('slice')} が工程 key でも none でもない")

    # dup-candidates: every delete/merge assigned exactly once, files inside the slice write_set
    dec = {c["id"]: c for c in dup["candidates"]}
    seen = {}
    for k in KEYS:
        s = by_key.get(k) or {}
        pats = []
        for p in s.get("write_set") or []:
            try:
                pats.append(re.compile(p))
            except re.error:
                pass
        for a in (s.get("scope") or {}).get("dup_assignments", []):
            cid = a.get("id")
            if cid in seen:
                e(f"dup 候補 {cid} が {seen[cid]} と {k} に重複して割り当てられた")
            seen[cid] = k
            c = dec.get(cid)
            if not c:
                e(f"{k} の dup 候補 {cid} が dup-candidates.json に無い")
                continue
            if a.get("decision") != c["decision"] or c["decision"] not in ("delete", "merge"):
                e(f"{k} の dup 候補 {cid} の decision {a.get('decision')} != raw {c['decision']}（delete/merge だけを割り当てる）")
            for f in c["files"]:
                path = f.rsplit(":", 1)[0] if re.search(r":\d+$", f) else f
                if not any(r.search(path) for r in pats):
                    e(f"{k} の dup 候補 {cid} の file {path} が {k} の write_set に入らない")
    for cid, c in dec.items():
        if c["decision"] in ("delete", "merge") and cid not in seen:
            e(f"dup 候補 {cid}（{c['decision']}）がどの工程にも割り当てられていない")
    return errs


def main(argv):
    if len(argv) == 4 and argv[1] == "--write-set":
        return write_set_mode(argv[2], load(argv[3]))
    if len(argv) != 2:
        print(__doc__)
        return 2
    path = argv[1]
    doc = load(path)
    errs = check(doc, os.path.dirname(os.path.abspath(path)))
    for x in errs:
        print(f"NG: {x}")
    if errs:
        print(f"slices-check: {len(errs)} 件の不一致")
        return 1
    print(f"slices-check: ok（工程 {len(doc['slices'])}、capability {len(doc['capability_owners'])}、base {doc['base_sha'][:12]}）")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
