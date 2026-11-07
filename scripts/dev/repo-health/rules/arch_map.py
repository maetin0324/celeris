"""rule arch-map.<kind>（ADR D5）: architecture-map と repo・対照（reference）との食い違い。

判定は `scripts/dev/check-architecture-map.py` の `find_diffs` が正本で、この module は呼ぶだけ
（種類の決め方はそちらの docstring）。地図と対照の path は profile から読む:

    [arch_map]
    path = "docs/architecture-map.md"      # 地図。無ければ全 kind が not_applicable
    reference = "<repo path>.json"         # 任意。capability の所在の対照。無ければ missing_row・
                                           # wrong_owner は not_applicable、他の kind は地図だけで判定
    stale_status = "<regex>"               # 任意。ADR の状態行がこれに当たれば stale（既定は検査器の STALE_STATUS）

地図は評価する側の tree（base・head）から読む。対照は policy の側（base、bootstrap なら head）から読み、
base と head を同じ対照で比べる（head が対照を書き換えて finding を消すことを防ぐ）。
地図の表の形が壊れている（見出し・列の数）と head では unknown、base では読める行だけで比べる。
"""

from __future__ import annotations

import importlib.util
import json
import os
import sys

from health.rule import Hit, NotApplicable, Rule, Unknown, Unsupported

CHECKER = os.path.join(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))),
                       "check-architecture-map.py")
# 種類の名前（判定ではない）。正本は検査器の KINDS で、読み込み時に一致を確かめる。
KINDS = ("missing_row", "missing_test", "wrong_owner", "missing_invariant", "stale", "wrong_path")
_cam = None


def checker():
    """検査器 check-architecture-map.py を読む（地図を評価する時だけ。無ければ unsupported）。"""
    global _cam
    if _cam is None:
        if not os.path.isfile(CHECKER):
            raise Unsupported(f"checker_missing: {CHECKER} が無い")
        spec = importlib.util.spec_from_file_location("check_architecture_map", CHECKER)
        assert spec is not None and spec.loader is not None
        module = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = module  # dataclass が module を引く
        spec.loader.exec_module(module)
        if tuple(module.KINDS) != KINDS:
            raise RuntimeError("check-architecture-map.py の KINDS と rule の種類が違う")
        _cam = module
    return _cam


REFERENCE_KINDS = ("missing_row", "wrong_owner")


def _exists(tree):
    files = tree.files()
    dirs = {"/".join(p.split("/")[:i]) for p in files for i in range(1, p.count("/") + 1)}

    def exists(rel: str) -> bool:
        rel = rel.rstrip("/")
        if "*" in rel:
            pat = checker()._glob_re(rel)
            return any(pat.match(p) for p in files) or any(pat.match(d) for d in dirs)
        return tree.has(rel) or rel in dirs

    return exists


def _reference(ctx) -> dict | None:
    path = ctx.profile.data.get("arch_map", {}).get("reference")
    if not path:
        return None
    text = ctx.policy_text(path)
    if text is None:
        raise Unknown(f"arch_map_reference_missing: {path} が policy の側に無い")
    try:
        data = json.loads(text)
    except json.JSONDecodeError as exc:
        raise Unknown(f"arch_map_reference_invalid: {path}: {exc}") from None
    if not isinstance(data, dict) or not isinstance(data.get("capabilities"), list):
        raise Unknown(f"arch_map_reference_invalid: {path} に capabilities の配列が無い")
    return data


def _diffs(ctx, tree, kind: str) -> list:
    cfg = ctx.profile.data.get("arch_map", {})
    doc = cfg.get("path")
    if not doc:
        raise NotApplicable("no_arch_map: profile に arch_map.path が無い")
    reference = _reference(ctx)
    if kind in REFERENCE_KINDS and reference is None:
        raise NotApplicable("no_reference: profile に arch_map.reference が無い")
    cam = checker()
    text = tree.read_text(doc)
    if text is None:
        if tree.side == "base":
            return []
        raise Unknown(f"arch_map_missing: profile の地図 {doc} が head に無い")
    lines = text.splitlines()
    broken = cam.check_columns(lines)
    shape = [e for e in broken if "見出し" in e or "列の数" in e]
    if shape and tree.side != "base":
        raise Unknown(f"arch_map_malformed: {doc}: {shape[0]}")
    diffs = cam.find_diffs(lines, doc, _exists(tree), tree.read_text, reference,
                           cfg.get("stale_status", cam.STALE_STATUS))
    return [d for d in diffs if d.kind == kind]


def _rule(kind: str) -> Rule:
    def evaluate(ctx, tree):
        doc = ctx.profile.data["arch_map"]["path"] if ctx.profile.data.get("arch_map", {}).get("path") else ""
        hits = []
        for d in _diffs(ctx, tree, kind):
            evidence = [{"path": doc, "line": d.line, "ref": tree.side, "excerpt": d.observed}] if d.line else \
                [{"path": doc, "ref": tree.side, "excerpt": d.observed}]
            evidence += [{"path": p, "ref": tree.side} for p in d.paths if p != doc]
            hits.append(Hit(evidence=evidence, scope=("file", doc), rationale=d.observed, key=f"{kind}|{d.key}"))
        return hits

    return Rule(id=f"arch-map.{kind}", kind="docs", adapter="docs", evaluate=evaluate,
                wraps="scripts/dev/check-architecture-map.py")


RULES = [_rule(kind) for kind in KINDS]
