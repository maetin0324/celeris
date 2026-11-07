"""rule の評価・delta・集約・report の組み立て（ADR D3・D4・D6・D7・D8）。"""

from __future__ import annotations

import hashlib
import importlib.util
import os
import re
import shutil
import subprocess
from dataclasses import dataclass

from . import TOOL_VERSION
from .adapters import Classifier, FileInfo
from .profile import Exception_, Profile
from .rule import Hit, NotApplicable, Rule, Unknown, Unsupported
from .snapshot import Snapshot, Tree, UsageError

TOOL_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RULES_DIR = os.path.join(TOOL_DIR, "rules")

# report の rules の並び（ADR D5 の順）。
ORDER = [
    "deps.violation", "deps.cycle", "size.responsibility-growth", "dup.contract", "dup.state",
    "dup.public-surface", "link.unlinked", "dup.test-fixture", "migration.remnant",
    "arch-map.missing_row", "arch-map.missing_test", "arch-map.wrong_owner", "arch-map.missing_invariant",
    "arch-map.stale", "arch-map.wrong_path", "doc.links", "adr.numbers", "progress.index", "human-text",
    "source-size", "policy.changed",
]
CANDIDATE_IDS = ("dup.", "link.unlinked", "migration.remnant")


def load_rules(rules_dir: str = RULES_DIR) -> dict[str, Rule]:
    """rules/ の module を名前順に読み、RULES を集める。同じ ID の二重登録は crash にする。"""
    registry: dict[str, Rule] = {}
    for name in sorted(os.listdir(rules_dir)):
        if not name.endswith(".py") or name.startswith("_"):
            continue
        spec = importlib.util.spec_from_file_location(f"repo_health_rules.{name[:-3]}", os.path.join(rules_dir, name))
        assert spec is not None and spec.loader is not None
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        for rule in getattr(module, "RULES", []):
            if rule.id in registry:
                raise RuntimeError(f"rule {rule.id} が二重に登録された（rules/{name}）")
            registry[rule.id] = rule
    return registry


@dataclass
class Context:
    """rule に渡す入力。policy（profile・baseline）は base 側（bootstrap なら head 側）。"""

    snapshot: Snapshot
    profile: Profile
    config: dict
    changed: list[str]
    changed_lines: dict[str, list[tuple[int, int]]]
    policy_files: list[dict]
    classifier: Classifier
    mode: str

    def info(self, tree: Tree, path: str) -> FileInfo:
        if path not in tree.infos:
            tree.infos[path] = self.classifier.classify(path, tree.read_bytes(path))
        return tree.infos[path]

    def excluded(self, path: str) -> bool:
        return any(p.search(path) for p in self.profile.exclude_paths)

    def target_files(self, tree: Tree, languages=(), classes=()) -> list[FileInfo]:
        out = []
        for path in tree.files():
            if self.excluded(path):
                continue
            info = self.info(tree, path)
            if languages and info.language not in languages:
                continue
            if classes and info.cls not in classes:
                continue
            out.append(info)
        return out

    def policy_text(self, path: str) -> str | None:
        """policy の file（baseline など）を policy の側で読む。"""
        tree = self.snapshot.base_tree if self.profile.policy_source != "bootstrap" else self.snapshot.head_tree
        return tree.read_text(path) if tree is not None else None

    def run(self, argv: list[str], cwd: str, timeout: int = 600) -> subprocess.CompletedProcess:
        if shutil.which(argv[0]) is None:
            raise Unsupported(f"tool_missing:{argv[0]}")
        return subprocess.run(argv, cwd=cwd, capture_output=True, text=True, timeout=timeout)


def fingerprint(rule_id: str, hit: Hit) -> str:
    key = hit.key
    if key is None:
        parts = sorted({f"{e['path']}#{e.get('symbol', '')}" for e in hit.evidence})
        key = f"{hit.scope[0]}:{hit.scope[1]}|" + "|".join(parts)
    return f"{rule_id}:" + hashlib.sha256(f"{rule_id}\n{key}".encode()).hexdigest()[:16]


