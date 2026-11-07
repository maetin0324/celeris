"""rule dup.public-surface: 同じ機能を出す公開口が複数の module にある候補。

拾う公開口（静的推定。削除・書き換えはしない）:
- http: route の登録（axum の `.route("/p", ...)`、`@app.get("/p")`・`router.post('/p', ...)` の形）。
  path の引数（`{id}`・`:id`・`<id>`）は `{}` に揃えて比べる。
- cli: subcommand の宣言（argparse の `add_parser("name")`、clap の `#[command(name = "name")]`）。
- fn: rust の行頭の `pub fn`（method ではない関数）と typescript の `export function`。
同じ種類・同じ名前（path）が、`group_by` 単位で `min_modules` 個以上の module にあれば finding
（鍵は `<種類>:<名前>`、value は module の数）。

設定（profile の [rules."dup.public-surface"]。共通の欄は health/scan.py）:
    classes = ["product"]
    group_by = "dir"
    min_modules = 2
    kinds = ["http", "cli", "fn"]
    ignore_names = ["main"]     # 慣習で各 module に置く名前（main・new など）
"""

from __future__ import annotations

import re
from collections import defaultdict

from health.rule import Rule
from health.scan import ConfigError, Options, excerpt, hit, line_of, module_of, strip_comments

RULE_ID = "dup.public-surface"
DEFAULTS = {"classes": ["product"], "group_by": "dir", "min_modules": 2, "kinds": ["http", "cli", "fn"],
            "ignore_names": ["main"]}
LANGUAGES = ("rust", "python", "typescript")
KINDS = ("http", "cli", "fn")
_Q = r"[\"'`]"
_PATTERNS = {
    "http": [
        re.compile(r"\.route\(\s*" + _Q + r"(/[^\"'`]*)" + _Q),
        re.compile(r"(?:@|\b)(?:app|router|api|bp|blueprint|server)\.(?:get|post|put|patch|delete|route)\(\s*"
                   + _Q + r"(/[^\"'`]*)" + _Q),
    ],
    "cli": [
        re.compile(r"\badd_parser\(\s*" + _Q + r"([A-Za-z0-9_-]+)" + _Q),
        re.compile(r"#\[command\([^)]*\bname\s*=\s*\"([A-Za-z0-9_-]+)\""),
    ],
    "fn": [
        re.compile(r"(?m)^pub(?:[ \t]+async)?[ \t]+fn[ \t]+([a-z_][a-z0-9_]*)"),
        re.compile(r"(?m)^export[ \t]+(?:default[ \t]+)?(?:async[ \t]+)?function[ \t]*\*?[ \t]*([A-Za-z_$][\w$]*)"),
    ],
}
_PARAM = re.compile(r"\{[^}]*\}|:[A-Za-z_][A-Za-z0-9_]*|<[^>]*>")


def _norm(kind: str, name: str) -> str:
    if kind == "http":
        name = _PARAM.sub("{}", name).rstrip("/") or "/"
    return name


def evaluate(ctx, tree):
    opts = Options(ctx, RULE_ID, DEFAULTS)
    if opts["group_by"] not in ("dir", "file"):
        raise ConfigError(f"{RULE_ID}.group_by は dir か file")
    bad = sorted(set(opts["kinds"]) - set(KINDS))
    if bad:
        raise ConfigError(f"{RULE_ID}.kinds に未知の種類 {bad}")
    ignore = set(opts["ignore_names"])
    found: dict[tuple[str, str], dict[str, list[dict]]] = defaultdict(lambda: defaultdict(list))
    for info, raw in opts.files(ctx, tree, languages=LANGUAGES):
        text = strip_comments(raw, info.language)
        for kind in opts["kinds"]:
            for pat in _PATTERNS[kind]:
                for m in pat.finditer(text):
                    name = _norm(kind, m.group(1))
                    if name in ignore:
                        continue
                    line = line_of(text, m.start())
                    found[(kind, name)][module_of(info.path, opts["group_by"])].append(
                        {"path": info.path, "line": line, "ref": tree.side, "symbol": f"{kind}:{name}",
                         "excerpt": excerpt(raw, line)})
    hits = []
    for (kind, name), mods in sorted(found.items()):
        if len(mods) < opts["min_modules"]:
            continue
        evs = [e for m in sorted(mods) for e in mods[m]]
        hits.append(hit(opts, evs, ("repo", f"{kind}:{name}"),
                        f"公開口 {kind} {name} が {len(mods)} 個の module（{opts['group_by']} 単位: "
                        f"{', '.join(sorted(mods))}）にある（閾値 {opts['min_modules']}）。同じ機能の入口が複数ある候補",
                        key=f"{kind}:{name}", value=len(mods), threshold=opts["min_modules"]))
    return hits


RULES = [Rule(id=RULE_ID, kind="structure", evaluate=evaluate, languages=LANGUAGES, candidate=True,
              default_required=False, default_severity="warning")]
