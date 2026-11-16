#!/usr/bin/env python3
"""V0 targets の記録（targets.json）を確かめる。

使い方: python3 agent-docs/progress/2026-11-15-health-v2-v0/targets-verify.py \
            agent-docs/progress/2026-11-15-health-v2-v0/targets.json

確かめること（欠けや不一致が 1 つでもあれば exit 1）:
  1. `python3 scripts/dev/test-target-inventory.py --json` を流し直し、totals の
     test_targets・expected_test_binaries が JSON の head と一致する。
  2. 主要 5 crate の build_no_run に test binary 数（整数）があり、inventory の
     expected_test_binaries と一致する。
  3. edit_verify に 3 回 × 5 crate の edit_to_verify_secs（数）と、前後の
     load average（/proc/loadavg の 1/5/15 分の 3 つの数）がある。
  4. 3 回の計測の rebuild_exit・list_exit が 0。
"""
from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

CRATES = ["task-core", "task-dispatch", "task-api", "task-worker", "celeris"]
ROUNDS = ["round1", "round2", "round3_after_settle"]


def is_num(v: object) -> bool:
    return isinstance(v, (int, float)) and not isinstance(v, bool)


def is_loadavg(v: object) -> bool:
    if not isinstance(v, str):
        return False
    parts = v.split()
    if len(parts) != 3:
        return False
    try:
        [float(p) for p in parts]
    except ValueError:
        return False
    return True


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print("usage: targets-verify.py <targets.json>", file=sys.stderr)
        return 2
    root = Path(__file__).resolve().parents[3]
    try:
        data = json.loads(Path(argv[1]).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as e:
        print(f"FAIL: {argv[1]} を読めない: {e}", file=sys.stderr)
        return 1

    errors: list[str] = []
    proc = subprocess.run(
        [sys.executable, "scripts/dev/test-target-inventory.py", "--json"],
        cwd=root,
        capture_output=True,
        text=True,
        check=False,
    )
    if proc.returncode != 0:
        print(f"FAIL: inventory が exit {proc.returncode}: {proc.stderr[-500:]}", file=sys.stderr)
        return 1
    inv = json.loads(proc.stdout)
    totals = inv.get("totals", {})

    head = data.get("head")
    if not isinstance(head, dict):
        errors.append("head が無い")
        head = {}
    for key in ("test_targets", "expected_test_binaries"):
        want = totals.get(key)
        got = head.get(key)
        if got != want:
            errors.append(f"head.{key}={got!r} が inventory の {want!r} と一致しない")

    build = data.get("build_no_run")
    if not isinstance(build, dict):
        errors.append("build_no_run が無い")
        build = {}
    inv_crates = inv.get("crates", {})
    if isinstance(inv_crates, list):
        inv_crates = {c.get("name"): c for c in inv_crates}
    for c in CRATES:
        b = build.get(c)
        if not isinstance(b, dict):
            errors.append(f"build_no_run.{c} が無い")
            continue
        nb = b.get("test_binaries")
        if not isinstance(nb, int) or isinstance(nb, bool):
            errors.append(f"build_no_run.{c}.test_binaries が整数でない: {nb!r}")
            continue
        exp = inv_crates.get(c, {}).get("expected_test_binaries")
        if nb != exp:
            errors.append(f"build_no_run.{c}.test_binaries={nb} が inventory の予測 {exp!r} と一致しない")

    ev = data.get("edit_verify")
    if not isinstance(ev, dict):
        errors.append("edit_verify が無い")
        ev = {}
    for r in ROUNDS:
        rd = ev.get(r)
        if not isinstance(rd, dict):
            errors.append(f"edit_verify.{r} が無い")
            continue
        for c in CRATES:
            m = rd.get(c)
            where = f"edit_verify.{r}.{c}"
            if not isinstance(m, dict):
                errors.append(f"{where} が無い")
                continue
            if not is_num(m.get("edit_to_verify_secs")):
                errors.append(f"{where}.edit_to_verify_secs が数でない: {m.get('edit_to_verify_secs')!r}")
            for k in ("loadavg_before", "loadavg_after"):
                if not is_loadavg(m.get(k)):
                    errors.append(f"{where}.{k} が load average の 3 つの数でない: {m.get(k)!r}")
            for k in ("rebuild_exit", "list_exit"):
                if m.get(k) != 0:
                    errors.append(f"{where}.{k}={m.get(k)!r}（0 でない）")

    if errors:
        for e in errors:
            print(f"FAIL: {e}", file=sys.stderr)
        return 1
    print(
        f"OK: head test_targets={totals['test_targets']} expected_test_binaries="
        f"{totals['expected_test_binaries']}、build_no_run 5 crate、edit_verify {len(ROUNDS)}×{len(CRATES)} 件"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
