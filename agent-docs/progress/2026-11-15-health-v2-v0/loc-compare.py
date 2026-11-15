#!/usr/bin/env python3
"""健全性 v2 V0 loc: B0 と同じ方法で行数・依存・共有状態を数え直し、比べる。

使い方（repo の root で実行する）:
  python3 agent-docs/progress/2026-11-15-health-v2-v0/loc-compare.py --build <out.json> [--head REV]
  python3 agent-docs/progress/2026-11-15-health-v2-v0/loc-compare.py --verify agent-docs/progress/2026-11-15-health-v2-v0/loc.json

--build は次を数えて JSON に書く。
  - B0 の基準（slices.json の base_sha）・`git merge-base <head> main`・head の 3 rev で
    `scripts/dev/source-size-report.py --exclusive-classes --rev <rev>` と同じ集計（run_exclusive_report）。
    4 見方・class × 言語・base からの差。
  - B0 の module-deps-measure.py を head の tree（git archive で $TMPDIR に展開）に当てた summary と cycles。
    B0 の module-deps.json と並べる。
  - 帰属: head の first-parent と、健全性 v2 の各 slice の task branch（refs/heads/celeris/<task id>）の
    first-parent を辿り、slice 自身の commit（WU の統合・直接の commit）の差を class × 言語で足す。
    main や他の slice の取り込み（Merge commit '…' into / sync: main）は slice 自身に数えない。
    base→head の総差から slice 自身の差を引いた残りは「他の task と取り込み」。
  - 見かけの増減: 同じ path の class の変化（分類の付け替え）と、slice 自身の commit の中で
    ある file から消えて別の file に現れた行（行の移動・file 分割）を class の組で数える。
--verify は記録した JSON を読み、base の 4 見方を source-size-report で数え直して slices.json の
loc_views と一致すること、head・merge-base の 4 見方、class × 言語の差、依存の欄が揃い、差が
head − base と合うことを確かめる。欠け・食い違いがあれば理由を stderr に出して 1 で終わる。
"""

from __future__ import annotations

import argparse
import collections
import importlib.util
import json
import os
import re
import subprocess
import sys
import tarfile
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
B0_DIR = os.path.join(os.path.dirname(HERE), "2026-11-07-health-v2-b0")
SLICES_JSON = os.path.join(B0_DIR, "slices.json")
MODULE_DEPS_JSON = os.path.join(B0_DIR, "module-deps.json")
MODULE_DEPS_MEASURE = os.path.join(B0_DIR, "module-deps-measure.py")
SCHEMA = "celeris.health-v2-v0.loc/1"
VIEWS = ["product_only", "own_total_with_tests", "all_tracked", "external_split_candidates"]
DEPS_KEYS = [
    "crate_deps", "module_uses", "module_cycles", "crate_cycles", "tables", "tables_live",
    "tables_multi_writer", "tables_multi_crate", "event_kinds", "event_kinds_multi_writer",
    "event_kinds_multi_crate", "files", "files_multi_writer",
]
# 健全性 v2 の slice（slices.json の task_id と同じ）。B0 の計測 WU も base の後に入った。
SLICE_KEYS = ["B0", "G1", "G2", "G3", "G4", "R1", "R2", "R3", "R4", "R5", "R6"]
B0_TASK = "01M6TYVJSPEYA2P8B7G56HSQ73"
MERGE_IN = re.compile(r"^(Merge commit '|Merge branch |sync: )")
MOVE_MIN_CHARS = 12  # 移動とみなす行の最小の長さ（strip 後）。括弧だけ・空行は数えない


def git(*args: str, binary: bool = False):
    out = subprocess.run(["git", *args], capture_output=True, check=True)
    return out.stdout if binary else out.stdout.decode("utf-8", "replace")


def load_ssr():
    path = os.path.join(os.getcwd(), "scripts", "dev", "source-size-report.py")
    spec = importlib.util.spec_from_file_location("source_size_report", path)
    if spec is None or spec.loader is None:
        raise SystemExit(f"cannot load {path}")
    mod = importlib.util.module_from_spec(spec)
    sys.modules["source_size_report"] = mod  # dataclass が module を引く
    spec.loader.exec_module(mod)
    return mod


