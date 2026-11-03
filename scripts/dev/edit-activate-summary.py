#!/usr/bin/env python3
"""S6 の edit→activate 測定の生データを scenario・mode ごとにまとめる（境界 ADR 付記 S5-10・S6）。

使い方:
  python3 -I scripts/dev/edit-activate-summary.py <jsonl>...

- 引数の file をまたいで集計する。同じ記録（kind・scenario・mode・rep・step・at が同じ行）は 1 回だけ数える。
  前半・後半の file とそれを連結した file を同時に渡しても二重に数えない。
- 1 試行の秒: TS の経路は kind=trial の行の secs。Rust の経路は kind=step の行を (scenario, mode, rep) で束ね、
  段の secs を足した値（edit-activate-baseline.sh --check-targets と同じ）。
- prime（温め）・skipped・affected の行は集計に入れない。estimate（release gate の gate.json の段の秒）は
  release ごとに段の秒を足して release_gate に別に出す（試行の集計とは混ぜない）。
- p50・p95 は最近傍順位（昇順で ceil(p*n/100) 番目）。
- exit 0: 全 scenario・mode が 20 試行以上で、どの試行も exit 0。exit 1: 20 試行未満の scenario・mode があるか、
  exit 0 でない試行・読めない行、段の数がそろわない Rust の試行がある。exit 2: file を読めない・引数が無い。
"""
import json
import math
import sys

MIN_TRIALS = 20
SKIP_KINDS = ("prime", "skipped", "affected")


def nearest_rank(values, p):
    ordered = sorted(values)
    return ordered[-(-p * len(ordered) // 100) - 1]


def is_secs(v):
    return isinstance(v, (int, float)) and not isinstance(v, bool) and math.isfinite(v) and v >= 0


def main(paths):
    if not paths:
        print(__doc__.strip().splitlines()[2].strip(), file=sys.stderr)
        return 2
    problems, seen = [], set()
    ts = {}  # (scenario, mode) -> {rep: (secs, exit)}
    rust = {}  # (scenario, mode) -> {rep: {step: (secs, exit)}}
    gate = {}  # release.at -> {step: secs}
    files = {}  # (scenario, mode) -> set(path)
    for path in paths:
        try:
            lines = open(path, encoding="utf-8").read().splitlines()
        except (OSError, UnicodeError) as e:
            print(f"cannot read {path}: {e}", file=sys.stderr)
            return 2
        for i, line in enumerate(lines, 1):
            if not line.strip():
                continue
            try:
                r = json.loads(line)
            except ValueError as e:
                problems.append(f"{path}:{i}: not JSON ({e})")
                continue
            if not isinstance(r, dict):
                problems.append(f"{path}:{i}: not an object")
                continue
            kind, scen, mode, rep = r.get("kind"), r.get("scenario"), r.get("mode"), r.get("rep")
            key = (kind, scen, mode, rep, r.get("step"), r.get("at"))
            if key in seen:
                continue
            seen.add(key)
            if kind in SKIP_KINDS:
                continue
            if kind == "estimate":
                rel = r.get("release") if isinstance(r.get("release"), dict) else {}
                if is_secs(r.get("secs")):
                    gate.setdefault(str(rel.get("at")), {})[r.get("step")] = r["secs"]
                continue
            tag = f"{path}:{i} ({scen} {mode} rep {rep})"
            if not is_secs(r.get("secs")):
                problems.append(f"{tag}: secs is not a number")
                continue
            files.setdefault((scen, mode), set()).add(path)
            if kind == "trial":
                reps = ts.setdefault((scen, mode), {})
                if rep in reps:
                    problems.append(f"{tag}: duplicate rep with a different record")
                reps[rep] = (r["secs"], r.get("exit"))
            elif kind == "step":
                steps = rust.setdefault((scen, mode), {}).setdefault(rep, {})
                if r.get("step") in steps:
                    problems.append(f"{tag}: duplicate step {r.get('step')!r} with a different record")
                steps[r.get("step")] = (r["secs"], r.get("exit"))
            else:
                problems.append(f"{tag}: unknown kind {kind!r}")
    trials = {}
    for k, reps in ts.items():
        trials[k] = [(rep, s, e == 0) for rep, (s, e) in reps.items()]
    for k, reps in rust.items():
        trials[k] = [(rep, round(sum(s for s, _ in st.values()), 3), all(e == 0 for _, e in st.values()))
                     for rep, st in reps.items()]
    out = {"min_trials": MIN_TRIALS, "scenarios": [], "release_gate": []}
    for (scen, mode), rows in sorted(trials.items(), key=lambda kv: (str(kv[0][0]), str(kv[0][1]))):
        values = [s for _, s, _ in rows]
        failed = sorted(rep for rep, _, ok in rows if not ok)
        steps = sorted({len(st) for st in rust.get((scen, mode), {}).values()})
        entry = {"scenario": scen, "mode": mode, "path": "ts" if (scen, mode) in ts else "rust",
                 "trials": len(values), "p50": nearest_rank(values, 50), "p95": nearest_rank(values, 95),
                 "min": min(values), "max": max(values), "failed_reps": failed,
                 "files": sorted(files.get((scen, mode), ()))}
        if steps:
            entry["steps_per_trial"] = steps
        out["scenarios"].append(entry)
        if len(values) < MIN_TRIALS:
            problems.append(f"{scen} {mode}: {len(values)} trials < {MIN_TRIALS}")
        if failed:
            problems.append(f"{scen} {mode}: trials not exit 0 (rep {failed})")
        if len(steps) > 1:
            problems.append(f"{scen} {mode}: trials have different step counts {steps}")
    for at, steps in sorted(gate.items()):
        out["release_gate"].append({"release_at": at, "steps": len(steps),
                                    "total_secs": round(sum(steps.values()), 3), "step_secs": steps})
    if not trials:
        problems.append("no trials")
    out["problems"] = problems
    print(json.dumps(out, ensure_ascii=False, indent=1))
    for p in problems:
        print(p, file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
