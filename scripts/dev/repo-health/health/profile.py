"""profile の解決と読み込み。

解決順・名前の探し方・base 側で読むこと（bootstrap・builtin）は ADR 2026-11-07-repo-health-contract D2。
解釈はこの module の 1 つだけにする: report（`resolve`）も review policy の出力（`resolve_base`・
`Profile.review_policy`。ADR 2026-11-07-health-review-integration D2.3）もここを通る。path は repo 内の
相対 path に限る（例外の出典を repo の path で書くため）。

profile の項目（未知の項目は読み込みエラー。破壊的変更の方針で旧い欄を黙って無視しない）:
    version = "1"                      # 必須。--policy-version と一致しなければ exit 2
    extends = "<name>"                 # 任意。1 段だけ（親の extends はエラー）
    description = "..."                # 任意
    policy_paths = ["<regex>", ...]    # policy とみなす path。既定 ["^scripts/dev/repo-health/"]
    exclude_paths = ["<regex>", ...]   # 解析しない path（vendor など）。diff の判定・rule の対象から外す
    [doc_only] paths = ["<regex>"]     # 変更が全部当たれば doc-only。既定 ['\\.md$', '^docs/', '^agent-docs/']
    [adapters] enabled = [...]         # 使う言語 adapter（既定は全部）。外した言語は adapter 無し扱い
               ignore_languages = [...]  # adapter の無い言語でも unsupported にしない言語（理由は profile のコメント）
    [classes] test = ["<regex>"] ...   # 種類の path 規則の追加（generated・fixture・migration・test・docs）
    [arch_map] path = "<repo path>"    # architecture-map の path（rules/arch_map.py が読む）
               reference = "<repo path>"  # 任意。地図と比べる capability の所在の JSON（policy の側で読む）
               stale_status = "<regex>"   # 任意。ADR の状態行がこれに当たれば arch-map.stale
    [metrics."<name>"]                 # 構造指標（report の metrics.structural。判定には使わない）
        path = "<repo path>"           # 数える file
        item = "<regex>"               # 数える行
        start = "<regex>" / end = "<regex>"  # 任意。start に当たった行の次から end に当たる行の前までだけ数える
        source = "..."                 # 任意。指標の出典（数え方を決めた記録）
    [rules."<id>"]                     # rule ごとの設定
        enabled = true|false           # false なら not_applicable（disabled_by_profile）
        required = true|false          # 既定は rule の既定
        <閾値など rule 固有の欄>
        [[rules."<id>".exceptions]]    # 例外。reason 必須、fingerprint か path（正規表現）のどちらか
        fingerprint = "<rule_id>:<hash>" / path = "<regex>"
        reason = "..."
        expires = YYYY-MM-DD           # 任意。TOML の日付（文字列・日時・時刻・数はエラー）。その日まで有効。
                                       # 基準日は base の commit の committer date の UTC の日付（実行時の時計は
                                       # 使わない）。期限切れの例外は免除しない（engine が finding の exception を
                                       # null にし expired_exception に書く。ADR 2026-11-07-repo-health-contract
                                       # 付記 2026-11-10）
    [review]                           # 任意。repo 固有の設計 review の方針（ADR 2026-11-07-health-review-integration D2）
        owner_map = "<repo path>"      # 変更 path の owner 行を引く file（地図の表の行）
        [[review.high_risk]]           # 高 risk の path class。class は空でない文字列で重複不可
        class = "<name>"
        paths = ["<regex>", ...]       # 1 件以上
        [review.invariants]            # 保護する不変条件の名前 → 根拠の file（repo path）
        <name> = "<repo path>"
        # 下位項目はこの 3 つだけ（未知の項目は読み込みエラー）。extends では節を丸ごと置き換えず、
        # 下位項目ごとに子が勝つ（_merge の表の扱い）。子の空の [review] は親の値をそのまま使う

review policy（`--print-review-policy` の JSON、schema/review-policy-v1.schema.json）:
    source = base|builtin|absent       # [review] が無ければ absent（owner_map は null、配列と表は空）
    definition_sha256                  # 解決に使った profile の内容（子、extends の親の順）の sha256。
                                       # 1 file ごとに `<path>\\0<byte 数>\\0<内容>` を続けて hash する。
                                       # 組み込み generic は path `builtin:generic`、内容は sort 済みの JSON
"""

from __future__ import annotations

import datetime
import hashlib
import json
import os
import re
import tomllib
from dataclasses import dataclass, field

