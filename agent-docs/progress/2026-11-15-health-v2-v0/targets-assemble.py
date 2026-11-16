#!/usr/bin/env python3
"""V0 targets の原票（targets-raw/）を targets.json にまとめる。

使い方: python3 agent-docs/progress/2026-11-15-health-v2-v0/targets-assemble.py
原票は B0 の test-targets-measure.sh の出力（build-*.json・edit-*.json・round2/・round3/）。
B0 の値は agent-docs/progress/2026-11-07-health-v2-b0/test-targets.json から読む。
"""
from __future__ import annotations

import json
import re
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
RAW = HERE / "targets-raw"
B0 = ROOT / "agent-docs/progress/2026-11-07-health-v2-b0/test-targets.json"
CRATES = ["task-core", "task-dispatch", "task-api", "task-worker", "celeris"]
ROUND_DIRS = {"round1": RAW, "round2": RAW / "round2", "round3_after_settle": RAW / "round3"}
ORDER = {
    "round1": ["celeris", "task-worker", "task-api", "task-dispatch", "task-core"],
    "round2": ["celeris", "task-worker", "task-api", "task-dispatch", "task-core"],
    "round3_after_settle": ["celeris", "task-dispatch", "task-worker", "task-api", "task-core"],
}
COMPILING = re.compile(r"^\s*Compiling (\S+) v")


def compiled(log: Path) -> list[str]:
    out = []
    for line in log.read_text(encoding="utf-8", errors="replace").splitlines():
        m = COMPILING.match(line)
        if m:
            out.append(m.group(1))
    return out


def host() -> dict:
    h: dict = {}
    for line in (RAW / "host.txt").read_text(encoding="utf-8").splitlines():
        if line.startswith("cargo-nextest"):
            h["nextest"] = line
        elif line.startswith("cargo "):
            h["cargo"] = line
        elif re.match(r"^\d{4}-\d\d-\d\dT", line):
            h["started_at"] = line
        else:
            k, _, v = line.partition(" ")
            h[k] = int(v) if k == "nproc" else v
    h["loadavg_note"] = (
        "loadavg は /proc/loadavg の 1/5/15 分。計測中の cargo 自身の並列 build と、"
        "同じ host の他の run（並行する WorkUnit を含む）を分けていない"
    )
    return h


def main() -> None:
    b0 = json.loads(B0.read_text(encoding="utf-8"))
    inv = json.loads((RAW / "inventory.json").read_text(encoding="utf-8"))
    tot = inv["totals"]
    # HEAD の inventory は crates を list（name 付き）で返す。名前の dict に直す。
    inv["crates"] = {c["name"]: c for c in inv["crates"]} if isinstance(inv["crates"], list) else inv["crates"]
    head_sha = (RAW / "head.txt").read_text(encoding="utf-8").strip()

    build = {}
    for c in CRATES:
        d = json.loads((RAW / f"build-{c}.json").read_text(encoding="utf-8"))
        d["expected_test_binaries"] = inv["crates"][c]["expected_test_binaries"]
        d["test_targets"] = inv["crates"][c]["test_targets"]
        build[c] = d

    ev: dict = {}
    for r, d in ROUND_DIRS.items():
        ev[r] = {}
        for c in CRATES:
            m = json.loads((d / f"edit-{c}.json").read_text(encoding="utf-8"))
            m["compiled"] = compiled(d / f"edit-{c}.log")
            ev[r][c] = m
    st = (RAW / "round3/settle.txt").read_text(encoding="utf-8").split()
    ev["settle_exit"] = int(st[1])
    ev["settle_secs"] = int(st[2])
    ev["settle_loadavg"] = " ".join(st[3:])
    ev["order"] = ORDER

    cmp_rows = {}
    for c in CRATES:
        b0b = b0["build_no_run"][c]
        b0e = {r: b0["edit_verify"][r][c]["edit_to_verify_secs"] for r in ORDER}
        cmp_rows[c] = {
            "test_targets": [b0["inventory_crates"][c]["test_targets"], inv["crates"][c]["test_targets"]],
            "test_binaries": [b0b["test_binaries"], build[c]["test_binaries"]],
            "build_secs": [b0b["secs"], build[c]["secs"]],
            "edit_to_verify_secs": {r: [b0e[r], ev[r][c]["edit_to_verify_secs"]] for r in ORDER},
            "affected_crates": [
                b0["edit_verify"]["round3_after_settle"][c]["affected_crates"],
                ev["round3_after_settle"][c]["affected_crates"],
            ],
        }

    out = {
        "schema": "celeris.health-v2-v0.test-targets/1",
        "procedure": "agent-docs/progress/2026-11-07-health-v2-b0/test-targets.md の「手順」をそのまま（同じ measure script・同じ順・round3 の前に task-core の対象 14 crate で settle）",
        "head": {
            "sha": head_sha,
            "rule": "計測時の HEAD（WU の base と同じ tree。agent-docs の外は未変更）",
            "test_targets": tot["test_targets"],
            "expected_test_binaries": tot["expected_test_binaries"],
            "tests_dir_rs_files": tot["tests_dir_rs_files"],
            "tests_dir_support_files": tot["tests_dir_support_files"],
        },
        "b0": {
            "sha": b0["base"]["sha"],
            "test_targets": b0["inventory_totals"]["test_targets"],
            "expected_test_binaries": b0["inventory_totals"]["expected_test_binaries"],
            "tests_dir_rs_files": b0["inventory_totals"]["tests_dir_rs_files"],
            "tests_dir_support_files": b0["inventory_totals"]["tests_dir_support_files"],
            "settle_secs": b0["edit_verify"]["settle_secs"],
        },
        "host": host(),
        "inventory_crates": {
            k: {
                "test_targets": v["test_targets"],
                "test_targets_by_location": v.get("test_targets_by_location"),
                "expected_test_binaries": v["expected_test_binaries"],
                "tests_dir_rs_files": v["tests_dir"]["rs_files"],
                "tests_dir_support_files": v["tests_dir"]["support_files"],
            }
            for k, v in inv["crates"].items()
        },
        "build_no_run": build,
        "edit_verify": ev,
        "comparison_b0_to_head": cmp_rows,
    }
    (HERE / "targets.json").write_text(json.dumps(out, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    print(json.dumps(cmp_rows, ensure_ascii=False))
    print("b0 crates:", {k: v["test_targets"] for k, v in b0["inventory_crates"].items()})
    print("head crates:", {k: v["test_targets"] for k, v in inv["crates"].items()})
    print("settle", ev["settle_exit"], ev["settle_secs"], ev["settle_loadavg"])


if __name__ == "__main__":
    main()