def _evidence(items: list[dict]) -> list[dict]:
    out = []
    for e in items:
        item = {"path": e["path"]}
        for k in ("line", "end_line", "ref", "symbol"):
            if e.get(k) is not None:
                item[k] = e[k]
        if e.get("excerpt") is not None:
            item["excerpt"] = str(e["excerpt"])[:400]
        out.append(item)
    return sorted(out, key=lambda x: (x["path"], x.get("line", 0), x.get("ref", "head")))


def _collect(rule: Rule, ctx: Context, tree: Tree) -> dict[str, Hit]:
    hits: dict[str, Hit] = {}
    for hit in rule.evaluate(ctx, tree) or []:
        fp = fingerprint(rule.id, hit)
        if fp in hits:  # 同じ鍵は 1 件にまとめる（S11）
            hits[fp].evidence.extend(hit.evidence)
        else:
            hits[fp] = hit
    return hits


def _exception(profile: Profile, rule_id: str, fp: str, paths: list[str]) -> Exception_ | None:
    for exc in profile.exceptions.get(rule_id, []):
        if exc.matches(fp, paths):
            return exc
    return None


def _finding(rule: Rule, fp: str, hit: Hit, base: Hit | None, ctx: Context, snap: Snapshot) -> dict:
    severity = hit.severity or rule.default_severity
    candidate = rule.candidate or rule.id.startswith(CANDIDATE_IDS)
    if candidate and severity == "error":
        severity = "warning"
    if base is None:
        delta = "new"
    elif hit.value is not None and base.value is not None and hit.value > base.value:
        delta = "worsened"
    else:
        delta = "existing"
    evidence = _evidence(hit.evidence)
    exc = _exception(ctx.profile, rule.id, fp, [e["path"] for e in evidence])
    finding = {
        "rule_id": rule.id,
        "fingerprint": fp,
        "severity": severity,
        "delta": delta,
        "candidate": candidate,
        "evidence": evidence,
        "snapshot": snap.finding_snapshot(),
        "scope": {"kind": hit.scope[0], "id": hit.scope[1]},
        "exception": exc.as_report() if exc else None,
        "rationale": hit.rationale,
    }
    if hit.value is not None and base is not None and base.value is not None:
        bl = {"value": base.value, "head_value": hit.value}
        if hit.threshold is not None:
            bl["threshold"] = hit.threshold
        finding["baseline"] = bl
    return finding


def is_fail_grade(f: dict) -> bool:
    return f["severity"] == "error" and f["delta"] in ("new", "worsened") and f["exception"] is None


def _adapter_gap(rule: Rule, ctx: Context) -> str | None:
    """構造 rule の対象 diff に adapter の無い言語があれば adapter_missing:<lang,...>。"""
    if rule.kind != "structure":
        return None
    enabled = ctx.profile.enabled_adapters
    ignored = ctx.profile.ignore_languages
    missing = set()
    snap = ctx.snapshot
    for path in ctx.changed:
        tree = snap.head_tree if snap.head_tree.has(path) else snap.base_tree
        if tree is None or not tree.has(path):
            continue
        info = ctx.info(tree, path)
        if info.is_code and info.language not in enabled and info.language not in ignored:
            missing.add(info.language)
    return "adapter_missing:" + ",".join(sorted(missing)) if missing else None


