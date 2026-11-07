#!/usr/bin/env python3
"""repo_health.py — 汎用 repo health 検査器（ADR 2026-11-07-repo-health-contract）。

    python3 scripts/dev/repo-health/repo_health.py --repo . --base <rev> --head <rev|worktree> \\
        --policy-version 1 [--profile <name|path>] [--rules <id,...>] --out <report.json>
    python3 scripts/dev/repo-health/repo_health.py --repo . --print-base
    python3 scripts/dev/repo-health/repo_health.py --print-review-policy --repo . --base <rev> [--profile <name|path>]

- `--base auto` は snapshot が `git merge-base HEAD refs/heads/main` に解決する（root・base の解決はここ 1 か所）。
  `--print-base` はその sha を出すだけ（既存検査の script を単独で走らせるときに使う）。
- `--print-review-policy` は base の commit の profile（extends を解決した結果）の [review] を JSON
  （celeris.repo-health.review-policy/1、schema/review-policy-v1.schema.json）で stdout に出して exit 0。
  head・作業ツリーの profile は読まない（ADR 2026-11-07-health-review-integration D2.3）。base が読めない・
  profile が誤りなら exit 2。--head・--out・--policy-version・--rules は受け付けない。

- report（celeris.repo-health.report/1）は --out にだけ書く。stdout には rule ごとの status と
  finding の数の要約を出す。
- exit code: 0 合格、1 必須 rule の fail、2 必須 rule の error・unknown・unsupported。
  report を書けないとき（使い方誤り・non-Git・git が無い・policy 版の不一致・profile の誤り）も
  exit 2 で、理由は stderr に出し --out には書かない。
- policy（profile・baseline・検査器）は base 側の内容で評価する。head が policy の file を変えれば
  policy.changed（必須）が fail になる。
- --rules は手元の確認用の絞り込み。policy.changed は外せない。
- Python 3 標準ライブラリだけを使う。外部 tool は git が必須、ほかは rule が宣言し、無ければ unsupported。
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sys

sys.dont_write_bytecode = True  # 対象 repo に __pycache__ を作って dirty にしない
HERE = os.path.dirname(os.path.abspath(__file__))
if HERE not in sys.path:
    sys.path.insert(0, HERE)  # python3 -I（-P）でも自分の package を読む

from health import engine, profile as profile_mod, snapshot  # noqa: E402
from health.snapshot import UsageError  # noqa: E402


def parse_args(argv: list[str]) -> argparse.Namespace:
    ap = argparse.ArgumentParser(prog="repo_health.py", description=__doc__.split("\n\n")[0])
    ap.add_argument("--repo", required=True, help="Git 作業ツリーの path（1 つだけ）")
    ap.add_argument("--print-base", action="store_true", help="--base auto の解決結果（sha）を出して終わる")
    ap.add_argument("--print-review-policy", action="store_true",
                    help="base の profile の [review] を JSON（celeris.repo-health.review-policy/1）で出して終わる")
    ap.add_argument("--base", help="比べる基準の commit（ref か sha）か auto（HEAD と main の merge-base）")
    ap.add_argument("--head", help="候補の commit か worktree")
    ap.add_argument("--policy-version", help="期待する policy の版（profile の version と一致すること）")
    ap.add_argument("--profile", default=None, help="profile の名前（profiles/<name>.toml）か repo 内の TOML の path")
    ap.add_argument("--rules", default=None, help="評価する rule ID のカンマ区切り（policy.changed は常に評価）")
    ap.add_argument("--out", help="report の書き先")
    args = ap.parse_args(argv)
    if args.print_review_policy:
        extra = [f"--{k.replace('_', '-')}" for k in ("print_base", "head", "policy_version", "rules", "out")
                 if getattr(args, k) not in (None, False)]
        if extra:
            ap.error(f"--print-review-policy と一緒に使えない: {', '.join(extra)}（head は読まない）")
        if args.base is None:
            ap.error("必須の引数が無い: --base")
    elif not args.print_base:
        missing = [f"--{k.replace('_', '-')}" for k in ("base", "head", "policy_version", "out")
                   if getattr(args, k) is None]
        if missing:
            ap.error(f"必須の引数が無い: {', '.join(missing)}")
    return args


def write_report(path: str, report: dict) -> None:
    text = json.dumps(report, ensure_ascii=False, indent=2) + "\n"
    parent = os.path.dirname(os.path.abspath(path))
    os.makedirs(parent, exist_ok=True)
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as fh:
        fh.write(text)
    os.replace(tmp, path)


def print_summary(report: dict) -> None:
    counts: dict[str, int] = {}
    for f in report["findings"]:
        counts[f["rule_id"]] = counts.get(f["rule_id"], 0) + 1
    snap = report["snapshot"]
    print(f"repo-health: mode={report['inputs']['mode']} base={snap['base']['sha']} head={snap['head']['sha']}"
          f"{' (dirty)' if snap['head']['dirty'] else ''}{' (shallow)' if snap['shallow'] else ''}"
          f" policy={report['policy']['source']}{' changed' if report['policy']['changed'] else ''}")
    for r in report["rules"]:
        req = "required" if r["required"] else "optional"
        reason = f" — {r['reason']}" if r["reason"] else ""
        print(f"  {r['id']:<28} {r['status']:<15} {req:<8} findings={counts.get(r['id'], 0)}{reason}")
    for f in report["findings"]:
        if f["severity"] == "error" and f["delta"] in ("new", "worsened") and f["exception"] is None:
            ev = f["evidence"][0]
            loc = f"{ev['path']}:{ev['line']}" if "line" in ev else ev["path"]
            print(f"  FAIL {f['rule_id']} {loc}: {f['rationale']}")
            for e in f["evidence"][:20]:
                if e.get("excerpt"):
                    print(f"       {e['excerpt']}")
    s = report["summary"]
    c = s["counts"]
    print(f"repo-health: verdict={s['verdict']} exit={s['exit_code']} reviewable={str(s['reviewable']).lower()}"
          f" findings={c['findings']} fail_grade={c['fail_grade']} new={c['new']} worsened={c['worsened']}"
          f" existing={c['existing']}")


def print_review_policy(args: argparse.Namespace) -> int:
    root, _sha, base_tree = snapshot.resolve_base(args.repo, args.base)
    try:
        policy = profile_mod.resolve_base(root, base_tree, args.profile).review_policy()
    finally:
        base_tree.reader.close()
    print(json.dumps(policy, ensure_ascii=False, indent=2))
    return 0


def run(args: argparse.Namespace) -> int:
    if shutil.which("git") is None:
        raise UsageError("tool_missing:git: git が PATH に無い。snapshot を決められないので report を書かない")
    if args.print_base:
        print(snapshot.default_base(snapshot.find_root(args.repo)))
        return 0
    if args.print_review_policy:
        return print_review_policy(args)
    only = [r.strip() for r in args.rules.split(",") if r.strip()] if args.rules is not None else None
    registry = engine.load_rules()
    snap = snapshot.resolve(args.repo, args.base, args.head)
    try:
        prof = None
        if snap.base_reachable:
            prof = profile_mod.resolve(snap, args.profile)
            if prof.version != args.policy_version:
                raise UsageError(f"policy_version_mismatch: profile {prof.name} の version は {prof.version}、"
                                 f"--policy-version は {args.policy_version}")
        report = engine.build_report(snap, prof, args.policy_version, args.profile, only, registry)
    finally:
        for tree in (snap.base_tree, snap.head_tree):
            if tree is not None:
                tree.cleanup()
        snap.head_tree.reader.close()
    write_report(args.out, report)
    print_summary(report)
    return report["summary"]["exit_code"]


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    try:
        return run(args)
    except UsageError as exc:
        print(f"repo-health: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
