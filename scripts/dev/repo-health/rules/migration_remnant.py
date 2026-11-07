"""rule migration.remnant: 移行の残骸（deprecated・legacy・互換分岐・新旧切替 flag・退役した path）の候補。

profile の語彙（`terms` の正規表現）と退役した path（`retired_paths`）を、コードの file から拾う。
1 件の finding は (file, 語) の組で、value はその file での出現数（base より増えれば worsened）。
鍵は `<path>:<語の名前>`（行番号を含めない）。静的推定で、file を読むだけ（削除・書き換えはしない）。
文書（ADR・進捗）は移行の経緯を書く場所なので既定では見ない（classes が product だけ）。

設定（profile の [rules."migration.remnant"]。共通の欄は health/scan.py）:
    classes = ["product"]
    [rules."migration.remnant".terms]    # 語の名前 = 正規表現。profile の表で既定を置き換える
    deprecated = '(?i)\\bdeprecated\\b'
    ...
    retired_paths = []                   # 退役した path・名前（文字列のまま探す）。例: 旧い config 欄・旧 script
"""

from __future__ import annotations

import re

from health.rule import Rule
from health.scan import ConfigError, Options, excerpt, hit, line_of

RULE_ID = "migration.remnant"
DEFAULT_TERMS = {
    "deprecated": r"(?i)(#\[deprecated|@deprecated|\bdeprecated\b|DeprecationWarning)",
    "legacy": r"(?i)\blegacy\b",
    "backward-compat": r"(?i)\bbackwards?[ _-]?compat(ible|ibility)?\b",
    "switch-flag": r"(?i)\b(use|enable|disable)_(new|old|legacy)_[a-z0-9_]+\b",
}
DEFAULTS = {"classes": ["product"], "terms": DEFAULT_TERMS, "retired_paths": []}


def evaluate(ctx, tree):
    opts = Options(ctx, RULE_ID, DEFAULTS)
    terms = opts["terms"]
    if not all(isinstance(v, str) for v in terms.values()):
        raise ConfigError(f"{RULE_ID}.terms の値は正規表現の文字列")
    try:
        pats = {name: re.compile(p) for name, p in sorted(terms.items())}
    except re.error as exc:
        raise ConfigError(f"{RULE_ID}.terms の正規表現が不正: {exc}") from None
    for r in opts["retired_paths"]:
        if not isinstance(r, str) or not r:
            raise ConfigError(f"{RULE_ID}.retired_paths は空でない文字列の配列")
        pats[f"retired:{r}"] = re.compile(re.escape(r))
    hits = []
    for info, text in opts.files(ctx, tree):
        if not info.is_code:
            continue
        for name, pat in pats.items():
            ms = list(pat.finditer(text))
            if not ms:
                continue
            evs = []
            for m in ms[:20]:
                line = line_of(text, m.start())
                evs.append({"path": info.path, "line": line, "ref": tree.side, "symbol": name,
                            "excerpt": excerpt(text, line)})
            hits.append(hit(opts, evs, ("file", info.path),
                            f"{info.path} に移行の残骸の語 {name}（{pat.pattern}）が {len(ms)} 回ある。"
                            "旧い経路・互換分岐・切替 flag が残っている候補（破壊的変更の方針では同じ変更で消す）。"
                            "残す理由があれば profile の例外に書く",
                            key=f"{info.path}:{name}", value=len(ms)))
    return hits


RULES = [Rule(id=RULE_ID, kind="structure", evaluate=evaluate, candidate=True, default_required=False,
              default_severity="warning")]
