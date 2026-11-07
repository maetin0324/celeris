"""rule link.unlinked: どこからも参照されない pub item・script・文書の候補。

拾うもの（静的推定。削除・書き換えはしない）:
- item: rust の `pub fn/struct/enum/trait/const/static/type`（`pub(crate)` などを含む）、typescript の
  `export function/const/let/class/interface/type/enum` の名前が、追跡 file 全体で定義の 1 回しか
  出てこない（単語境界で数える）。serde・derive・macro・trait 実装で名前を書かずに使う item は誤検出に
  なる（候補として出すだけ）。
- script: `script_paths` に当たる file の名前（basename、拡張子なしの名前が 7 文字以上ならそれも）が、
  自分以外の追跡 file に出てこない。
- doc: `doc_paths` に当たる markdown の名前が、自分以外の追跡 file に出てこない。
`entry_points` に当たる file（README・index・main など）は script・doc として数えない。
鍵は `item:<path>:<名前>`・`script:<path>`・`doc:<path>`。

設定（profile の [rules."link.unlinked"]。共通の欄は health/scan.py）:
    classes = ["product"]         # item を拾う file の種類
    kinds = ["item", "script", "doc"]
    script_paths = ['^scripts/']
    doc_paths = ['^docs/']
    entry_points = ['(^|/)README\\.md$', '(^|/)(index|main|lib|mod|__init__|__main__)\\.[a-z]+$']
    ignore_names = ["main", "new", "default"]
"""

from __future__ import annotations

import re
from collections import Counter

from health.rule import Rule
from health.scan import ConfigError, Options, excerpt, hit, line_of

RULE_ID = "link.unlinked"
DEFAULTS = {
    "classes": ["product"],
    "kinds": ["item", "script", "doc"],
    "script_paths": ["^scripts/"],
    "doc_paths": ["^docs/"],
    "entry_points": [r"(^|/)README\.md$", r"(^|/)(index|main|lib|mod|__init__|__main__)\.[a-z]+$"],
    "ignore_names": ["main", "new", "default"],
}
KINDS = ("item", "script", "doc")
_ITEM = {
    "rust": re.compile(r"(?m)^[ \t]*pub(?:\([a-z:]+\))?[ \t]+(?:async[ \t]+)?(?:unsafe[ \t]+)?"
                       r"(?:fn|struct|enum|trait|const|static|type)[ \t]+([A-Za-z_][A-Za-z0-9_]*)"),
    "typescript": re.compile(r"(?m)^export[ \t]+(?:declare[ \t]+)?(?:async[ \t]+)?"
                             r"(?:function\*?|const|let|class|interface|type|enum)[ \t]+([A-Za-z_$][\w$]*)"),
}
_WORD = re.compile(r"[A-Za-z_$][A-Za-z0-9_$]*")
_TOKEN = re.compile(r"[A-Za-z0-9_.\-]+")


def _index(ctx, tree):
    """(単語の出現数, 各 token を含む file の数, file ごとの token 集合)。excluded・generated を除く全追跡 file。"""
    words: Counter = Counter()
    doc_freq: Counter = Counter()
    per_file: dict[str, set[str]] = {}
    for path in tree.files():
        if ctx.excluded(path) or ctx.info(tree, path).cls == "generated":
            continue
        text = tree.read_text(path)
        if text is None:
            continue
        words.update(_WORD.findall(text))
        toks = set(_TOKEN.findall(text))
        toks.update(t.strip(".-") for t in list(toks))
        per_file[path] = toks
        doc_freq.update(toks)
    return words, doc_freq, per_file


def _names(path: str) -> list[str]:
    base = path.rsplit("/", 1)[-1]
    stem = base.rsplit(".", 1)[0] if "." in base else base
    return [base] + ([stem] if stem != base and len(stem) > 6 else [])


def evaluate(ctx, tree):
    opts = Options(ctx, RULE_ID, DEFAULTS)
    bad = sorted(set(opts["kinds"]) - set(KINDS))
    if bad:
        raise ConfigError(f"{RULE_ID}.kinds に未知の種類 {bad}")
    words, doc_freq, per_file = _index(ctx, tree)
    ignore = set(opts["ignore_names"])
    entry = opts.regexes("entry_points")
    hits = []
    if "item" in opts["kinds"]:
        for info, text in opts.files(ctx, tree, languages=tuple(_ITEM)):
            for m in _ITEM[info.language].finditer(text):
                name = m.group(1)
                if name in ignore or words[name] > 1:
                    continue
                line = line_of(text, m.start())
                hits.append(hit(opts, [{"path": info.path, "line": line, "ref": tree.side, "symbol": name,
                                        "excerpt": excerpt(text, line)}], ("file", info.path),
                                f"公開の item {name} の名前が追跡 file 全体で定義の 1 回しか出てこない。"
                                "未使用の候補（derive・macro・trait 実装で名前を書かずに使う場合は誤検出）",
                                key=f"item:{info.path}:{name}"))
    for kind, key in (("script", "script_paths"), ("doc", "doc_paths")):
        if kind not in opts["kinds"]:
            continue
        pats = opts.regexes(key)
        for path in sorted(per_file):
            if not any(p.search(path) for p in pats) or any(p.search(path) for p in entry):
                continue
            if not opts.wants(path) or (kind == "doc" and ctx.info(tree, path).language != "markdown"):
                continue
            if kind == "script" and ctx.info(tree, path).language == "markdown":
                continue
            own = per_file[path]
            names = _names(path)
            if any(doc_freq[n] - (1 if n in own else 0) > 0 for n in names):
                continue
            hits.append(hit(opts, [{"path": path, "ref": tree.side}], ("file" if kind == "script" else "doc", path),
                            f"{kind} {path} の名前（{', '.join(names)}）が自分以外の追跡 file に出てこない。"
                            "どこからも呼ばれない・読まれない候補",
                            key=f"{kind}:{path}"))
    return hits


RULES = [Rule(id=RULE_ID, kind="structure", evaluate=evaluate, candidate=True, default_required=False,
              default_severity="warning")]
