"""rule deps.violation・deps.cycle（ADR 2026-11-07-repo-health-contract D5）。

依存の辺は health/lang_deps.py（Rust の Cargo.toml・use、Python の import、TypeScript の相対 import）から作る。

deps.violation: profile が宣言する層と許可の向きに反する辺。
    [rules."deps.violation"]
    include_dev = false                 # Cargo の dev-dependencies も見るか（既定 false）
    [[rules."deps.violation".layers]]
    name = "core"                       # 層の名前（一意）
    paths = ['^crates/core/']           # 層に属す path の正規表現（辺の元の file・先の file に当てる。最初に当たった層）
    allow = []                          # 依存してよい他の層の名前（自層は常に可）
  層が 1 つも無ければ not_applicable（no_layers_declared）。どの層にも当たらない path の辺は見ない。
  fingerprint は（元の層→先の層、元の file、先の単位）。同じ組の辺は 1 件にまとめ、evidence に全部の行を並べる。

deps.cycle: 単位（Rust は package の src 直下の module、Python・TypeScript は file、Cargo は package）の
強連結成分（大きさ 2 以上）。Rust の use は同じ package の中の辺だけ、package をまたぐ循環は Cargo の辺で見る。
    [rules."deps.cycle"]
    include_dev = false                 # Cargo の dev-dependencies の辺を数えるか
    include_type_only = false           # TypeScript の import type を数えるか
    max_evidence = 40                   # 1 件の finding に載せる辺の上限
  base の循環との突き合わせ: head の循環は、節点を最も多く共有する base の循環と同じ fingerprint にする
  （共有が無ければ新規）。値は節点の数で、base より増えれば worsened。循環が縮んだ・分かれただけなら existing。
"""

from __future__ import annotations

import re

from health import lang_deps
from health.rule import Hit, NotApplicable, Rule, Unknown

LANGS = ("rust", "python", "typescript")


def _check_keys(cfg: dict, allowed: set[str], rid: str) -> None:
    bad = sorted(set(cfg) - allowed - {"enabled", "required"})
    if bad:
        raise Unknown(f"config_invalid: rules.{rid} に未知の欄 {bad}")


# ------------------------------------------------------------------ deps.violation

def _layers(cfg: dict) -> list[tuple[str, list[re.Pattern], set[str]]]:
    _check_keys(cfg, {"layers", "include_dev"}, "deps.violation")
    raw = cfg.get("layers", [])
    if not isinstance(raw, list):
        raise Unknown("config_invalid: rules.deps.violation.layers は表の配列")
    out = []
    names: set[str] = set()
    for i, layer in enumerate(raw):
        where = f"rules.deps.violation.layers[{i}]"
        if not isinstance(layer, dict) or set(layer) - {"name", "paths", "allow"}:
            raise Unknown(f"config_invalid: {where} は name・paths・allow だけの表")
        name = layer.get("name")
        paths = layer.get("paths")
        allow = layer.get("allow", [])
        if not isinstance(name, str) or not name or name in names:
            raise Unknown(f"config_invalid: {where}.name が無いか重複")
        if not isinstance(paths, list) or not paths or not all(isinstance(p, str) for p in paths):
            raise Unknown(f"config_invalid: {where}.paths は 1 つ以上の正規表現")
        if not isinstance(allow, list) or not all(isinstance(a, str) for a in allow):
            raise Unknown(f"config_invalid: {where}.allow は層の名前の配列")
        try:
            pats = [re.compile(p) for p in paths]
        except re.error as exc:
            raise Unknown(f"config_invalid: {where}.paths の正規表現が不正: {exc}") from None
        names.add(name)
        out.append((name, pats, set(allow)))
    for name, _, allow in out:
        unknown = sorted(allow - names)
        if unknown:
            raise Unknown(f"config_invalid: 層 {name} の allow に未知の層 {unknown}")
    return out


def _layer_of(layers, path: str) -> str | None:
    for name, pats, _ in layers:
        if any(p.search(path) for p in pats):
            return name
    return None


