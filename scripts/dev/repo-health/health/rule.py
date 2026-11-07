"""rule の登録口。

rule は `scripts/dev/repo-health/rules/` の module に置き、module 変数 `RULES`（`Rule` の list）で
登録する。engine は rules/ の module を名前順に読み込み、同じ rule ID の二重登録は crash にする。

    from health.rule import Hit, Rule

    def evaluate(ctx, tree):          # tree は base か head の Tree。両側で呼ばれる
        return [Hit(evidence=[{"path": "a.rs", "line": 3}], scope=("file", "a.rs"),
                    rationale="...", value=12, threshold=10)]

    RULES = [Rule(id="size.responsibility-growth", kind="structure", adapter="rust",
                  languages=("rust",), evaluate=evaluate)]

- kind: structure（doc-only で not_applicable）・docs（doc-only でも評価）・policy（core が使う）
- languages: 対象の言語。空なら言語を問わない。head の対象 file にその言語が 1 つも無ければ not_applicable
- tools: 要る外部 tool の名前（PATH で探す）。無ければ unsupported（tool_missing:<name>）
- candidate: 静的推定なら True。severity は warning 以下に切り下げる
- head_only: base では評価しない（delta は常に new）。policy.changed のように差分そのものを見る rule
- 判定できないときは `raise Unknown("<reason>")`、評価できない環境なら `raise Unsupported("<reason>")`。
  それ以外の例外は error（crashed:<rule_id>:<例外の型>）。
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Callable


class Unknown(Exception):
    """評価は終わったが判定できない（baseline が読めない など）。args[0] は reason。"""


class Unsupported(Exception):
    """この環境では評価できない。args[0] は reason（tool_missing:<name> など）。"""


@dataclass
class Hit:
    """rule が返す finding の素。fingerprint・delta・snapshot・exception は engine が埋める。"""

    evidence: list[dict]
    scope: tuple[str, str]
    rationale: str
    severity: str | None = None  # 省略時は rule の default_severity
    key: str | None = None  # fingerprint の鍵。省略時は evidence の (path, symbol) から作る
    value: float | None = None  # 悪化を比べる値（大きいほど悪い）
    threshold: float | None = None


@dataclass
class Rule:
    id: str
    kind: str  # structure | docs | policy
    evaluate: Callable
    adapter: str = "generic"  # report の rules[].adapter（rust・typescript・docs・generic）
    languages: tuple[str, ...] = ()
    tools: tuple[str, ...] = ()
    candidate: bool = False
    default_required: bool = True
    default_severity: str = "error"
    head_only: bool = False
    wraps: str | None = None
    extra: dict = field(default_factory=dict)
