#!/usr/bin/env python3
"""long-horizon ベンチ bench_version 2 の検査 script（保護領域。worker に見せない）。

使い方: python3 -I -S check_outcome.py --case C06 --outcome o1 --tree <最終の木の絶対 path>
exit: 0 = 公開の契約を満たす、1 = 満たさない、2 = 検査を実行できない（引数の誤り・木が無い）。
stdout に {"reached": bool, "violations": [...]} を 1 行で書く。

判定の入力は木の out/<outcome>.txt だけ。LLM の回答文・controller の主張・reviewer の判断を読まない。
木の中のコードを実行も import もしない。条件は公開の契約（public/<case>/repo-seed/README.md の「成果の契約」）から導けるものだけ。
"""
import hashlib
import json
import os
import re
import stat
import sys

GRADER_MARKER = "LHB-GRADER-V2-c41e7d0a96b3"
MAX_BYTES = 65536
FIELDS = ("case", "outcome", "status", "evidence")
LINE = re.compile(r"^\s*(?:[-*]\s+)?([A-Za-z_]+)\s*[:：=]\s*(.*?)\s*$")


def unquote(value):
    if len(value) >= 2 and value[0] == value[-1] and value[0] in "`\"'":
        return value[1:-1].strip()
    return value


def from_pairs(pairs):
    """JSON の object を [(key, value)] のまま受ける（同じ key の重複を見逃さない）。"""
    return pairs


def pairs_of(text):
    """(key, value) の列。JSON の object なら欄、それ以外は行の形。値は文字列でないとき None。"""
    body = text.strip()
    if body.startswith("{"):
        try:
            obj = json.loads(body, object_pairs_hook=from_pairs)
        except ValueError:
            obj = None
        if isinstance(obj, list):
            return [(k.lower(), v if isinstance(v, str) else None) for k, v in obj]
    out = []
    for line in text.splitlines():
        m = LINE.match(line)
        if m:
            out.append((m.group(1).lower(), unquote(m.group(2))))
    return out


def judge(tree, case, outcome):
    out_dir = os.path.join(tree, "out")
    path = os.path.join(out_dir, outcome + ".txt")
    try:
        if not stat.S_ISDIR(os.lstat(out_dir).st_mode):
            return ["out_is_not_a_directory"]
        st = os.lstat(path)
    except OSError:
        return ["missing_file"]
    if not stat.S_ISREG(st.st_mode):
        return ["not_a_regular_file"]
    if st.st_size == 0:
        return ["empty_file"]
    if st.st_size > MAX_BYTES:
        return ["too_large"]
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(fd, "rb") as f:
        raw = f.read(MAX_BYTES + 1)
    try:
        text = raw.decode("utf-8").lstrip("﻿")
    except UnicodeDecodeError:
        return ["not_utf8"]
    values = {k: [] for k in FIELDS}
    for key, value in pairs_of(text):
        if key in values:
            values[key].append(value)
    violations = []
    want = {
        "case": case,
        "outcome": outcome,
        "status": "verified",
        "evidence": hashlib.sha256(f"minichunk {case} {outcome} chunk".encode()).hexdigest(),
    }
    for key in FIELDS:
        seen = values[key]
        if not seen:
            violations.append(f"{key}:missing")
        elif len(set(seen)) != 1 or seen[0] is None:
            violations.append(f"{key}:conflict")
        else:
            got = seen[0]
            if key in ("status", "evidence"):
                got = got.lower()
            if got != want[key]:
                violations.append(f"{key}:wrong")
    return violations


def main(argv):
    args = dict(zip(argv[0::2], argv[1::2]))
    case, outcome, tree = args.get("--case"), args.get("--outcome"), args.get("--tree")
    if (
        len(argv) != 6
        or not case
        or not outcome
        or not tree
        or not re.fullmatch(r"C[0-9]{2}", case)
        or not re.fullmatch(r"o[0-9]+", outcome)
        or not os.path.isabs(tree)
        or not os.path.isdir(tree)
    ):
        print(json.dumps({"error": "usage"}))
        return 2
    violations = judge(tree, case, outcome)
    print(json.dumps({"reached": not violations, "violations": violations}))
    return 0 if not violations else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
