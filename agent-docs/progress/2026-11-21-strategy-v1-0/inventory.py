#!/usr/bin/env python3
"""inventory.tsv の各行を基準 commit で確かめ、file:line の表（inventory.md の表の部分）を出す。

  python3 -I <dir>/inventory.py           # 表を stdout に出す（行番号は BASE の file で最初に pattern に合う行）
  python3 -I <dir>/inventory.py --check   # reuse・partial・risk は BASE と HEAD の両方で行が引けること、
                                          # missing は BASE の crates/・control-ts/・web/・scripts/ で 0 件であることを確かめる
"""
import re
import subprocess
import sys
from pathlib import Path

BASE = "7f69057907ad649829ec5a08b5ac7c892b9fe055"
HERE = Path(__file__).resolve().parent


def show(rev, path):
    r = subprocess.run(["git", "show", f"{rev}:{path}"], capture_output=True, text=True)
    return r.stdout if r.returncode == 0 else None


def line_of(rev, path, pattern):
    text = show(rev, path)
    if text is None:
        return None
    rx = re.compile(pattern)
    for i, line in enumerate(text.splitlines(), 1):
        if rx.search(line):
            return i
    return None


def main():
    check = "--check" in sys.argv
    rows = [l.split("\t") for l in (HERE / "inventory.tsv").read_text(encoding="utf-8").splitlines()[1:] if l.strip()]
    bad = 0
    out = ["| id | 状態 | 領域 | 場所（基準 commit の file:line） | 説明 |", "|---|---|---|---|---|"]
    for rid, status, area, path, pattern, note in rows:
        if status == "missing":
            r = subprocess.run(["git", "grep", "-P", "-n", pattern, BASE, "--", "crates", "control-ts", "web", "scripts"],
                               capture_output=True, text=True)
            hits = [l for l in r.stdout.splitlines() if l]
            loc = f"`git grep -P '{pattern}'` → {len(hits)} 件".replace("|", "\\|")
            if hits:
                bad += 1
                print(f"FAIL {rid}: missing のはずが {len(hits)} 件: {hits[0]}", file=sys.stderr)
        else:
            n = line_of(BASE, path, pattern)
            h = line_of("HEAD", path, pattern)
            loc = f"`{path}:{n}`"
            if n is None or (check and h is None):
                bad += 1
                print(f"FAIL {rid}: {path} に {pattern!r} が無い（BASE {n}・HEAD {h}）", file=sys.stderr)
        out.append(f"| {rid} | {status} | {area} | {loc} | {note} |")
    if not check:
        print("\n".join(out))
    print(f"inventory: {len(rows)} 行, 失敗 {bad}", file=sys.stderr)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