from .adapters import ADAPTER_LANGUAGES, CLASSES
from .snapshot import Snapshot, Tree, UsageError

REPO_PROFILE_DIR = "scripts/dev/repo-health/profiles"
TOOL_PROFILE_DIR = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "profiles")
DEFAULT_NAME = "default"
BUILTIN_NAME = "generic"
REVIEW_POLICY_SCHEMA = "celeris.repo-health.review-policy/1"

BUILTIN_GENERIC: dict = {
    "version": "1",
    "description": "組み込み generic（repo に profile が無いとき）",
    "policy_paths": ["^scripts/dev/repo-health/"],
    "doc_only": {"paths": [r"\.md$", "^docs/", "^agent-docs/"]},
    "rules": {"policy.changed": {"required": True}},
}

_TOP_KEYS = {"version", "extends", "description", "policy_paths", "exclude_paths", "doc_only", "adapters",
             "classes", "arch_map", "metrics", "rules", "review"}
_REVIEW_KEYS = {"owner_map", "high_risk", "invariants"}
_RULE_ID = re.compile(
    r"^(deps\.violation|deps\.cycle|size\.responsibility-growth|dup\.contract|dup\.state|dup\.public-surface|"
    r"link\.unlinked|dup\.test-fixture|migration\.remnant|arch-map\.(missing_row|missing_test|wrong_owner|"
    r"missing_invariant|stale|wrong_path)|doc\.links|adr\.numbers|progress\.index|human-text|source-size|"
    r"policy\.changed)$"
)


@dataclass
class Exception_:
    reason: str
    path: str  # profile の repo path
    line: int
    fingerprint: str | None = None
    path_re: re.Pattern | None = None
    expires: datetime.date | None = None  # その日まで有効。None は無期限

    def matches(self, fingerprint: str, evidence_paths: list[str]) -> bool:
        if self.fingerprint is not None:
            return self.fingerprint == fingerprint
        assert self.path_re is not None
        return any(self.path_re.search(p) for p in evidence_paths)

    def expired(self, as_of: datetime.date | None) -> bool:
        """基準日 as_of（base の commit の UTC の日付）に期限を過ぎているか。日付が無ければ期限付きは期限切れ。"""
        return self.expires is not None and (as_of is None or as_of > self.expires)

    def as_report(self) -> dict:
        out: dict = {"reason": self.reason, "source": {"path": self.path, "line": self.line}}
        if self.expires is not None:
            out["expires"] = self.expires.isoformat()
        return out


@dataclass
class Profile:
    name: str
    requested: str | None
    source: str  # cli | repo | builtin（inputs.profile.source）
    policy_source: str  # base | bootstrap | builtin（policy.source）
    data: dict
    files: list[str] = field(default_factory=list)  # repo 内の profile file（extends 先を含む）
    exceptions: dict[str, list[Exception_]] = field(default_factory=dict)
    definition: list[tuple[str, bytes]] = field(default_factory=list)  # 解決に使った (path, 内容)。子、親の順

    @property
    def version(self) -> str:
        return str(self.data["version"])

    def list_of(self, key: str, default: list[str]) -> list[str]:
        return list(self.data.get(key, default))

    @property
    def policy_paths(self) -> list[re.Pattern]:
        pats = self.list_of("policy_paths", ["^scripts/dev/repo-health/"])
        return [re.compile(p) for p in pats]

    @property
    def exclude_paths(self) -> list[re.Pattern]:
        return [re.compile(p) for p in self.list_of("exclude_paths", [])]

    @property
    def doc_only_paths(self) -> list[re.Pattern]:
        pats = self.data.get("doc_only", {}).get("paths", [r"\.md$", "^docs/", "^agent-docs/"])
        return [re.compile(p) for p in pats]

    @property
    def enabled_adapters(self) -> set[str]:
        return set(self.data.get("adapters", {}).get("enabled", ADAPTER_LANGUAGES))

    @property
    def ignore_languages(self) -> set[str]:
        return set(self.data.get("adapters", {}).get("ignore_languages", []))

    @property
    def classes(self) -> dict[str, list[str]]:
        return dict(self.data.get("classes", {}))

    def rule_config(self, rule_id: str) -> dict:
        cfg = dict(self.data.get("rules", {}).get(rule_id, {}))
        cfg.pop("exceptions", None)
        return cfg

    @property
    def metrics(self) -> dict[str, dict]:
        return dict(self.data.get("metrics", {}))

    def mentioned_rules(self) -> list[str]:
        return sorted(self.data.get("rules", {}))

    @property
    def has_expiring_exceptions(self) -> bool:
        """期限付きの例外があるか（report の policy.exceptions_as_of を書くかどうか）。"""
        return any(e.expires is not None for excs in self.exceptions.values() for e in excs)

    def definition_sha256(self) -> str:
        h = hashlib.sha256()
        for path, raw in self.definition:
            h.update(path.encode("utf-8") + b"\0" + str(len(raw)).encode() + b"\0" + raw)
        return h.hexdigest()

    def review_policy(self) -> dict:
        """review policy の JSON（ADR 2026-11-07-health-review-integration D2.3）。"""
        rv = self.data.get("review")
        return {
            "schema": REVIEW_POLICY_SCHEMA,
            "source": "absent" if rv is None else self.policy_source,
            "owner_map": None if rv is None else rv.get("owner_map"),
            "high_risk": [{"class": c["class"], "paths": list(c["paths"])} for c in (rv or {}).get("high_risk", [])],
            "invariants": dict(sorted((rv or {}).get("invariants", {}).items())),
            "definition_sha256": self.definition_sha256(),
        }