def violation(ctx, tree):
    layers = _layers(ctx.config)
    if not layers:
        raise NotApplicable("no_layers_declared: profile の rules.\"deps.violation\".layers が空")
    allow = {name: a for name, _, a in layers}
    include_dev = bool(ctx.config.get("include_dev", False))
    hits = []
    for e in lang_deps.build(ctx, tree).edges:
        if e.kind == "cargo-dev" and not include_dev:
            continue
        sl, dl = _layer_of(layers, e.src), _layer_of(layers, e.dst)
        if sl is None or dl is None or sl == dl or dl in allow[sl]:
            continue
        allowed = ", ".join(sorted(allow[sl])) or "なし"
        hits.append(Hit(
            evidence=[{"path": e.src, "line": e.line, "ref": tree.side, "symbol": f"{e.src_unit} -> {e.dst_unit}",
                       "excerpt": e.excerpt}],
            scope=("crate" if e.kind.startswith("cargo") else "module", e.src_unit),
            rationale=(f"層 {sl} の {e.src} が層 {dl} の {e.dst_unit} に依存する（{e.kind}）。"
                       f"profile が層 {sl} に許す依存先: {allowed}"),
            key=f"{sl}->{dl}|{e.src}|{e.dst_unit}",
        ))
    return hits


# ------------------------------------------------------------------ deps.cycle

def _cycle_edges(ctx, graph) -> list:
    include_dev = bool(ctx.config.get("include_dev", False))
    include_type = bool(ctx.config.get("include_type_only", False))
    out = []
    for e in graph.edges:
        if e.src_unit == e.dst_unit:
            continue
        if e.kind == "cargo-dev" and not include_dev:
            continue
        if e.type_only and not include_type:
            continue
        if e.kind == "use":
            if "/src/" not in e.dst_unit or e.src_unit.rsplit("/src/", 1)[0] != e.dst_unit.rsplit("/src/", 1)[0]:
                continue  # package をまたぐ use は Cargo の辺で見る
        out.append(e)
    return out


def _components(ctx, tree):
    cached = getattr(tree, "_rh_cycles", None)
    if cached is not None:
        return cached
    edges = _cycle_edges(ctx, lang_deps.build(ctx, tree))
    adj: dict[str, set[str]] = {}
    nodes: set[str] = set()
    for e in edges:
        adj.setdefault(e.src_unit, set()).add(e.dst_unit)
        nodes.update((e.src_unit, e.dst_unit))
    result = (lang_deps.sccs(nodes, adj), edges)
    tree._rh_cycles = result  # noqa: SLF001 — tree ごとの cache
    return result


def _key(nodes) -> str:
    return "cycle:" + "|".join(sorted(nodes))


def cycle(ctx, tree):
    _check_keys(ctx.config, {"include_dev", "include_type_only", "max_evidence"}, "deps.cycle")
    max_ev = int(ctx.config.get("max_evidence", 40))
    comps, edges = _components(ctx, tree)
    base_comps: list[list[str]] = []
    if tree.side == "head" and ctx.snapshot.base_tree is not None:
        base_comps = _components(ctx, ctx.snapshot.base_tree)[0]
    groups: dict[str, dict] = {}
    for comp in comps:
        key, matched = _key(comp), None
        if tree.side == "head":
            shared = sorted(((-len(set(comp) & set(b)), _key(b), b) for b in base_comps if set(comp) & set(b)))
            if shared:  # 節点を最も多く共有する base の循環（同数なら鍵の順で先）
                key, matched = shared[0][1], shared[0][2]
        g = groups.setdefault(key, {"nodes": set(), "base": matched})
        g["nodes"].update(comp)
    hits = []
    for key, g in sorted(groups.items()):
        nodes = g["nodes"]
        inner = [e for e in edges if e.src_unit in nodes and e.dst_unit in nodes]
        seen, evidence = set(), []
        for e in inner:
            if (e.src_unit, e.dst_unit) in seen:
                continue
            seen.add((e.src_unit, e.dst_unit))
            evidence.append({"path": e.src, "line": e.line, "ref": tree.side,
                             "symbol": f"{e.src_unit} -> {e.dst_unit}", "excerpt": e.excerpt})
        total = len(evidence)
        evidence = evidence[:max_ev]
        kinds = {e.kind for e in inner}
        scope_kind = "crate" if kinds <= {"cargo", "cargo-dev"} else "module"
        why = f"{len(nodes)} 単位の循環（辺 {total} 組）: {', '.join(sorted(nodes))}。"
        if total > len(evidence):
            why += f" evidence は先頭 {len(evidence)} 組だけ。"
        if g["base"] is not None:
            why += f" base の循環（{len(g['base'])} 単位）と節点を共有するので同じ循環として比べた。"
        hits.append(Hit(evidence=evidence, scope=(scope_kind, sorted(nodes)[0]), rationale=why, key=key,
                        value=float(len(nodes))))
    return hits


RULES = [
    Rule(id="deps.violation", kind="structure", evaluate=violation, adapter="generic", languages=LANGS),
    Rule(id="deps.cycle", kind="structure", evaluate=cycle, adapter="generic", languages=LANGS),
]
