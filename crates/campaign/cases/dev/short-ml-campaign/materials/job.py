"""リッジ回帰（numpy のみ）。λ=1.0、切片は罰則なし。入力は inputs/、出力は results/。"""
import csv
import os
import sys

import numpy as np

LAMBDA = 1.0


def read(path):
    with open(path, newline="") as f:
        r = csv.reader(f)
        head = next(r)
        return head, np.array([[float(v) for v in row] for row in r])


def main():
    head, t = read("inputs/train.csv")
    names, x, y = head[:-1], t[:, :-1], t[:, -1]
    fhead, e = read("inputs/eval-features.csv")
    mx, my = x.mean(axis=0), y.mean()
    xc, yc = x - mx, y - my
    w = np.linalg.solve(xc.T @ xc + LAMBDA * np.eye(x.shape[1]), xc.T @ yc)
    b = my - mx @ w
    rmse = float(np.sqrt(np.mean((x @ w + b - y) ** 2)))
    os.makedirs("results", exist_ok=True)
    with open("results/coefficients.csv", "w") as f:
        f.write("name,value\n")
        for n, v in zip(names, w):
            f.write(f"{n},{v:.6f}\n")
        f.write(f"intercept,{b:.6f}\n")
    with open("results/metrics.csv", "w") as f:
        f.write("name,value\n")
        f.write(f"train_rmse,{rmse:.6f}\n")
        f.write(f"n_train_rows,{len(y)}\n")
        f.write(f"numpy_version,{np.__version__}\n")
    with open("results/predictions.csv", "w") as f:
        f.write("id,pred\n")
        for row in e:
            f.write(f"{int(row[0])},{float(row[1:] @ w + b):.6f}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