def count_rev(ssr, rev: str) -> dict:
    cfg = ssr.load_exclusive_config(None, os.getcwd())
    rep = ssr.run_exclusive_report(os.getcwd(), cfg, rev=rev, with_files=False)
    return {
        "sha": rep["sha"],
        "config_sha256": rep["config_sha256"],
        "classification": rep["classification"],
        "views": {v: {"lines": rep["views"][v]["lines"], "rust_lines": rep["views"][v]["rust_lines"]} for v in VIEWS},
        "matrix": {c: {lang: cell["lines"] for lang, cell in langs.items()} for c, langs in rep["matrix"].items()},
        "class_totals": rep["class_totals"],
        "language_totals": rep["language_totals"],
        "totals": rep["totals"],
    }


def matrix_diff(a: dict, b: dict) -> dict:
    out = {}
    for c in sorted(set(a) | set(b)):
        langs = set(a.get(c, {})) | set(b.get(c, {}))
        out[c] = {lang: b.get(c, {}).get(lang, 0) - a.get(c, {}).get(lang, 0) for lang in sorted(langs)}
    return out


# ---------------------------------------------------------------- 依存と共有状態
def measure_deps(rev: str) -> dict:
    tmp = tempfile.mkdtemp(prefix="v0-deps-", dir=os.environ.get("TMPDIR"))
    data = git("archive", "--format=tar", rev, binary=True)
    tar_path = os.path.join(tmp, "tree.tar")
    with open(tar_path, "wb") as fh:
        fh.write(data)
    with tarfile.open(tar_path) as tf:
        tf.extractall(os.path.join(tmp, "tree"), filter="data")
    out = subprocess.run(
        [sys.executable, MODULE_DEPS_MEASURE, rev], cwd=os.path.join(tmp, "tree"), capture_output=True, check=True
    )
    return json.loads(out.stdout)


def deps_block(d: dict) -> dict:
    s = d["summary"]
    return {
        "summary": {k: s[k] for k in s},
        "cycles": [{"level": c["level"], "crate": c.get("crate"), "modules": c.get("modules"), "size": c.get("size")} for c in d["cycles"]],
        "multi_writer": {
            kind: sorted(e["name"] for e in d["shared_state"] if e["kind"] == kind and e.get("multi_writer"))
            for kind in sorted({e["kind"] for e in d["shared_state"]})
        },
    }


# ---------------------------------------------------------------- 帰属
class Blobs:
    def __init__(self):
        self.p = subprocess.Popen(["git", "cat-file", "--batch"], stdin=subprocess.PIPE, stdout=subprocess.PIPE)

    def get(self, sha: str) -> bytes:
        assert self.p.stdin and self.p.stdout
        self.p.stdin.write(sha.encode() + b"\n")
        self.p.stdin.flush()
        header = self.p.stdout.readline().split()
        if len(header) < 3 or header[1] == b"missing":
            return b""
        size = int(header[2])
        body = self.p.stdout.read(size)
        self.p.stdout.read(1)
        return body


def first_parent_chain(tip: str, stop: str) -> list[dict]:
    raw = git("rev-list", "--first-parent", "--parents", "--format=%s", f"{stop}..{tip}").split("\n")
    out = []
    i = 0
    while i < len(raw):
        if raw[i].startswith("commit "):
            ids = raw[i].split()[1:]
            out.append({"sha": ids[0], "parents": ids[1:], "subject": raw[i + 1] if i + 1 < len(raw) else ""})
            i += 2
        else:
            i += 1
    return out


def wu_refs() -> dict:
    """WU の branch: task id -> {key: sha}。"""
    refs: dict = collections.defaultdict(dict)
    for line in git("for-each-ref", "--format=%(refname) %(objectname)", "refs/heads/celeris-wu/").splitlines():
        ref, sha = line.split()
        parts = ref.split("/")[3:]
        if len(parts) >= 2:
            refs[parts[0]]["/".join(parts[1:])] = sha
    return refs


def is_ancestor(a: str, b: str) -> bool:
    return subprocess.run(["git", "merge-base", "--is-ancestor", a, b], capture_output=True).returncode == 0