def _err(where: str, msg: str) -> UsageError:
    return UsageError(f"profile_invalid: {where}: {msg}")


def _check_regexes(where: str, values) -> None:
    if not isinstance(values, list) or not all(isinstance(v, str) for v in values):
        raise _err(where, "文字列の配列でなければならない")
    for v in values:
        try:
            re.compile(v)
        except re.error as exc:
            raise _err(where, f"正規表現が不正 {v!r}: {exc}") from None


def validate(data: dict, where: str) -> None:
    unknown = sorted(set(data) - _TOP_KEYS)
    if unknown:
        raise _err(where, f"未知の項目 {unknown}")
    if "version" in data and not isinstance(data["version"], (str, int)):
        raise _err(where, "version は文字列")
    for key in ("policy_paths", "exclude_paths"):
        if key in data:
            _check_regexes(f"{where}:{key}", data[key])
    if "doc_only" in data:
        if set(data["doc_only"]) - {"paths"}:
            raise _err(where, "doc_only は paths だけ")
        _check_regexes(f"{where}:doc_only.paths", data["doc_only"].get("paths", []))
    if "adapters" in data:
        ad = data["adapters"]
        if set(ad) - {"enabled", "ignore_languages"}:
            raise _err(where, "adapters は enabled・ignore_languages だけ")
        bad = sorted(set(ad.get("enabled", [])) - set(ADAPTER_LANGUAGES))
        if bad:
            raise _err(where, f"adapters.enabled に未知の言語 {bad}（使えるのは {list(ADAPTER_LANGUAGES)}）")
    if "classes" in data:
        bad = sorted(set(data["classes"]) - set(CLASSES[:-1]))
        if bad:
            raise _err(where, f"classes に未知の種類 {bad}")
        for k, v in data["classes"].items():
            _check_regexes(f"{where}:classes.{k}", v)
    if "arch_map" in data:
        am = data["arch_map"]
        if set(am) - {"path", "reference", "stale_status"}:
            raise _err(where, "arch_map は path・reference・stale_status だけ")
        if not all(isinstance(v, str) for v in am.values()):
            raise _err(where, "arch_map の値は文字列")
        if "stale_status" in am:
            _check_regexes(f"{where}:arch_map.stale_status", [am["stale_status"]])
    for name, m in data.get("metrics", {}).items():
        if not isinstance(m, dict) or set(m) - {"path", "item", "start", "end", "source"} or \
                not all(isinstance(v, str) for v in m.values()) or "path" not in m or "item" not in m:
            raise _err(where, f"metrics.{name} は path・item（必須）・start・end・source の文字列だけ")
        _check_regexes(f"{where}:metrics.{name}", [m[k] for k in ("item", "start", "end") if k in m])
    if "review" in data:
        _validate_review(data["review"], where)
    for rid, cfg in data.get("rules", {}).items():
        if not _RULE_ID.match(rid):
            raise _err(where, f"未知の rule {rid}")
        if not isinstance(cfg, dict):
            raise _err(where, f"rules.{rid} は表")
        for key in ("enabled", "required"):
            if key in cfg and not isinstance(cfg[key], bool):
                raise _err(where, f"rules.{rid}.{key} は真偽値")
        for i, exc in enumerate(cfg.get("exceptions", [])):
            if not isinstance(exc, dict) or not str(exc.get("reason", "")).strip():
                raise _err(where, f"rules.{rid}.exceptions[{i}] に reason が無い")
            if ("fingerprint" in exc) == ("path" in exc):
                raise _err(where, f"rules.{rid}.exceptions[{i}] は fingerprint か path のどちらか 1 つ")
            if set(exc) - {"fingerprint", "path", "reason", "expires"}:
                raise _err(where, f"rules.{rid}.exceptions[{i}] に未知の欄")
            # datetime.datetime は datetime.date の subclass なので型そのものを比べる
            if "expires" in exc and type(exc["expires"]) is not datetime.date:
                raise _err(where, f"rules.{rid}.exceptions[{i}].expires は TOML の日付（YYYY-MM-DD。引用符を付けない）"
                                  f"でなければならない（{type(exc['expires']).__name__} {exc['expires']!r}）")
            if "path" in exc:
                _check_regexes(f"{where}:rules.{rid}.exceptions[{i}].path", [exc["path"]])
        if rid == "policy.changed" and cfg.get("exceptions"):
            raise _err(where, "policy.changed には例外を置けない")


