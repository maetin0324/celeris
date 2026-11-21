#!/usr/bin/env python3
"""研究ケース: BenchFS ISC27 task C（01M7C56WK4E86BTZNDKAX7SP7C）の Sirius の node 停止とアプリ同居（c-results.md 11.15〜11.21 節）。

新しい実験はしない。task C が 2026-11-16 に残した c-results.md を読むだけ（sha256 で版を固定）。
  python3 -I <dir>/research_case.py verify [--src <c-results.md>]
時点 k の slice = 11.15 節の始まりから 11.k 節の終わりまで（その時点の記録）。後の節にだけある結果の語（sentinel）が
前の slice に無いことと、否定対照（11.17 の slice に 16 節の総括を混ぜる）が検出されることを確かめる。
データは agent-platform の repo に写さない（未発表の論文の測定。外部送信は既定で不可）。
"""
import hashlib
import re
import sys
from pathlib import Path

SRC = "/local/celeris/data/workspaces/01M7C56WK4E86BTZNDKAX7SP7C/artifacts/c-results.md"
SHA = "2830ce8242fd4d3ba538422ef5dc6c847e467eaebc3d25983109149423d264ea"
POINTS = ["11.15", "11.16", "11.17", "11.18", "11.19", "11.20", "11.21"]
# 11.20 節（job 50183 の結果）で初めて現れる語。前の slice にあれば未来の漏れ。
SENTINELS = {"1 / 488": "11.20", "16 件目": "11.20", "a19 が 2026-11-16 21:28": "11.20"}


def sections(text):
    heads = [(m.start(), m.group(1)) for m in re.finditer(r"^#{2,3} (\d+(?:\.\d+)?)\.? ", text, re.M)]
    out = {}
    for i, (pos, num) in enumerate(heads):
        end = heads[i + 1][0] if i + 1 < len(heads) else len(text)
        out[num] = text[pos:end]
    return out


def slices(secs):
    acc, res = "", {}
    for p in POINTS:
        acc += secs[p]
        res[p] = acc
    return res


def leaks(point, text):
    bad = []
    for s, first in SENTINELS.items():
        if POINTS.index(point) < POINTS.index(first) and s in text:
            bad.append(f"{point}: {s!r}（初出 {first}）")
    return bad


def main():
    src = sys.argv[sys.argv.index("--src") + 1] if "--src" in sys.argv else SRC
    data = Path(src).read_bytes()
    got = hashlib.sha256(data).hexdigest()
    if got != SHA:
        print(f"FAIL sha256 {got} != {SHA}（版が違う）")
        return 1
    secs = sections(data.decode("utf-8"))
    sl = slices(secs)
    ok = True
    for p, t in sl.items():
        b = leaks(p, t)
        d = hashlib.sha256(t.encode()).hexdigest()
        print(f"{'FAIL' if b else 'ok  '} slice {p}: {len(t.splitlines())} 行 sha256 {d}" + (f" leak {b}" if b else ""))
        ok &= not b
    # 否定対照: 11.17 の slice に 16 節（停止の総括）を混ぜると検出されること
    neg = leaks("11.17", sl["11.17"] + secs["16"])
    print(("ok   " if neg else "FAIL ") + f"negative control 11.17+§16: {len(neg)} leaks detected")
    ok &= bool(neg)
    print("research-case verify:", "ok" if ok else "FAILED")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
