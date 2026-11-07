"""候補 rule（dup.*・link.unlinked・migration.remnant）が共有する走査の部品。

すべて静的推定で、file を読むだけ（書き換え・削除をしない）。rule 固有の設定は profile の
`[rules."<id>"]` から ctx.config で渡る。共通の欄:
    severity = "warning" | "info"    # 既定 warning（候補は error にならない。ADR D4・S4）
    paths = ["<regex>"]              # 対象の path（既定は全部）
    ignore_paths = ["<regex>"]       # 対象から外す path
    classes = ["product", ...]       # 対象の種類（rule ごとに既定が違う）
未知の欄は ConfigError（report では error・crashed:<rule_id>:ConfigError）。
"""

from __future__ import annotations

import re

from .rule import Hit

COMMON_KEYS = {"enabled", "required", "severity", "paths", "ignore_paths", "classes"}
SEVERITIES = ("warning", "info")


class ConfigError(Exception):
    """profile の rule 設定の誤り。"""


class Options:
    """rule の設定を読み、既定と型を確かめる。"""

    def __init__(self, ctx, rule_id: str, defaults: dict) -> None:
        cfg = dict(ctx.config)
        unknown = sorted(set(cfg) - COMMON_KEYS - set(defaults))
        if unknown:
            raise ConfigError(f"{rule_id}: 未知の欄 {unknown}")
        self.values = {"severity": "warning", "paths": [], "ignore_paths": [], **defaults}
        for key, value in cfg.items():
            if key in ("enabled", "required"):
                continue
            want = self.values.get(key)
            if want is not None and type(want) is not type(value) and not (
                isinstance(want, (int, float)) and isinstance(value, (int, float)) and not isinstance(value, bool)
            ):
                raise ConfigError(f"{rule_id}.{key}: 型が違う（既定 {want!r}）")
            self.values[key] = value
        if self.values["severity"] not in SEVERITIES:
            raise ConfigError(f"{rule_id}.severity は {SEVERITIES} のどれか")
        self.paths = [re.compile(p) for p in self.values["paths"]]
        self.ignore = [re.compile(p) for p in self.values["ignore_paths"]]

    def __getitem__(self, key: str):
        return self.values[key]

    def regexes(self, key: str) -> list[re.Pattern]:
        return [re.compile(p) for p in self.values[key]]

    def wants(self, path: str) -> bool:
        if self.paths and not any(p.search(path) for p in self.paths):
            return False
        return not any(p.search(path) for p in self.ignore)

    def files(self, ctx, tree, languages=(), classes=None):
        """(FileInfo, text) を path 順に返す。classes を省けば設定の classes。"""
        cls = tuple(self.values.get("classes", ())) if classes is None else tuple(classes)
        for info in ctx.target_files(tree, languages=languages, classes=cls):
            if not self.wants(info.path):
                continue
            text = tree.read_text(info.path)
            if text is not None:
                yield info, text


def line_of(text: str, offset: int) -> int:
    return text.count("\n", 0, offset) + 1


def excerpt(text: str, line: int) -> str:
    lines = text.splitlines()
    return lines[line - 1].strip() if 0 < line <= len(lines) else ""


def hit(opts: Options, evidence: list[dict], scope: tuple[str, str], rationale: str, key: str,
        value: float | None = None, threshold: float | None = None) -> Hit:
    return Hit(evidence=evidence, scope=scope, rationale=rationale, severity=opts["severity"], key=key,
               value=value, threshold=threshold)


def module_of(path: str, group_by: str) -> str:
    """書き手・公開口の単位。file は path そのもの、dir は親 dir（crate の src 下の dir など）。"""
    if group_by == "file":
        return path
    return path.rsplit("/", 1)[0] if "/" in path else "."


def strip_comments(text: str, language: str) -> str:
    """行番号を保ったまま行 comment を空白にする（文字列の中は気にしない粗い近似）。"""
    if language in ("rust", "typescript"):
        return re.sub(r"//[^\n]*", lambda m: " " * len(m.group(0)), text)
    if language in ("python", "shell"):
        return re.sub(r"(?m)^\s*#[^\n]*", lambda m: " " * len(m.group(0)), text)
    return text
