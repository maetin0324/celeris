#!/usr/bin/env python3
"""健全性 v2 B0 dup-candidates の機械的な走査（読むだけ。build も試験もしない）。

出すもの（JSON を stdout へ）:
- same_name_types: 2 か所以上で定義された pub struct/enum の名前と file:line（同名 = 候補の入口。判断は人/agent）
- unreferenced_pub: pub item の名前が、定義の行を除き tracked の *.rs・*.ts・*.md（docs/ を除く）に 1 度も出ない
- unreferenced_scripts: scripts/ の tracked file の basename が、自分以外の tracked file に 1 度も出ない

参照検索の手順（再現）:
  1. `git ls-files` で tracked file を得る（crates/script-runtime/vendor/ は除く）
  2. pub item の定義を正規表現 `^\\s*pub(\\([a-z]+\\))? (fn|struct|enum|trait|const|static|type) NAME` で拾う
  3. NAME を単語境界 `\\bNAME\\b` で全 *.rs（vendor 除く）・control-ts/web の *.ts/*.tsx から数え、
     定義の行の 1 回を引いて 0 なら未参照の候補にする
  4. 候補は agent が `git grep -n -w NAME` で再確認してから dup-candidates.json に載せる
     （serde/clap の derive・trait の実装・macro・schema 生成で名前を書かずに使う item は誤検出になる）
使い方: python3 dup-candidates-scan.py > /tmp/scan.json  （repo の根で実行）
"""
import collections
import json
import re
import subprocess
import sys


def tracked():
    out = subprocess.run(["git", "ls-files"], capture_output=True, text=True, check=True).stdout
    return [p for p in out.splitlines() if "/vendor/" not in p]


DEF = re.compile(r"^\s*pub(?:\([a-z]+\))? (?:async )?(fn|struct|enum|trait|const|static|type) ([A-Za-z_][A-Za-z0-9_]*)")
TYPE = re.compile(r"^\s*pub(?:\([a-z]+\))? (struct|enum) ([A-Z][A-Za-z0-9]*)")
WORD = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")


def main():
    files = tracked()
    rs = [p for p in files if p.endswith(".rs") and p.startswith("crates/")]
    ts = [p for p in files if p.endswith((".ts", ".tsx", ".mjs")) and p.startswith(("web/", "control-ts/"))]
    counts = collections.Counter()
    defs = []
    types = collections.defaultdict(list)
    for p in rs + ts:
        try:
            text = open(p, encoding="utf-8").read()
        except (OSError, UnicodeDecodeError):
            continue
        counts.update(WORD.findall(text))
        if not p.endswith(".rs"):
            continue
        is_test = "/tests/" in p or p.endswith(("_tests.rs", "/tests.rs"))
        for i, line in enumerate(text.splitlines(), 1):
            m = DEF.match(line)
            if m and not is_test:
                defs.append((m.group(2), m.group(1), f"{p}:{i}"))
            t = TYPE.match(line)
            if t:
                types[t.group(2)].append(f"{p}:{i}")
    same = {k: v for k, v in sorted(types.items()) if len(v) >= 2}
    unref = [
        {"name": n, "kind": k, "at": at}
        for n, k, at in defs
        if counts[n] <= 1 and n not in ("main", "new", "default")
    ]
    scripts = [p for p in files if p.startswith("scripts/") and not p.endswith((".md",))]
    blob = {}
    for p in files:
        try:
            blob[p] = open(p, encoding="utf-8").read()
        except (OSError, UnicodeDecodeError):
            pass
    unref_scripts = []
    for s in scripts:
        base = s.rsplit("/", 1)[-1]
        stem = base.rsplit(".", 1)[0]
        hit = any((base in t or (len(stem) > 6 and stem in t)) for p, t in blob.items() if p != s)
        if not hit:
            unref_scripts.append(s)
    json.dump(
        {
            "schema": "celeris.health-v2-b0.dup-candidates-scan/1",
            "rs_files": len(rs),
            "ts_files": len(ts),
            "same_name_types": same,
            "unreferenced_pub": unref,
            "unreferenced_scripts": unref_scripts,
        },
        sys.stdout,
        ensure_ascii=False,
        indent=1,
    )


if __name__ == "__main__":
    main()
