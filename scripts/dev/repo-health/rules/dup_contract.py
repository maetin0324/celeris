"""rule dup.contract: 同じ契約（型・schema）の定義が 2 か所以上にある候補。

拾うもの（静的推定。削除・書き換えはしない）:
- 同名: rust の `pub struct/enum/type`、python の top level の `class`、typescript の
  `export interface/type/class/enum` が、同じ言語で 2 つ以上の file にある。鍵は `name:<言語>:<名前>`。
- 同形: field の名前の集合が同じ（`min_fields` 個以上）型が 2 つ以上の file にある。名前は違ってよい。
  鍵は `shape:<言語>:<field の名前の並び>`。
- schema file: `*.schema.json` の `title` が同じ file が 2 つ以上。
1 件の finding は 1 つの名前（形）で、value は定義の数（base より増えれば worsened）。

設定（profile の [rules."dup.contract"]。共通の欄は health/scan.py）:
    classes = ["product"]      # 既定。試験・fixture の型は数えない
    min_fields = 3             # 同形とみなす最小の field 数
    ignore_names = []          # 同名の重複として扱わない名前（Args・Error など、慣習で各所に置く名前）
    shape = true               # 同形の検出を行うか
"""

from __future__ import annotations

import json
import re
from collections import defaultdict

from health.rule import Rule
from health.scan import Options, excerpt, hit, line_of, strip_comments

RULE_ID = "dup.contract"
DEFAULTS = {"classes": ["product"], "min_fields": 3, "ignore_names": [], "shape": True}

_DEF = {
    "rust": re.compile(r"(?m)^[ \t]*pub[ \t]+(struct|enum|type)[ \t]+([A-Z][A-Za-z0-9_]*)"),
    "python": re.compile(r"(?m)^class[ \t]+([A-Z][A-Za-z0-9_]*)"),
    "typescript": re.compile(
        r"(?m)^[ \t]*export[ \t]+(?:default[ \t]+)?(?:declare[ \t]+)?(?:abstract[ \t]+)?"
        r"(interface|type|class|enum)[ \t]+([A-Z][A-Za-z0-9_]*)"),
}
_RUST_FIELD = re.compile(r"(?m)^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?([a-z_][a-z0-9_]*)[ \t]*:")
_TS_FIELD = re.compile(r"(?m)^[ \t]*(?:readonly[ \t]+)?([A-Za-z_$][A-Za-z0-9_$]*)\??[ \t]*:")
_PY_FIELD = re.compile(r"(?m)^[ \t]+([a-z_][a-z0-9_]*)[ \t]*:[^=\n]*(?:=|$)")


def _brace_body(text: str, start: int) -> str | None:
    """start 以降の最初の `{` から対応する `}` までの中身（`;` が先に来れば None）。"""
    i = start
    while i < len(text) and text[i] not in "{;":
        if text[i] == "(":
            return None  # tuple struct
        i += 1
    if i >= len(text) or text[i] != "{":
        return None
    depth = 0
    for j in range(i, len(text)):
        if text[j] == "{":
            depth += 1
        elif text[j] == "}":
            depth -= 1
            if depth == 0:
                return text[i + 1:j]
    return None


def _top_level(body: str) -> str:
    """入れ子の `{...}` を消し、1 段目の行だけ残す。"""
    out, depth = [], 0
    for ch in body:
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
        elif depth == 0:
            out.append(ch)
    return "".join(out)


def _fields(language: str, text: str, m: re.Match) -> tuple[str, ...]:
    if language == "python":
        lines = text[m.end():].split("\n")[1:]
        body = []
        for line in lines:
            if line.strip() and not line.startswith((" ", "\t")):
                break
            body.append(line)
        indents = [len(x) - len(x.lstrip()) for x in body if x.strip()]
        if not indents:
            return ()
        level = indents[0]
        top = [x for x in body if x.strip() and len(x) - len(x.lstrip()) == level]
        names = [n for n in _PY_FIELD.findall("\n".join(top)) if not n.startswith("_")]
        return tuple(sorted(set(names)))
    kind = m.group(1)
    if kind not in ("struct", "interface", "class", "type"):
        return ()
    body = _brace_body(text, m.end())
    if body is None:
        return ()
    pat = _RUST_FIELD if language == "rust" else _TS_FIELD
    return tuple(sorted(set(pat.findall(_top_level(body)))))


def evaluate(ctx, tree):
    opts = Options(ctx, RULE_ID, DEFAULTS)
    ignore = set(opts["ignore_names"])
    by_name: dict[tuple[str, str], list[dict]] = defaultdict(list)
    by_shape: dict[tuple[str, tuple[str, ...]], list[dict]] = defaultdict(list)
    for info, raw in opts.files(ctx, tree, languages=tuple(_DEF)):
        text = strip_comments(raw, info.language)
        for m in _DEF[info.language].finditer(text):
            name = m.group(m.lastindex or 1)
            line = line_of(text, m.start())
            ev = {"path": info.path, "line": line, "ref": tree.side, "symbol": name,
                  "excerpt": excerpt(raw, line)}
            if name not in ignore:
                by_name[(info.language, name)].append(ev)
            if opts["shape"]:
                fields = _fields(info.language, text, m)
                if len(fields) >= opts["min_fields"]:
                    by_shape[(info.language, fields)].append(ev)
    for info, raw in opts.files(ctx, tree, classes=opts["classes"]):
        if not info.path.endswith(".schema.json"):
            continue
        try:
            title = json.loads(raw).get("title")
        except (ValueError, AttributeError):
            continue
        if isinstance(title, str) and title and title not in ignore:
            by_name[("json-schema", title)].append({"path": info.path, "line": 1, "ref": tree.side,
                                                    "symbol": title})
    hits = []
    for (lang, name), evs in sorted(by_name.items()):
        files = sorted({e["path"] for e in evs})
        if len(files) < 2:
            continue
        hits.append(hit(opts, evs, ("repo", f"{lang}:{name}"),
                        f"{lang} の型 {name} が {len(files)} 個の file で定義されている（{', '.join(files)}）。"
                        "同じ契約の正本が複数ある候補。別の意味なら名前を分けるか profile の例外に理由を書く",
                        key=f"name:{lang}:{name}", value=len(evs)))
    for (lang, fields), evs in sorted(by_shape.items()):
        files = sorted({e["path"] for e in evs})
        names = sorted({e["symbol"] for e in evs})
        if len(files) < 2 or len(names) < 2:
            continue  # 同名で同形は上の同名の finding に含まれる
        hits.append(hit(opts, evs, ("repo", f"{lang}:shape:{'+'.join(names)}"),
                        f"{lang} の型 {', '.join(names)} の field の名前の集合が同じ（{len(fields)} 個: "
                        f"{', '.join(fields)}）。名前の違う同じ契約の候補",
                        key=f"shape:{lang}:{','.join(fields)}", value=len(evs)))
    return hits


RULES = [Rule(id=RULE_ID, kind="structure", evaluate=evaluate, languages=tuple(_DEF), candidate=True,
              default_required=False, default_severity="warning")]