def _validate_review(rv, where: str) -> None:
    if not isinstance(rv, dict):
        raise _err(where, "review は表")
    unknown = sorted(set(rv) - _REVIEW_KEYS)
    if unknown:
        raise _err(where, f"review に未知の項目 {unknown}（使えるのは {sorted(_REVIEW_KEYS)}）")
    if "owner_map" in rv and (not isinstance(rv["owner_map"], str) or not rv["owner_map"].strip()):
        raise _err(where, "review.owner_map は空でない文字列")
    if "high_risk" in rv:
        hr = rv["high_risk"]
        if not isinstance(hr, list):
            raise _err(where, "review.high_risk は表の配列")
        seen: set[str] = set()
        for i, c in enumerate(hr):
            if not isinstance(c, dict):
                raise _err(where, f"review.high_risk[{i}] は表")
            if set(c) - {"class", "paths"}:
                raise _err(where, f"review.high_risk[{i}] は class・paths だけ")
            cls = c.get("class")
            if not isinstance(cls, str) or not cls.strip():
                raise _err(where, f"review.high_risk[{i}].class は空でない文字列")
            if cls in seen:
                raise _err(where, f"review.high_risk の class {cls!r} が重複")
            seen.add(cls)
            if not c.get("paths"):
                raise _err(where, f"review.high_risk[{i}].paths は 1 件以上")
            _check_regexes(f"{where}:review.high_risk[{i}].paths", c["paths"])
    if "invariants" in rv:
        inv = rv["invariants"]
        if not isinstance(inv, dict) or not all(isinstance(v, str) and v.strip() for v in inv.values()) or \
                not all(str(k).strip() for k in inv):
            raise _err(where, "review.invariants は名前 → 根拠の file（空でない文字列）の表")


def _parse(text: str, where: str) -> dict:
    try:
        data = tomllib.loads(text)
    except tomllib.TOMLDecodeError as exc:
        raise _err(where, f"TOML が読めない: {exc}") from None
    validate(data, where)
    return data


def _merge(parent: dict, child: dict) -> dict:
    out = dict(parent)
    for key, value in child.items():
        if key == "extends":
            continue
        if key == "rules":
            rules = {k: dict(v) for k, v in parent.get("rules", {}).items()}
            for rid, cfg in value.items():
                rules[rid] = {**rules.get(rid, {}), **cfg}
            out["rules"] = rules
        elif isinstance(value, dict) and isinstance(parent.get(key), dict):
            out[key] = {**parent[key], **value}
        else:
            out[key] = value
    return out


def _exception_lines(text: str, rid: str, exceptions: list[dict], path: str) -> list[Exception_]:
    lines = text.splitlines()
    header = re.compile(r'^\s*\[\[\s*rules\.(?:"' + re.escape(rid) + r'"|' + re.escape(rid) + r")\.exceptions\s*\]\]")
    starts = [i + 1 for i, line in enumerate(lines) if header.match(line)]
    out: list[Exception_] = []
    for i, exc in enumerate(exceptions):
        line = starts[i] if i < len(starts) else 1
        out.append(Exception_(
            reason=str(exc["reason"]),
            path=path,
            line=line,
            fingerprint=exc.get("fingerprint"),
            path_re=re.compile(exc["path"]) if "path" in exc else None,
            expires=exc.get("expires"),
        ))
    return out


