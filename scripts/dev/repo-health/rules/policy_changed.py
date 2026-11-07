"""rule policy.changed（ADR D2）: head が policy の file（profile・baseline・検査器）を base から変えたか。

評価に使う policy は base 側。head の変更は別の review で承認する。base に profile が無い初回
（bootstrap）は severity warning で合格を妨げない。
"""

from __future__ import annotations

from health.rule import Hit, Rule


def evaluate(ctx, tree):
    changed = [f for f in ctx.policy_files if f["base_blob"] != f["head_blob"]]
    if not changed:
        return []
    evidence = [{"path": f["path"], "ref": "head" if f["head_blob"] else "base"} for f in changed]
    bootstrap = ctx.profile.policy_source == "bootstrap"
    names = ", ".join(f["path"] for f in changed)
    if bootstrap:
        why = (f"base に profile が無い初回（bootstrap）。head の policy で評価した。変えた policy の file: {names}。"
               "初回の基準として扱い、次からは base 側で評価する")
    else:
        why = (f"head が policy の file を変えた（base blob と head blob が違う）: {names}。"
               f"評価は {ctx.profile.policy_source} 側の内容で行った。policy の変更は別の review で承認する")
    return [Hit(
        evidence=evidence,
        scope=("repo", "policy"),
        rationale=why,
        severity="warning" if bootstrap else "error",
        key="policy-files:" + ",".join(f["path"] for f in changed),
    )]


RULES = [Rule(id="policy.changed", kind="policy", evaluate=evaluate, head_only=True)]
