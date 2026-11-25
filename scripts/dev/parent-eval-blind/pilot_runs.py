"""Pilot output layout: iteration roots for comparison and blind scoring, and the arm dirs a run writes."""
import argparse
import pathlib
import sys

STRATEGY_STAGE = "strategy-v1"


def iteration_roots(runs):
    """Accept an iteration root or a container of iteration roots, in stable order.

    Case directories identify a direct root; otherwise its immediate directories
    are iterations. Duplicate names are rejected because metrics use those keys.
    """
    roots = []
    for value in runs:
        root = pathlib.Path(value)
        if not root.is_dir():
            raise ValueError(f"runs directory does not exist: {root}")
        if any((root / case).is_dir() for case in ("replay", "research")):
            roots.append(root)
        else:
            children = sorted(p for p in root.iterdir() if p.is_dir())
            if not children:
                raise ValueError(f"no iterations under runs directory: {root}")
            roots.extend(children)
    names = [root.resolve().name for root in roots]
    if len(set(names)) != len(names):
        raise ValueError(f"--runs dirs need distinct names: {names}")
    return roots


def run_dirs(out, case, points, arms):
    """The dirs one invocation writes: <out>/<case>/<point>/<arm>/, and <out>/<case>/strategy-v1/ when arm c runs."""
    out = pathlib.Path(out)
    dirs = [out / case / p / m for p in points for m in arms]
    if "c" in arms:
        dirs.insert(0, out / case / STRATEGY_STAGE)
    return dirs


def complete_stage(stage):
    """A strategy-v1 dir whose stage run finished (the driver writes run.json after the version file). Arm c of the same
    <out> (iteration) reuses it, so points can run one invocation at a time."""
    return (stage / "run.json").is_file() and (stage / "strategy-v1.json").is_file()


def existing_run_dirs(out, case, points, arms):
    """Dirs of run_dirs that already exist, except a complete strategy-v1 (reused, not rewritten). A run never deletes
    or overwrites them (earlier iterations stay)."""
    return [d for d in run_dirs(out, case, points, arms)
            if d.exists() and not (d.name == STRATEGY_STAGE and complete_stage(d))]


def main():
    p = argparse.ArgumentParser(description="exit 1 if a dir this run would write already exists")
    p.add_argument("command", choices=("check-new",))
    p.add_argument("--out", required=True)
    p.add_argument("--case", required=True)
    p.add_argument("--points", required=True)
    p.add_argument("--arms", required=True)
    a = p.parse_args()
    found = existing_run_dirs(a.out, a.case, a.points.split(","), a.arms.split(","))
    for d in found:
        print(f"exists: {d}", file=sys.stderr)
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main())
