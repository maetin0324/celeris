#!/usr/bin/env python3
"""short-ml-campaign の固定入力と評価側の参照値を作る（標準ライブラリのみ・LLM なし・seed 固定）。

使い方: python3 gen_short_ml.py <inputs-out-dir> <private-out-dir>
  inputs-out-dir : train.csv（x1..x4,y）と eval-features.csv（id,x1..x4。目的変数なし）
  private-out-dir: eval-targets.csv（id,y）と reference.json（評価側だけが読む参照値）
置いた file が正本。再生成の一致は保証しない（浮動小数点の丸めは 6 桁に固定）。
"""
import json
import random
import sys
from pathlib import Path

SEED = 20261019
N_TRAIN, N_EVAL = 1600, 400
TRUE_W = [1.5, -0.8, 0.3, 0.0]
TRUE_B = 2.0
NOISE = 0.5
LAMBDA = 1.0


def solve(a, b):
    n = len(b)
    m = [row[:] + [b[i]] for i, row in enumerate(a)]
    for c in range(n):
        p = max(range(c, n), key=lambda r: abs(m[r][c]))
        m[c], m[p] = m[p], m[c]
        for r in range(n):
            if r != c:
                f = m[r][c] / m[c][c]
                m[r] = [x - f * y for x, y in zip(m[r], m[c])]
    return [m[i][n] / m[i][i] for i in range(n)]


def ridge(x, y, lam):
    n, d = len(x), len(x[0])
    mx = [sum(r[j] for r in x) / n for j in range(d)]
    my = sum(y) / n
    xc = [[r[j] - mx[j] for j in range(d)] for r in x]
    yc = [v - my for v in y]
    a = [[sum(r[i] * r[j] for r in xc) + (lam if i == j else 0.0) for j in range(d)] for i in range(d)]
    b = [sum(r[i] * v for r, v in zip(xc, yc)) for i in range(d)]
    w = solve(a, b)
    return w, my - sum(wi * mi for wi, mi in zip(w, mx))


def rmse(x, y, w, b):
    return (sum((sum(wi * xi for wi, xi in zip(w, r)) + b - v) ** 2 for r, v in zip(x, y)) / len(y)) ** 0.5


def main(inp, priv):
    rng = random.Random(SEED)

    def rows(n):
        x = [[round(rng.gauss(0, 1), 6) for _ in range(4)] for _ in range(n)]
        y = [round(TRUE_B + sum(w * v for w, v in zip(TRUE_W, r)) + rng.gauss(0, NOISE), 6) for r in x]
        return x, y

    xt, yt = rows(N_TRAIN)
    xe, ye = rows(N_EVAL)
    inp, priv = Path(inp), Path(priv)
    inp.mkdir(parents=True, exist_ok=True)
    priv.mkdir(parents=True, exist_ok=True)
    (inp / "train.csv").write_text("x1,x2,x3,x4,y\n" + "".join(
        ",".join(f"{v:.6f}" for v in r) + f",{y:.6f}\n" for r, y in zip(xt, yt)))
    (inp / "eval-features.csv").write_text("id,x1,x2,x3,x4\n" + "".join(
        f"{i}," + ",".join(f"{v:.6f}" for v in r) + "\n" for i, r in enumerate(xe)))
    (priv / "eval-targets.csv").write_text("id,y\n" + "".join(f"{i},{y:.6f}\n" for i, y in enumerate(ye)))
    w, b = ridge(xt, yt, LAMBDA)
    (priv / "reference.json").write_text(json.dumps({
        "seed": SEED, "lambda": LAMBDA,
        "coef": {f"x{i + 1}": round(v, 6) for i, v in enumerate(w)}, "intercept": round(b, 6),
        "train_rmse": round(rmse(xt, yt, w, b), 6), "eval_rmse": round(rmse(xe, ye, w, b), 6),
    }, indent=2) + "\n")


if __name__ == "__main__":
    main(*sys.argv[1:3])