def owner_of(commit: dict, refs: dict, task_ids: dict, current_tid: str) -> str | None:
    """統合の commit の WU の task。今の slice の WU の branch が含めば今の slice、他 task の WU の
    branch の先端と一致すればその task。どちらでもなければ None（後の branch が含むだけでは決めない）。"""
    m = re.match(r"^(?:integrate|merge) wu/(\S+)", commit["subject"])
    if not m or len(commit["parents"]) < 2:
        return None
    key, p2 = m.group(1), commit["parents"][1]
    sha = refs.get(current_tid, {}).get(key)
    if sha and (sha == p2 or is_ancestor(p2, sha)):
        return task_ids.get(current_tid, current_tid)
    for tid, keys in refs.items():
        if keys.get(key) == p2:
            return task_ids.get(tid, tid)
    return None


SLICE_PREFIX = re.compile(r"^(?:health-v2 )?(B0|G[1-4]|R[1-6]|V0)\b")


def slice_own_commits(base: str, slices: dict, refs: dict) -> dict:
    """slice -> 自身の commit（first-parent 上の WU 統合と直接の commit）。取り込みの merge は除く。"""
    task_ids = {tid: k for k, tid in slices.items()}
    own: dict = {}
    seen: set = set()
    for key, tid in slices.items():
        ref = f"refs/heads/celeris/{tid}"
        try:
            tip = git("rev-parse", ref).strip()
        except subprocess.CalledProcessError:
            continue
        commits = []
        for c in first_parent_chain(tip, base):
            if c["sha"] in seen:
                break
            who = owner_of(c, refs, task_ids, tid)
            subj = c["subject"]
            pm = SLICE_PREFIX.match(subj)
            if pm and pm.group(1) != key:
                break
            into = re.search(r"into celeris/([0-9A-Z]+)", subj)
            if who is not None and who != key:
                break
            if into and into.group(1) != tid:
                break
            if MERGE_IN.match(subj):
                continue  # 取り込み。slice 自身ではない
            if re.match(r"^integrate [0-9A-Z]{10}:", subj):
                break  # 他 task の policy branch の統合
            commits.append(c)
        own[key] = commits
        seen.update(c["sha"] for c in commits)
    return own


def classify_lines(ssr, cfg, path: str, data: bytes) -> dict:
    if not data:
        return {}
    r = ssr.classify_file_exclusive(cfg, path, data)
    lang = r["language"]
    return {(c, lang): n for c, n in r["classes"].items()}


def dominant_class(ssr, cfg, path: str, data: bytes) -> str | None:
    cl = classify_lines(ssr, cfg, path, data)
    if not cl:
        return None
    return max(cl.items(), key=lambda kv: kv[1])[0][0]


def commit_delta(ssr, cfg, blobs: Blobs, commit: dict, moves: collections.Counter, moved_paths: collections.Counter) -> collections.Counter:
    parent = commit["parents"][0]
    raw = git("diff-tree", "-r", "--no-renames", "--raw", "-z", parent, commit["sha"])
    fields = raw.split("\0")
    delta: collections.Counter = collections.Counter()
    removed: dict = collections.defaultdict(list)
    added: dict = collections.defaultdict(list)
    i = 0
    while i + 1 < len(fields):
        meta, path = fields[i], fields[i + 1]
        i += 2
        if not meta.startswith(":"):
            continue
        _om, _nm, osha, nsha, _st = meta[1:].split()
        old = blobs.get(osha) if set(osha) != {"0"} else b""
        new = blobs.get(nsha) if set(nsha) != {"0"} else b""
        for k, n in classify_lines(ssr, cfg, path, old).items():
            delta[k] -= n
        for k, n in classify_lines(ssr, cfg, path, new).items():
            delta[k] += n
        if b"\0" in old[:8000] or b"\0" in new[:8000]:
            continue
        oc = dominant_class(ssr, cfg, path, old) if old else None
        nc = dominant_class(ssr, cfg, path, new) if new else None
        ol = collections.Counter(x.strip() for x in old.decode("utf-8", "replace").split("\n") if len(x.strip()) >= MOVE_MIN_CHARS)
        nl = collections.Counter(x.strip() for x in new.decode("utf-8", "replace").split("\n") if len(x.strip()) >= MOVE_MIN_CHARS)
        for line, n in (ol - nl).items():
            removed[line].append((path, oc, n))
        for line, n in (nl - ol).items():
            added[line].append((path, nc, n))
    # 同じ commit の中で、ある file から消えて別の file に現れた行を移動とみなす
    for line, outs in removed.items():
        ins = added.get(line)
        if not ins:
            continue
        ins = [x for x in ins]
        for opath, oc, on in outs:
            for j, (ipath, ic, inn) in enumerate(ins):
                if ipath == opath or on == 0 or inn == 0:
                    continue
                m = min(on, inn)
                moves[f"{oc}->{ic}"] += m
                moved_paths[f"{opath} -> {ipath}"] += m
                on -= m
                ins[j] = (ipath, ic, inn - m)
    return delta


