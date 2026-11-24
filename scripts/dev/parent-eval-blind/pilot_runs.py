"""Resolve pilot iteration roots identically for comparison and blind scoring."""
import pathlib


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
