"""rule dup.test-fixture: 内容がほぼ同じ試験 file・fixture の候補。

試験・fixture の種類の file を、前後の空白を除いた行の集合で比べる。`min_line_len` 文字以上の行だけを
使い、`max_line_files` 個より多い file に出る行（import・定型句）は比べない。2 つの file の行の集合の
Jaccard 係数が `similarity` 以上なら同じ組。組は連結成分にまとめ、1 件の finding にする
（鍵は組の path の並び）。静的推定で、file を読むだけ（削除・書き換えはしない）。

設定（profile の [rules."dup.test-fixture"]。共通の欄は health/scan.py）:
    classes = ["test", "fixture"]
    similarity = 0.9
    min_lines = 5          # 比べる行がこれより少ない file は数えない
    min_line_len = 8
    max_line_files = 20
"""

from __future__ import annotations

from collections import defaultdict

from health.rule import Rule
from health.scan import Options, hit

RULE_ID = "dup.test-fixture"
DEFAULTS = {"classes": ["test", "fixture"], "similarity": 0.9, "min_lines": 5, "min_line_len": 8,
            "max_line_files": 20}


def _find(parent: dict[str, str], x: str) -> str:
    while parent[x] != x:
        parent[x] = parent[parent[x]]
        x = parent[x]
    return x


def evaluate(ctx, tree):
    opts = Options(ctx, RULE_ID, DEFAULTS)
    lines: dict[str, set[str]] = {}
    for info, text in opts.files(ctx, tree):
        s = {ln.strip() for ln in text.splitlines() if len(ln.strip()) >= opts["min_line_len"]}
        if len(s) >= opts["min_lines"]:
            lines[info.path] = s
    index: dict[str, list[str]] = defaultdict(list)
    for path in sorted(lines):
        for ln in lines[path]:
            index[ln].append(path)
    shared: set[tuple[str, str]] = set()
    for paths in index.values():
        if len(paths) < 2 or len(paths) > opts["max_line_files"]:
            continue
        for i, a in enumerate(paths):
            for b in paths[i + 1:]:
                shared.add((a, b))
    limit = opts["max_line_files"]
    usable = {p: {ln for ln in s if len(index[ln]) <= limit} for p, s in lines.items()}
    parent = {p: p for p in lines}
    best: dict[tuple[str, str], float] = {}
    for a, b in sorted(shared):
        common = usable[a] & usable[b]
        union = len(usable[a] | usable[b])
        if not union or len(common) < opts["min_lines"]:
            continue
        sim = len(common) / union
        if sim >= opts["similarity"]:
            best[(a, b)] = sim
            parent[_find(parent, a)] = _find(parent, b)
    groups: dict[str, list[str]] = defaultdict(list)
    for p in sorted(lines):
        groups[_find(parent, p)].append(p)
    hits = []
    for members in sorted(groups.values()):
        if len(members) < 2:
            continue
        sims = [v for (a, b), v in best.items() if a in members]
        evs = [{"path": p, "ref": tree.side} for p in members]
        hits.append(hit(opts, evs, ("repo", "test-fixture:" + members[0]),
                        f"試験・fixture の file {len(members)} 個（{', '.join(members)}）の行の集合がほぼ同じ"
                        f"（Jaccard 係数の最小 {min(sims):.2f}、閾値 {opts['similarity']}）。"
                        "共通の fixture か helper にまとめられる候補",
                        key="group:" + ",".join(members), threshold=opts["similarity"]))
    return hits


RULES = [Rule(id=RULE_ID, kind="structure", evaluate=evaluate, candidate=True, default_required=False,
              default_severity="warning")]