def counter_to_matrix(c: collections.Counter) -> dict:
    out: dict = {}
    for (cls, lang), n in sorted(c.items()):
        if n:
            out.setdefault(cls, {})[lang] = n
    return out


def views_of_matrix(m: dict, view_classes: dict) -> dict:
    out = {}
    for v, classes in view_classes.items():
        out[v] = {
            "lines": sum(n for c in classes for n in m.get(c, {}).values()),
            "rust_lines": sum(m.get(c, {}).get("rust", 0) for c in classes),
        }
    return out


def reclassified(ssr, base: str, head: str) -> dict:
    cfg = ssr.load_exclusive_config(None, os.getcwd())
    b = {f["path"]: f for f in ssr.run_exclusive_report(os.getcwd(), cfg, rev=base, with_files=True)["files"]}
    h = {f["path"]: f for f in ssr.run_exclusive_report(os.getcwd(), cfg, rev=head, with_files=True)["files"]}
    moves: collections.Counter = collections.Counter()
    examples = []
    for p in sorted(set(b) & set(h)):
        bc = max(b[p]["classes"].items(), key=lambda kv: kv[1])[0] if b[p]["classes"] else None
        hc = max(h[p]["classes"].items(), key=lambda kv: kv[1])[0] if h[p]["classes"] else None
        if bc and hc and bc != hc:
            moves[f"{bc}->{hc}"] += h[p]["lines"]
            examples.append({"path": p, "from": bc, "to": hc, "lines_base": b[p]["lines"], "lines_head": h[p]["lines"]})
    return {"by_pair": dict(moves), "files": examples}


