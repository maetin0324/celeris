#!/usr/bin/env python3
"""K3/K4 の grade-case/1（recompute のみ）を原票 tgz から作る。固定入力の sha256 は原票の実 file から計算する。
使い方: gen_recompute.py <main-43223.tgz> <main-v3-n2-43706.tgz> <grade-cases dir>
試行は原票を `inputs/` に展開する前提（case.json の fetch 手順）。統計量は反復平均の帯域（比は作らない: K3 の比の統計量は人の確認待ち）。"""
import hashlib, json, sys, tarfile

def sha(b): return hashlib.sha256(b).hexdigest()

def members(tgz):
    with tarfile.open(tgz) as t:
        return {m.name: t.extractfile(m).read() for m in t if m.isfile() and m.name.endswith(('.json',))}

def write(out, split, cid, rec):
    case = {"schema": "celeris.grade-case/1", "id": cid, "split": split, "recompute": rec}
    path = f"{out}/{split}/{cid}/{cid}.json"
    open(path, "w").write(json.dumps(case, ensure_ascii=False, indent=2) + "\n")

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
    write(out, "dev", "k3-locusta-ab-analysis", {"report": "report.md", "pinned_raw": pinned, "metrics": metrics})

def k4(tgz, out):
    fs = members(tgz)
    cells = sorted(n for n in fs if n.endswith("/cell.json") and "/cells/" in n)
    pinned = {"inputs/" + n: sha(fs[n]) for n in cells}
    metrics = [{"name": "cells_with_cell_json", "source": {"json": {"files": ["inputs/" + n for n in cells], "pointer": "/nhosts"}},
                "agg": "count", "tolerance_rel": 0.0}]
    for fsn in ["benchfs", "chfs", "gekkofs", "unifyfs"]:
        c = [n for n in cells if f"/cells/{fsn}/" in n]
        metrics.append({"name": f"cells_{fsn}", "source": {"json": {"files": ["inputs/" + n for n in c], "pointer": "/nhosts"}},
                        "agg": "count", "tolerance_rel": 0.0})
    write(out, "final", "k4-cmp4-validity-analysis", {"report": "report.md", "pinned_raw": pinned, "metrics": metrics})

k3(sys.argv[1], sys.argv[3]); k4(sys.argv[2], sys.argv[3])