class _Source:
    """profile の file を探す場所。base（不達なら None）・head（読まないなら None）・検査器自身の dir の順。"""

    def __init__(self, base: Tree | None, head: Tree | None) -> None:
        self.base = base
        self.head = head

    def find(self, ref: str, by_name: bool) -> tuple[str, str, str, bytes] | None:
        """(text, repo の path, policy の出所 base|bootstrap|builtin, 内容の byte 列)"""
        path = f"{REPO_PROFILE_DIR}/{ref}.toml" if by_name else ref
        for tree, source in ((self.base, "base"), (self.head, "bootstrap")):
            if tree is not None and tree.has(path):
                text, raw = tree.read_text(path), tree.read_bytes(path)
                if text is not None and raw is not None:
                    return text, path, source, raw
        if by_name:
            tool_path = os.path.join(TOOL_PROFILE_DIR, f"{ref}.toml")
            if os.path.isfile(tool_path):
                with open(tool_path, "rb") as fh:
                    raw = fh.read()
                return raw.decode("utf-8"), path, "builtin", raw
        return None


def _is_name(ref: str) -> bool:
    return "/" not in ref and not ref.endswith(".toml")


def resolve(snap: Snapshot, requested: str | None) -> Profile:
    """report 用。base に無ければ head（bootstrap）、検査器自身の profile の順に探す。"""
    return _resolve(snap.root, _Source(snap.base_tree, snap.head_tree), requested)


def resolve_base(root: str, base: Tree, requested: str | None) -> Profile:
    """review policy 用。head・作業ツリーは読まない（base か検査器自身の profile だけ）。"""
    return _resolve(root, _Source(base, None), requested)


def _resolve(root: str, src: _Source, requested: str | None) -> Profile:
    if requested is not None:
        by_name = _is_name(requested)
        ref = requested
        if not by_name:
            full = os.path.realpath(os.path.join(root, requested) if not os.path.isabs(requested) else requested)
            rel = os.path.relpath(full, os.path.realpath(root))
            if rel.startswith(".."):
                raise UsageError(f"profile_outside_repo: --profile {requested} は repo の外")
            ref = rel.replace(os.sep, "/")
        found = src.find(ref, by_name)
        if found is None:
            raise UsageError(f"profile_not_found: --profile {requested}")
        source = "cli"
        name = ref if by_name else os.path.splitext(os.path.basename(ref))[0]
    else:
        found = src.find(DEFAULT_NAME, True)
        if found is not None and found[2] == "builtin":
            found = None  # 検査器自身の default.toml は repo の既定ではない
        if found is None:
            validate(BUILTIN_GENERIC, "builtin:generic")
            raw = json.dumps(BUILTIN_GENERIC, ensure_ascii=False, sort_keys=True).encode("utf-8")
            return Profile(BUILTIN_NAME, None, "builtin", "builtin", dict(BUILTIN_GENERIC),
                           definition=[("builtin:generic", raw)])
        source = "repo"
        name = DEFAULT_NAME

    text, path, policy_source, raw = found
    data = _parse(text, path)
    files = [path] if policy_source != "builtin" else []
    definition = [(path, raw)]
    exceptions = {rid: _exception_lines(text, rid, cfg.get("exceptions", []), path)
                  for rid, cfg in data.get("rules", {}).items() if cfg.get("exceptions")}
    if "extends" in data:
        parent_ref = str(data["extends"])
        parent = src.find(parent_ref, True)
        if parent is None:
            raise _err(path, f"extends の {parent_ref} が見つからない")
        ptext, ppath, psource, praw = parent
        pdata = _parse(ptext, ppath)
        definition.append((ppath, praw))
        if "extends" in pdata:
            raise _err(ppath, "extends は 1 段だけ")
        for rid, cfg in pdata.get("rules", {}).items():
            if cfg.get("exceptions") and rid not in exceptions:
                exceptions[rid] = _exception_lines(ptext, rid, cfg["exceptions"], ppath)
        if psource != "builtin":
            files.append(ppath)
        # 親か子のどちらかが head にしか無ければ全体を bootstrap とする
        if "bootstrap" in (policy_source, psource):
            policy_source = "bootstrap"
        data = _merge(pdata, data)
    if "version" not in data:
        raise _err(path, "version が無い")
    return Profile(name, requested, source, policy_source, data, sorted(set(files)), exceptions, definition)