def build(out_path: str, head_rev: str) -> int:
    ssr = load_ssr()
    sl = json.load(open(SLICES_JSON, encoding="utf-8"))
    base = sl["base_sha"]
    head = git("rev-parse", head_rev).strip()
    mb = git("merge-base", head, "main").strip()
    revs = {"base": count_rev(ssr, base), "merge_base": count_rev(ssr, mb), "head": count_rev(ssr, head)}
    view_classes = {v: sl_v for v, sl_v in ((v, None) for v in VIEWS)}
    cfg = ssr.load_exclusive_config(None, os.getcwd())
    rep = ssr.run_exclusive_report(os.getcwd(), cfg, rev=base, with_files=False)
    view_classes = {v: rep["views"][v]["classes"] for v in VIEWS}
    diffs = {
        "head_minus_base": {
            "views": {v: {k: revs["head"]["views"][v][k] - revs["base"]["views"][v][k] for k in ("lines", "rust_lines")} for v in VIEWS},
            "matrix": matrix_diff(revs["base"]["matrix"], revs["head"]["matrix"]),
            "class_totals": {c: revs["head"]["class_totals"].get(c, 0) - revs["base"]["class_totals"].get(c, 0) for c in revs["base"]["class_totals"]},
        },
        "head_minus_merge_base": {
            "views": {v: {k: revs["head"]["views"][v][k] - revs["merge_base"]["views"][v][k] for k in ("lines", "rust_lines")} for v in VIEWS},
        },
    }
    # 帰属
    slices = {s["key"]: s["task_id"] for s in sl["slices"] if s["key"] in SLICE_KEYS}
    slices = {"B0": B0_TASK, **slices}
    refs = wu_refs()
    own = slice_own_commits(base, slices, refs)
    blobs = Blobs()
    attribution = {}
    total_own: collections.Counter = collections.Counter()
    for key in SLICE_KEYS:
        commits = [c for c in own.get(key, []) if is_ancestor(c["sha"], head)]
        moves: collections.Counter = collections.Counter()
        moved_paths: collections.Counter = collections.Counter()
        delta: collections.Counter = collections.Counter()
        for c in commits:
            delta.update(commit_delta(ssr, cfg, blobs, c, moves, moved_paths))
        total_own.update(delta)
        m = counter_to_matrix(delta)
        attribution[key] = {
            "task_id": slices[key],
            "own_commits": len(commits),
            "views": views_of_matrix(m, view_classes),
            "matrix": m,
            "moved_lines_by_class_pair": dict(sorted(moves.items(), key=lambda kv: -kv[1])),
            "moved_lines_top_paths": [{"pair": p, "lines": n} for p, n in moved_paths.most_common(8)],
        }
    total_m = diffs["head_minus_base"]["matrix"]
    own_m = counter_to_matrix(total_own)
    rest = matrix_diff(own_m, total_m)
    rest = {c: {lang: n for lang, n in langs.items() if n} for c, langs in rest.items()}
    rest = {c: v for c, v in rest.items() if v}
    deps_head = measure_deps(head)
    deps_b0 = json.load(open(MODULE_DEPS_JSON, encoding="utf-8"))
    out = {
        "schema": SCHEMA,
        "method": {
            "loc": "scripts/dev/source-size-report.py --exclusive-classes --rev <rev>（run_exclusive_report を同じ config で呼ぶ）",
            "deps": "agent-docs/progress/2026-11-07-health-v2-b0/module-deps-measure.py <rev> を git archive で展開した <rev> の tree で実行（B0 と同じ引数）",
            "attribution": "健全性 v2 の各 slice の task branch の first-parent 上で、その slice の WU の統合と直接の commit の差（親 1 → commit）を class × 言語で足す。Merge commit/sync: main の取り込みは除く",
            "moves": f"slice 自身の commit の中で、ある file から消えて別の file に現れた行（strip 後 {MOVE_MIN_CHARS} 文字以上、完全一致）を file の主な class の組で数える",
            "reclassified": "base と head の両方にある path で主な class が変わったもの（分類の付け替え）",
        },
        "revs": {"base": base, "merge_base": mb, "head": head},
        "view_classes": view_classes,
        "counts": revs,
        "diffs": diffs,
        "attribution": {
            "slices": attribution,
            "slices_total": {"views": views_of_matrix(own_m, view_classes), "matrix": own_m},
            "other_tasks_and_merges": {"views": views_of_matrix(rest, view_classes), "matrix": rest},
        },
        "reclassified": reclassified(ssr, base, head),
        "deps": {"b0": deps_block(deps_b0), "head": deps_block(deps_head)},
    }
    with open(out_path, "w", encoding="utf-8") as fh:
        json.dump(out, fh, ensure_ascii=False, indent=1)
        fh.write("\n")
    print(f"wrote {out_path}")
    return 0


