#!/usr/bin/env python3
"""Fail new/worsening size debt; frozen baseline is changed only by explicit review.

The frozen audit caps historical debt. The comparison revision also prevents
regrowth after a reduction has landed. Exceptions require reasons in the TOML.
No check writes or refreshes the baseline.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys

SOURCE = Path(__file__).with_name("source-size-report.py")
spec = importlib.util.spec_from_file_location("source_size_report", SOURCE)
ssr = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = ssr
spec.loader.exec_module(ssr)
BASELINE = "scripts/dev/source-size-baseline.json"


def key(w):
    return (w["path"], w["check"], w.get("module", ""))


def revision_value(root, revision, warning):
    """Measure only a warning's source at a resolved commit, without a checkout."""
    path = warning["path"]
    proc = subprocess.run(["git", "-C", root, "show", f"{revision}:{path}"],
                          capture_output=True, text=True)
    if proc.returncode:
        # Absent path is new debt. Other git errors must not grant an allowance.
        return 0
    lines = proc.stdout.splitlines()
    if warning["kind"] == "ts_production":
        return len(lines)
    info = ssr.analyze_rust_file(lines)
    if warning["check"] == ssr.CHECK_PRODUCTION_SIZE:
        return info.prod_lines
    return max((m["lines"] for m in info.inline_test_mods
                if m["name"] == warning["module"]), default=0)


def evaluate(report, baseline, previous):
    debts = {key(w): w for w in baseline["debt"]}
    failures = []
    for w in report["warnings"]:
        if w["excepted"]:
            continue
        if "value" not in w:
            failures.append({**w, "cause": "module correctness violation; no debt allowance"})
            continue
        old = debts.get(key(w))
        fixed = old["value"] if old and old["kind"] == w["kind"] else w["threshold"]
        prior = previous.get(key(w), 0)
        limit = max(w["threshold"], min(fixed, prior))
        if w["value"] > limit:
            failures.append({**w, "cause": f"value={w['value']} threshold={w['threshold']} "
                             f"baseline={fixed} comparison={prior} allowed={limit}"})
    return failures


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", default=".")
    parser.add_argument("--baseline", default=BASELINE)
    parser.add_argument("--base", help="comparison commit (default CELERIS_WU_BASE or merge-base HEAD main)")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    root = os.path.abspath(args.root)
    try:
        baseline = json.loads((Path(root) / args.baseline).read_text())
        if baseline["version"] != 1 or baseline["classification"] != "source-size-report/1":
            raise ValueError("unsupported baseline classification/version")
        sha = ssr.git(root, "rev-parse", "--verify", baseline["sha"] + "^{commit}").strip()
        if sha != baseline["sha"]:
            raise ValueError("baseline must record a full commit SHA")
        base = args.base or os.environ.get("CELERIS_WU_BASE") or ssr.git(root, "merge-base", "HEAD", "main").strip()
        base = ssr.git(root, "rev-parse", "--verify", base + "^{commit}").strip()
        prod, inline, exceptions = ssr.load_config(None, root)
        if baseline["thresholds"] != {"production_lines": prod, "inline_test_lines": inline}:
            raise ValueError("threshold change requires explicit baseline review")
        for debt in baseline["debt"]:
            if revision_value(root, sha, debt) != debt["value"]:
                raise ValueError(f"baseline does not match {sha}: {key(debt)}")
        report = ssr.run_report(root, prod, inline, exceptions)
        previous = {key(w): revision_value(root, base, w) for w in report["warnings"] if "value" in w}
        failures = evaluate(report, baseline, previous)
        result = {"baseline_sha": sha, "comparison_sha": base,
                  "head_sha": ssr.git(root, "rev-parse", "HEAD").strip(),
                  "scope": "working tree including untracked sources",
                  "warning_count": report["warning_count"], "excepted_count": report["excepted_count"],
                  "failures": failures}
        if args.json:
            print(json.dumps(result, indent=2))
        else:
            print(f"source-size gate: baseline={sha} comparison={base} head={result['head_sha']}")
            for w in failures:
                print(f"FAIL {w['path']} {w['check']} {w.get('module', '')}: {w['cause']}")
            print(f"source-size gate: {len(failures)} failure(s), {report['warning_count']} debt warning(s), "
                  f"{report['excepted_count']} reasoned exception(s)")
        return int(bool(failures))
    except Exception as exc:
        print(f"source-size gate: error: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
