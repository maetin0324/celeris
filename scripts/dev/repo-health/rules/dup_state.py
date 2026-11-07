"""rule dup.state: 同じ状態（SQL の表）を複数の module が書いている候補。

コードの中の SQL の書き込み文（`INSERT [OR ...] INTO <表>`・`REPLACE INTO <表>`・`UPDATE <表> SET`・
`DELETE FROM <表>`）を拾い、表ごとに書き手の module を数える。書き手が `min_writers` 以上なら
finding（鍵は `table:<表>`、value は書き手の module の数）。module の単位は `group_by`。
静的推定で、file を読むだけ（削除・書き換えはしない）。

設定（profile の [rules."dup.state"]。共通の欄は health/scan.py）:
    classes = ["product"]      # 既定。試験・migration・fixture の書き込みは数えない
    group_by = "dir"           # "dir"（親 dir を 1 module）か "file"
    min_writers = 2
    case_sensitive = true      # true なら大文字の SQL keyword だけ拾う（文章の update などを拾わない）
    ignore_tables = []         # 数えない表（共有の監査表など。理由は profile の comment か例外に書く）
"""

from __future__ import annotations

import re
from collections import defaultdict

from health.rule import Rule
from health.scan import ConfigError, Options, excerpt, hit, line_of, module_of

RULE_ID = "dup.state"
DEFAULTS = {"classes": ["product"], "group_by": "dir", "min_writers": 2, "case_sensitive": True,
            "ignore_tables": []}
LANGUAGES = ("rust", "python", "typescript", "shell")
_TABLE = r"[\"`\[]?([A-Za-z_][A-Za-z0-9_]*)[\"`\]]?"
_WRITE = (
    r"\b(?:INSERT(?:\s+OR\s+[A-Z]+)?\s+INTO|REPLACE\s+INTO|DELETE\s+FROM)\s+" + _TABLE,
    r"\bUPDATE\s+(?:OR\s+[A-Z]+\s+)?" + _TABLE + r"\s+SET\b",
)


def evaluate(ctx, tree):
    opts = Options(ctx, RULE_ID, DEFAULTS)
    if opts["group_by"] not in ("dir", "file"):
        raise ConfigError(f"{RULE_ID}.group_by は dir か file")
    flags = 0 if opts["case_sensitive"] else re.IGNORECASE
    pats = [re.compile(p, flags) for p in _WRITE]
    ignore = {t.lower() for t in opts["ignore_tables"]}
    writers: dict[str, dict[str, list[dict]]] = defaultdict(lambda: defaultdict(list))
    for info, text in opts.files(ctx, tree, languages=LANGUAGES):
        for pat in pats:
            for m in pat.finditer(text):
                table = m.group(1).lower()
                if table in ignore or table in ("select", "values", "set", "where"):
                    continue
                line = line_of(text, m.start())
                writers[table][module_of(info.path, opts["group_by"])].append(
                    {"path": info.path, "line": line, "ref": tree.side, "symbol": table,
                     "excerpt": excerpt(text, line)})
    hits = []
    for table, mods in sorted(writers.items()):
        if len(mods) < opts["min_writers"]:
            continue
        evs = [e for m in sorted(mods) for e in mods[m]]
        hits.append(hit(opts, evs, ("repo", f"table:{table}"),
                        f"表 {table} を {len(mods)} 個の module（{opts['group_by']} 単位: {', '.join(sorted(mods))}）が"
                        f"書いている（閾値 {opts['min_writers']}）。状態の書き手が 1 つに定まっていない候補",
                        key=f"table:{table}", value=len(mods), threshold=opts["min_writers"]))
    return hits


RULES = [Rule(id=RULE_ID, kind="structure", evaluate=evaluate, languages=LANGUAGES, candidate=True,
              default_required=False, default_severity="warning")]
