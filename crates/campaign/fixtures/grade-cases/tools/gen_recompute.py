#!/usr/bin/env python3
"""K3/K4 の grade-case/1（recompute）を原票 tgz から作る。固定入力の sha256 は原票の実 file から計算する。
使い方: gen_recompute.py <main-43223.tgz> <main-v3-n2-43706.tgz> <grade-cases dir>

- `input_archive`: grader が自分で展開する固定入力 tgz（grade-case の隣に同名で置く。repo には置かず launcher の prepare が
  `--inputs-src` から写す。ADR 2026-10-23-k4-cells-validity-grading D3）。`pinned_raw`・metrics の path は展開先 `inputs/` 起点。
- K4 は `cells`（セル有効性の再計算: planned-cells.json の期待集合、qstat -xf 記録の job 終了状態、実効 fsync/checkRead/api、
  完全な出力、results/cells.csv と件数 metric の照合）を持つ。旧 5 件数 metric も残す。
- K3 の統計量は反復平均の帯域（比は作らない: K3 の比の統計量は人の確認待ち）。"""
import hashlib, json, os, re, sys, tarfile

def sha(b): return hashlib.sha256(b).hexdigest()

def members(tgz, suffixes=(".json",)):
    with tarfile.open(tgz) as t:
        return {m.name.removeprefix("./"): t.extractfile(m).read() for m in t if m.isfile() and m.name.endswith(suffixes)}

def write(out, split, cid, rec):
    case = {"schema": "celeris.grade-case/1", "id": cid, "split": split, "recompute": rec}
    path = f"{out}/{split}/{cid}/{cid}.json"
    open(path, "w").write(json.dumps(case, ensure_ascii=False, indent=2) + "\n")

def archive(tgz):
    return {"path": os.path.basename(tgz), "sha256": sha(open(tgz, "rb").read()), "extract_to": "inputs"}

def k3(tgz, out):
    fs = members(tgz)
    ior = {n: b for n, b in fs.items() if n.endswith("ior_results/ior_result_0.json")}
    pinned, metrics = {}, []
    for cond in ["L_base", "L_A", "L_B", "L_AB", "U"]:
        for x in ["4k", "1m", "4m"]:
            files = sorted(n for n in ior if f"/{cond}_t{x}_p1_sep/" in n)
            assert len(files) == 3, (cond, x, files)
            for n in files: pinned["inputs/" + n] = sha(ior[n])
            for i, op in enumerate(["write", "read"]):
                metrics.append({"name": f"bw_{cond}_{x}_{op}_mean_mib",
                                "source": {"json": {"files": ["inputs/" + n for n in files], "pointer": f"/summary/{i}/bwMeanMIB"}},
                                "agg": "mean", "tolerance_rel": 0.001})
    write(out, "dev", "k3-locusta-ab-analysis",
          {"report": "report.md", "input_archive": archive(tgz), "pinned_raw": pinned, "metrics": metrics})

def k4(tgz, out):
    fs = members(tgz, (".json", ".txt"))
    cells = sorted(n for n in fs if n.endswith("/cell.json") and "/cells/" in n)
    planned = [n for n in fs if n.endswith("/planned-cells.json")]
    assert len(planned) == 1, planned
    run = os.path.dirname(planned[0])
    qstat = {re.search(r"qstat-(\d+)\.txt$", n)[1]: n for n in fs if re.search(r"(^|/)review\d*/qstat-\d+\.txt$", n)}
    jobs = {json.loads(fs[n])["job"].split(".")[0] for n in cells}
    assert jobs <= set(qstat), (jobs, qstat)
    pinned = {"inputs/" + n: sha(fs[n]) for n in cells}
    pinned["inputs/" + planned[0]] = sha(fs[planned[0]])
    for j in sorted(jobs): pinned["inputs/" + qstat[j]] = sha(fs[qstat[j]])
    metrics = [{"name": "cells_with_cell_json", "source": {"json": {"files": ["inputs/" + n for n in cells], "pointer": "/nhosts"}},
                "agg": "count", "tolerance_rel": 0.0}]
    for fsn in ["benchfs", "chfs", "gekkofs", "unifyfs"]:
        c = [n for n in cells if f"/cells/{fsn}/" in n]
        metrics.append({"name": f"cells_{fsn}", "source": {"json": {"files": ["inputs/" + n for n in c], "pointer": "/nhosts"}},
                        "agg": "count", "tolerance_rel": 0.0})
    cells_spec = {"run": "inputs/" + run, "planned": "planned-cells.json",
                  "jobs": {j: "inputs/" + qstat[j] for j in sorted(jobs)},
                  "table": "results/cells.csv",
                  "api": {"benchfs": "BENCHFS", "chfs": "CHFS", "gekkofs": "POSIX", "unifyfs": "POSIX"},
                  "tolerance_rel": 0.001}
    write(out, "final", "k4-cmp4-validity-analysis",
          {"report": "report.md", "input_archive": archive(tgz), "pinned_raw": pinned, "metrics": metrics, "cells": cells_spec})

k3(sys.argv[1], sys.argv[3]); k4(sys.argv[2], sys.argv[3])