def evaluate_rule(rule: Rule, ctx: Context) -> tuple[str, str, list[dict]]:
    """(status, reason, findings)"""
    snap = ctx.snapshot
    if ctx.config.get("enabled") is False:
        return "not_applicable", "disabled_by_profile", []
    if ctx.mode == "doc-only" and rule.kind == "structure":
        return "not_applicable", "doc_only: 変更は profile の doc_only.paths だけ", []
    for tool in rule.tools:
        if shutil.which(tool) is None:
            return "unsupported", f"tool_missing:{tool}", []
    gap = _adapter_gap(rule, ctx)
    if gap:
        return "unsupported", gap, []
    if rule.languages:
        langs = [lang for lang in rule.languages if lang in ctx.profile.enabled_adapters]
        if not langs:
            return "not_applicable", "adapter_disabled: profile の adapters.enabled に対象の言語が無い", []
        if not ctx.target_files(snap.head_tree, languages=langs):
            return "not_applicable", "no_target_files: head に対象の言語の file が無い", []
    try:
        head_hits = _collect(rule, ctx, snap.head_tree)
        base_hits: dict[str, Hit] = {}
        if not rule.head_only and snap.base_tree is not None:
            base_hits = _collect(rule, ctx, snap.base_tree)
    except NotApplicable as exc:
        return "not_applicable", str(exc.args[0] if exc.args else "not_applicable"), []
    except Unknown as exc:
        return "unknown", str(exc.args[0] if exc.args else "unknown"), []
    except Unsupported as exc:
        return "unsupported", str(exc.args[0] if exc.args else "unsupported"), []
    except subprocess.TimeoutExpired:
        return "error", f"timeout:{rule.id}", []
    except Exception as exc:  # noqa: BLE001 — rule の失敗は error として report に残す
        return "error", f"crashed:{rule.id}:{type(exc).__name__}", []
    findings = [_finding(rule, fp, hit, base_hits.get(fp), ctx, snap) for fp, hit in sorted(head_hits.items())]
    n = sum(1 for f in findings if is_fail_grade(f))
    if n:
        return "fail", f"{n} fail-grade finding(s)", findings
    return "pass", "", findings


def _policy_files(snap: Snapshot, profile: Profile) -> list[dict]:
    pats = profile.policy_paths
    paths = set(profile.files)
    for tree in (snap.base_tree, snap.head_tree):
        if tree is None:
            continue
        paths.update(p for p in tree.files() if any(r.search(p) for r in pats))
    out = []
    for p in sorted(paths):
        base_blob = snap.base_tree.blob_sha(p) if snap.base_tree is not None else None
        out.append({"path": p, "base_blob": base_blob, "head_blob": snap.head_tree.blob_sha(p)})
    return out


def _mode(changed: list[str], profile: Profile) -> str:
    if changed and all(any(r.search(p) for r in profile.doc_only_paths) for p in changed):
        return "doc-only"
    return "full"


def _select(registry: dict[str, Rule], profile_rules: list[str], only: list[str] | None) -> list[str]:
    ids = set(registry) | set(profile_rules)
    if only is not None:
        unknown = sorted(set(only) - ids)
        if unknown:
            raise UsageError(f"rule_not_found: --rules {','.join(unknown)} は登録も profile の記載も無い")
        ids = set(only) | {"policy.changed"}  # policy.changed は絞り込みで外せない
    return sorted(ids, key=lambda r: (ORDER.index(r) if r in ORDER else len(ORDER), r))


def _metrics(snap: Snapshot, ctx: Context) -> dict:
    langs: dict[str, int] = {}
    classes: dict[str, int] = {}
    for info in ctx.target_files(snap.head_tree):
        langs[info.language] = langs.get(info.language, 0) + 1
        classes[info.cls] = classes.get(info.cls, 0) + 1
    changed_lines = sum(b - a + 1 for rs in ctx.changed_lines.values() for a, b in rs)
    return {
        "diff": {"changed_files": len(ctx.changed), "changed_lines": changed_lines},
        "head_files": {"by_language": dict(sorted(langs.items())), "by_class": dict(sorted(classes.items()))},
    }


def count_metric(tree: Tree | None, spec: dict) -> int | None:
    """profile の構造指標 1 つを数える。file が無ければ None。"""
    if tree is None:
        return None
    text = tree.read_text(spec["path"])
    if text is None:
        return None
    item = re.compile(spec["item"])
    start = re.compile(spec["start"]) if "start" in spec else None
    end = re.compile(spec["end"]) if "end" in spec else None
    inside = start is None
    n = 0
    for line in text.splitlines():
        if not inside:
            inside = bool(start.search(line))
            continue
        if end is not None and start is not None and end.search(line):
            break
        if item.search(line):
            n += 1
    return n


def _structural(snap: Snapshot, profile: Profile) -> dict:
    out = {}
    for name, spec in sorted(profile.metrics.items()):
        entry = {"path": spec["path"], "base": count_metric(snap.base_tree, spec),
                 "head": count_metric(snap.head_tree, spec)}
        if "source" in spec:
            entry["source"] = spec["source"]
        out[name] = entry
    return out