# ---------------------------------------------------------------- verify
def verify(path: str) -> int:
    errors: list[str] = []
    try:
        d = json.load(open(path, encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as e:
        print(f"FAIL: cannot read {path}: {e}", file=sys.stderr)
        return 1
    if d.get("schema") != SCHEMA:
        errors.append(f"schema が {SCHEMA} でない: {d.get('schema')!r}")
    sl = json.load(open(SLICES_JSON, encoding="utf-8"))
    revs = d.get("revs", {})
    if revs.get("base") != sl["base_sha"]:
        errors.append(f"revs.base {revs.get('base')!r} が slices.json の base_sha {sl['base_sha']} と違う")
    counts = d.get("counts", {})
    for r in ("base", "merge_base", "head"):
        c = counts.get(r)
        if not c:
            errors.append(f"counts.{r} が無い")
            continue
        for v in VIEWS:
            cell = c.get("views", {}).get(v)
            if not cell or not isinstance(cell.get("lines"), int) or not isinstance(cell.get("rust_lines"), int):
                errors.append(f"counts.{r}.views.{v} が欠けている")
        if not c.get("matrix"):
            errors.append(f"counts.{r}.matrix（class × 言語）が無い")
        elif sum(sum(x.values()) for x in c["matrix"].values()) != c.get("totals", {}).get("tracked_lines"):
            errors.append(f"counts.{r}.matrix の合計が totals.tracked_lines と合わない")
    # base を数え直して slices.json の loc_views と比べる
    ssr = load_ssr()
    try:
        recount = count_rev(ssr, sl["base_sha"])
    except subprocess.CalledProcessError as e:
        errors.append(f"base を数え直せない: {e}")
        recount = None
    if recount:
        for v in VIEWS:
            want = sl["loc_views"]["views"][v]
            got = recount["views"][v]
            rec = counts.get("base", {}).get("views", {}).get(v, {})
            for k in ("lines", "rust_lines"):
                if got[k] != want[k]:
                    errors.append(f"base の {v}.{k} を数え直すと {got[k]}、slices.json の loc_views は {want[k]}")
                if rec.get(k) != want[k]:
                    errors.append(f"loc.json の base {v}.{k} {rec.get(k)} が slices.json の {want[k]} と違う")
    # head を記録の sha で数え直す
    if counts.get("head", {}).get("sha"):
        try:
            hc = count_rev(ssr, counts["head"]["sha"])
            for v in VIEWS:
                for k in ("lines", "rust_lines"):
                    if hc["views"][v][k] != counts["head"]["views"].get(v, {}).get(k):
                        errors.append(f"head の {v}.{k} を数え直すと {hc['views'][v][k]}、記録は {counts['head']['views'].get(v, {}).get(k)}")
            if hc["matrix"] != counts["head"].get("matrix"):
                errors.append("head の class × 言語を数え直すと記録と違う")
        except subprocess.CalledProcessError as e:
            errors.append(f"head（{counts['head']['sha']}）を数え直せない: {e}")
    # 差の欄
    diff = d.get("diffs", {}).get("head_minus_base", {})
    if counts.get("base") and counts.get("head"):
        want_m = matrix_diff(counts["base"].get("matrix", {}), counts["head"].get("matrix", {}))
        if diff.get("matrix") != want_m:
            errors.append("diffs.head_minus_base.matrix（class × 言語の差）が head − base と合わない、または無い")
        for v in VIEWS:
            for k in ("lines", "rust_lines"):
                want = counts["head"]["views"].get(v, {}).get(k, 0) - counts["base"]["views"].get(v, {}).get(k, 0)
                if diff.get("views", {}).get(v, {}).get(k) != want:
                    errors.append(f"diffs.head_minus_base.views.{v}.{k} が {want} でない")
    # 帰属の和
    att = d.get("attribution", {})
    if not att.get("slices"):
        errors.append("attribution.slices が無い")
    elif diff.get("matrix") is not None:
        own = att.get("slices_total", {}).get("matrix", {})
        rest = att.get("other_tasks_and_merges", {}).get("matrix", {})
        for c in set(diff["matrix"]) | set(own) | set(rest):
            for lang in set(diff["matrix"].get(c, {})) | set(own.get(c, {})) | set(rest.get(c, {})):
                if own.get(c, {}).get(lang, 0) + rest.get(c, {}).get(lang, 0) != diff["matrix"].get(c, {}).get(lang, 0):
                    errors.append(f"帰属の和（slice + 他）が総差と合わない: {c}/{lang}")
    # 依存の欄
    deps = d.get("deps", {})
    b0deps = json.load(open(MODULE_DEPS_JSON, encoding="utf-8"))["summary"]
    for side in ("b0", "head"):
        s = deps.get(side, {}).get("summary", {})
        for k in DEPS_KEYS:
            if not isinstance(s.get(k), int):
                errors.append(f"deps.{side}.summary.{k} が無い")
        if "cycles" not in deps.get(side, {}):
            errors.append(f"deps.{side}.cycles が無い")
    if deps.get("b0", {}).get("summary") and deps["b0"]["summary"] != b0deps:
        errors.append("deps.b0.summary が B0 の module-deps.json の summary と違う")
    if errors:
        for e in errors:
            print(f"FAIL: {e}", file=sys.stderr)
        return 1
    print(f"OK: base の 4 見方は slices.json の loc_views と一致、head・merge-base の 4 見方・class × 言語の差・帰属・依存の欄が揃う（{path}）")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--build", metavar="OUT")
    g.add_argument("--verify", metavar="JSON")
    ap.add_argument("--head", default="HEAD")
    a = ap.parse_args()
    if a.build:
        return build(a.build, a.head)
    return verify(a.verify)


if __name__ == "__main__":
    sys.exit(main())
