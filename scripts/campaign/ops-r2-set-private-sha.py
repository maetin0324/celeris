#!/usr/bin/env python3
"""公開部 case.json の private_ref.sha256 を、人が承認票に書いた新しい private の sha256 に替える。

private の中身・path は読まない（入力は sha256 の文字列だけ）。漏洩済みの旧 sha256 は拒否する。
manifest の再 freeze の要否も判定する: frozen manifest が固定する materials_sha256 は
Case::public_json（private_ref・grade_case_ref を除いた公開部）から作るので、private_ref.sha256 だけを
替えた変更では公開部が変わらない。公開部が変わったら（= sha256 以外も変わったら）止める。

usage: ops-r2-set-private-sha.py --case <id> --sha256 <hex> [--cases-root crates/campaign/cases] [--dry-run]
"""
import argparse
import json
import os
import re
import sys

# 漏洩済みの旧 private の sha256（git の履歴と clone に残る。ADR 付記 2026-10-30「K2 の private の作り直し」）。
LEAKED = {
    "k2-bf-xattr-restart-campaign": {"d02126077a47495b121e1e678f055ba4e2493f92d05cd010f2bda32d30694760"},
}


def public_part(case):
    return {k: v for k, v in case.items() if k not in ("private_ref", "grade_case_ref")}


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("--case", required=True)
    ap.add_argument("--sha256", required=True)
    ap.add_argument("--cases-root", default="crates/campaign/cases")
    ap.add_argument("--dry-run", action="store_true")
    a = ap.parse_args(argv)
    sha = a.sha256.strip().lower()
    if not re.fullmatch(r"[0-9a-f]{64}", sha):
        sys.stderr.write("--sha256 must be 64 hex chars\n")
        return 64
    if not re.fullmatch(r"[a-z0-9][a-z0-9-]*", a.case):
        sys.stderr.write("invalid case id\n")
        return 64
    if sha in LEAKED.get(a.case, set()):
        sys.stderr.write("refused: this sha256 is the leaked old private\n")
        return 65
    path = os.path.join(a.cases_root, "final", a.case, "case.json")
    with open(path, encoding="utf-8") as f:
        text = f.read()
    case = json.loads(text)
    old = case["private_ref"]["sha256"]
    if text.count(old) != 1:
        sys.stderr.write("old sha256 must appear exactly once in case.json\n")
        return 65
    new = json.loads(text)
    new["private_ref"]["sha256"] = sha
    if public_part(new) != public_part(case):
        sys.stderr.write("public part changed; manifest re-freeze needed\n")
        return 65
    out = text.replace(old, sha)
    if json.loads(out) != new:
        sys.stderr.write("rewrite would change more than private_ref.sha256\n")
        return 65
    print(json.dumps({"case": a.case, "old_sha256": old, "new_sha256": sha,
                      "changed": old != sha, "manifest_refreeze": False,
                      "reason": "materials_sha256 excludes private_ref"}, ensure_ascii=False))
    if not a.dry_run and old != sha:
        tmp = path + ".tmp"
        with open(tmp, "w", encoding="utf-8") as f:
            f.write(out)
        os.replace(tmp, path)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