def _summary(rules: list[dict], findings: list[dict], snap: Snapshot) -> dict:
    required = [r for r in rules if r["required"]]
    if any(r["status"] in ("error", "unknown", "unsupported") for r in required):
        verdict, code = "error", 2
    elif any(r["status"] == "fail" for r in required):
        verdict, code = "fail", 1
    else:
        verdict, code = "pass", 0
    reviewable = snap.head_kind == "commit" and not snap.dirty and snap.base_reachable
    return {
        "verdict": verdict,
        "exit_code": code,
        "reviewable": reviewable,
        "counts": {
            "findings": len(findings),
            "fail_grade": sum(1 for f in findings if is_fail_grade(f)),
            "new": sum(1 for f in findings if f["delta"] == "new"),
            "worsened": sum(1 for f in findings if f["delta"] == "worsened"),
            "existing": sum(1 for f in findings if f["delta"] == "existing"),
        },
    }


def _rule_entry(rid: str, required: bool, status: str, reason: str, rule: Rule | None) -> dict:
    entry = {"id": rid, "required": required, "status": status, "reason": reason}
    if rule is not None:
        entry["adapter"] = rule.adapter
        if rule.wraps:
            entry["wraps"] = rule.wraps
    return entry


def build_report(snap: Snapshot, profile: Profile | None, policy_version: str, requested: str | None,
                 only: list[str] | None, registry: dict[str, Rule]) -> dict:
    inputs_profile = {
        "requested": requested,
        "resolved": profile.name if profile else (requested or "default"),
        "source": profile.source if profile else ("cli" if requested else "repo"),
    }
    rules_out: list[dict] = []
    findings: list[dict] = []
    metrics = None
    if not snap.base_reachable or profile is None:
        # D7: base に届かない。policy も差分も決まらないので全 rule を unsupported にする。
        mode = "full"
        policy = {"version": policy_version, "source": "unavailable", "changed": False, "files": []}
        for rid in _select(registry, [], only):
            rules_out.append(_rule_entry(rid, True, "unsupported", "base_unreachable", registry.get(rid)))
    else:
        changed = [p for p in snap.changed if not any(r.search(p) for r in profile.exclude_paths)]
        mode = _mode(changed, profile)
        policy_files = _policy_files(snap, profile)
        classifier = Classifier(profile.classes)
        changed_lines = {p: r for p, r in snap.changed_lines.items() if p in set(changed)}
        policy = {
            "version": profile.version,
            "source": profile.policy_source,
            "changed": any(f["base_blob"] != f["head_blob"] for f in policy_files),
            "files": policy_files,
        }
        for rid in _select(registry, profile.mentioned_rules(), only):
            rule = registry.get(rid)
            cfg = profile.rule_config(rid)
            required = bool(cfg.get("required", rule.default_required if rule else True))
            if rule is None:
                rules_out.append(_rule_entry(rid, required, "unsupported",
                                             f"rule_missing:{rid}: profile にあるが rules/ に実装が無い", None))
                continue
            ctx = Context(snap, profile, cfg, changed, changed_lines, policy_files, classifier, mode)
            status, reason, fs = evaluate_rule(rule, ctx)
            rules_out.append(_rule_entry(rid, required, status, reason, rule))
            findings.extend(fs)
        metrics = _metrics(snap, Context(snap, profile, {}, changed, changed_lines, policy_files, classifier, mode))
        if profile.metrics:
            metrics["structural"] = _structural(snap, profile)
    findings.sort(key=lambda f: f["fingerprint"])
    report = {
        "schema": "celeris.repo-health.report/1",
        "tool": {"name": "repo_health.py", "version": TOOL_VERSION},
        "inputs": {
            "base": snap.base_input,
            "head": snap.head_input,
            "policy_version": policy_version,
            "profile": inputs_profile,
            "mode": mode,
        },
        "snapshot": snap.report_snapshot(),
        "policy": policy,
        "rules": rules_out,
        "findings": findings,
        "summary": _summary(rules_out, findings, snap),
    }
    if metrics is not None:
        report["metrics"] = metrics
    return report
