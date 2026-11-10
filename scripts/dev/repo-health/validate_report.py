#!/usr/bin/env python3
"""validate_report.py — repo health report（report-v1）の検証器。

    python3 scripts/dev/repo-health/validate_report.py <report.json>...

全件が妥当なら exit 0。1 件でも不正なら違反の場所を「file:JSON pointer: 内容」で標準出力に出して
exit 1。file が読めない・JSON でない・引数が無いのも exit 1（その file の違反として出す）。

検証は 2 段:
1. 形: schema/report-v1.schema.json（JSON Schema 2020-12 の部分集合。type・const・enum・required・
   properties・additionalProperties・items・minItems・minLength・maxLength・minimum・pattern・$ref）。
   外部 package は使わない（Python 3 標準ライブラリだけ）。
2. 意味（ADR 2026-11-07-repo-health-contract の集約規則。S13 は付記 2026-11-10）:
   S1  finding の snapshot は report の snapshot と一致する
   S2  finding の rule_id は rules にある
   S3  rule の status と finding の整合: pass は fail 級 finding なし、fail は 1 件以上、
       not_applicable・unsupported は finding なし
   S4  candidate=true の finding の severity は warning か info（候補は合格を妨げない・自動削除しない）
   S5  summary.verdict・exit_code は必須 rule の status から決まる
       （fail → fail/1、error・unknown・unsupported → error/2、どれも無ければ pass/0。error が fail に勝つ）
   S6  summary.counts は findings から数え直した値と一致する
   S7  summary.reviewable は head が commit・dirty でない・base に届く のときだけ true
   S8  policy.changed と rule_id=policy.changed の finding は同時に在る・同時に無い。
       policy.source=bootstrap なら policy.changed の finding は severity error でない
   S9  base に届かない（shallow で reachable=false）か policy.source=unavailable なら verdict は error
   S10 mode=doc-only では構造 rule（deps.*・size.*・dup.*・link.unlinked・migration.remnant・source-size）は
       not_applicable
   S11 fingerprint は report の中で一意で、先頭は自分の rule_id
   S12 rule の reason は pass 以外で空でない。exception 付き finding は fail 級に数えない
   S13 期限付き例外: expired_exception のある finding は exception が null（期限切れは免除しない）。
       exception.expires か expired_exception.expires があれば policy.exceptions_as_of がある。
       期限内（exception.expires）は exceptions_as_of が null でなく expires 以前、
       期限切れ（expired_exception.expires）は exceptions_as_of が null か expires より後
"""

from __future__ import annotations

import datetime
import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCHEMA_PATH = HERE / "schema" / "report-v1.schema.json"

# S10: doc-only で評価しない rule（前方一致）。ADR 2026-11-07-repo-health-contract D7。
DOC_ONLY_NOT_APPLICABLE_PREFIXES = ("deps.", "size.", "dup.", "link.unlinked", "migration.remnant", "source-size")
# 必須 rule を合格にしない status（ADR D3）。
BLOCKING_ERROR = ("error", "unknown", "unsupported")


class Violations:
    def __init__(self, file: str) -> None:
        self.file = file
        self.items: list[str] = []

    def add(self, pointer: str, message: str) -> None:
        self.items.append(f"{self.file}:{pointer or '/'}: {message}")


# ---------------------------------------------------------------- schema (subset)

def _type_ok(value, name: str) -> bool:
    if name == "object":
        return isinstance(value, dict)
    if name == "array":
        return isinstance(value, list)
    if name == "string":
        return isinstance(value, str)
    if name == "integer":
        return isinstance(value, int) and not isinstance(value, bool)
    if name == "number":
        return isinstance(value, (int, float)) and not isinstance(value, bool)
    if name == "boolean":
        return isinstance(value, bool)
    if name == "null":
        return value is None
    raise ValueError(f"schema の type が未対応: {name}")


def _resolve_ref(root: dict, ref: str) -> dict:
    if not ref.startswith("#/"):
        raise ValueError(f"schema の $ref が未対応: {ref}")
    node = root
    for part in ref[2:].split("/"):
        node = node[part]
    return node


def check_schema(root: dict, schema: dict, value, pointer: str, out: Violations) -> None:
    """schema の部分集合で value を検査し、違反を out に足す。"""
    if "$ref" in schema:
        check_schema(root, _resolve_ref(root, schema["$ref"]), value, pointer, out)
        return
    if "const" in schema and value != schema["const"]:
        out.add(pointer, f"値は {schema['const']!r} でなければならない（{value!r}）")
        return
    if "enum" in schema and value not in schema["enum"]:
        out.add(pointer, f"値は {schema['enum']} のどれかでなければならない（{value!r}）")
        return
    if "type" in schema:
        names = schema["type"] if isinstance(schema["type"], list) else [schema["type"]]
        if not any(_type_ok(value, n) for n in names):
            out.add(pointer, f"型は {'|'.join(names)} でなければならない（{type(value).__name__}）")
            return
    if isinstance(value, str):
        if "minLength" in schema and len(value) < schema["minLength"]:
            out.add(pointer, f"長さは {schema['minLength']} 以上でなければならない")
        if "maxLength" in schema and len(value) > schema["maxLength"]:
            out.add(pointer, f"長さは {schema['maxLength']} 以下でなければならない")
        if "pattern" in schema and not re.search(schema["pattern"], value):
            out.add(pointer, f"pattern {schema['pattern']} に合わない（{value!r}）")
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        if "minimum" in schema and value < schema["minimum"]:
            out.add(pointer, f"値は {schema['minimum']} 以上でなければならない（{value}）")
        if "maximum" in schema and value > schema["maximum"]:
            out.add(pointer, f"値は {schema['maximum']} 以下でなければならない（{value}）")
    if isinstance(value, list):
        if "minItems" in schema and len(value) < schema["minItems"]:
            out.add(pointer, f"要素は {schema['minItems']} 個以上でなければならない")
        if "items" in schema:
            for i, item in enumerate(value):
                check_schema(root, schema["items"], item, f"{pointer}/{i}", out)
    if isinstance(value, dict):
        for key in schema.get("required", []):
            if key not in value:
                out.add(pointer, f"必須の欄 {key!r} が無い")
        props = schema.get("properties", {})
        for key, sub in props.items():
            if key in value:
                check_schema(root, sub, value[key], f"{pointer}/{key}", out)
        extra = schema.get("additionalProperties", True)
        for key in value:
            if key in props:
                continue
            if extra is False:
                out.add(f"{pointer}/{key}", "schema に無い欄（決定性のため時刻などの追加欄は許さない）")
            elif isinstance(extra, dict):
                check_schema(root, extra, value[key], f"{pointer}/{key}", out)


# ---------------------------------------------------------------- semantics

def is_fail_grade(finding: dict) -> bool:
    return (finding.get("severity") == "error"
            and finding.get("delta") in ("new", "worsened")
            and finding.get("exception") is None)


def expected_verdict(rules: list[dict]) -> tuple[str, int]:
    required = [r for r in rules if r.get("required")]
    if any(r.get("status") in BLOCKING_ERROR for r in required):
        return "error", 2
    if any(r.get("status") == "fail" for r in required):
        return "fail", 1
    return "pass", 0


def check_semantics(report: dict, out: Violations) -> None:
    snap = report["snapshot"]
    rules = report["rules"]
    findings = report["findings"]
    summary = report["summary"]
    policy = report["policy"]
    rule_by_id = {r["id"]: r for r in rules}
    for i, r in enumerate(rules):
        if r["id"] in [x["id"] for x in rules[:i]]:
            out.add(f"/rules/{i}/id", f"rule {r['id']} が重複している")
        if r["status"] != "pass" and not r["reason"].strip():
            out.add(f"/rules/{i}/reason", f"S12: status {r['status']} の reason が空")

    fail_grade_by_rule: dict[str, int] = {}
    seen_fp: dict[str, int] = {}
    for i, f in enumerate(findings):
        p = f"/findings/{i}"
        fs = f["snapshot"]
        if (fs["base"] != snap["base"]["sha"] or fs["head"] != snap["head"]["sha"]
                or fs["dirty"] != snap["head"]["dirty"] or fs["shallow"] != snap["shallow"]):
            out.add(f"{p}/snapshot", "S1: report の snapshot と一致しない")
        if f["rule_id"] not in rule_by_id:
            out.add(f"{p}/rule_id", f"S2: rules に無い rule {f['rule_id']}")
        else:
            st = rule_by_id[f["rule_id"]]["status"]
            if st in ("not_applicable", "unsupported"):
                out.add(f"{p}/rule_id", f"S3: status {st} の rule に finding がある")
        if f["candidate"] and f["severity"] == "error":
            out.add(f"{p}/severity", "S4: candidate の finding は severity error にできない")
        if not f["fingerprint"].startswith(f["rule_id"] + ":"):
            out.add(f"{p}/fingerprint", "S11: fingerprint の先頭は rule_id")
        if f["fingerprint"] in seen_fp:
            out.add(f"{p}/fingerprint", f"S11: fingerprint が /findings/{seen_fp[f['fingerprint']]} と重複")
        seen_fp.setdefault(f["fingerprint"], i)
        if is_fail_grade(f):
            fail_grade_by_rule[f["rule_id"]] = fail_grade_by_rule.get(f["rule_id"], 0) + 1

    for i, r in enumerate(rules):
        n = fail_grade_by_rule.get(r["id"], 0)
        if r["status"] == "pass" and n:
            out.add(f"/rules/{i}/status", f"S3: fail 級の finding が {n} 件あるのに pass")
        if r["status"] == "fail" and not n:
            out.add(f"/rules/{i}/status", "S3: fail 級の finding（severity error・delta new/worsened・exception 無し）が無いのに fail")

    verdict, code = expected_verdict(rules)
    if summary["verdict"] != verdict:
        out.add("/summary/verdict", f"S5: 必須 rule の status からは {verdict} のはず（{summary['verdict']}）")
    if summary["exit_code"] != code:
        out.add("/summary/exit_code", f"S5: verdict {verdict} の exit_code は {code} のはず（{summary['exit_code']}）")

    counts = summary["counts"]
    expected_counts = {
        "findings": len(findings),
        "fail_grade": sum(1 for f in findings if is_fail_grade(f)),
        "new": sum(1 for f in findings if f["delta"] == "new"),
        "worsened": sum(1 for f in findings if f["delta"] == "worsened"),
        "existing": sum(1 for f in findings if f["delta"] == "existing"),
    }
    for k, v in expected_counts.items():
        if counts.get(k) != v:
            out.add(f"/summary/counts/{k}", f"S6: findings から数えると {v}（{counts.get(k)}）")

    reviewable = (snap["head"]["kind"] == "commit" and not snap["head"]["dirty"]
                  and snap["base"]["reachable"])
    if summary["reviewable"] != reviewable:
        out.add("/summary/reviewable", f"S7: head.kind/dirty/base.reachable からは {reviewable} のはず")
    if snap["head"]["kind"] == "commit" and snap["head"]["dirty"]:
        out.add("/snapshot/head/dirty", "S7: commit の head は dirty にならない")

    policy_findings = [f for f in findings if f["rule_id"] == "policy.changed"]
    if policy["changed"] and not policy_findings:
        out.add("/policy/changed", "S8: changed=true なのに rule_id=policy.changed の finding が無い")
    if not policy["changed"] and policy_findings:
        out.add("/policy/changed", "S8: policy.changed の finding があるのに changed=false")
    if policy["source"] == "bootstrap":
        for f in policy_findings:
            if f["severity"] == "error":
                out.add("/policy/source", "S8: bootstrap の policy.changed は severity error にしない")

    if (not snap["base"]["reachable"] or policy["source"] == "unavailable") and summary["verdict"] != "error":
        out.add("/summary/verdict", "S9: base に届かない・policy が読めない report の verdict は error")
    if snap["base"]["sha"] is None and snap["base"]["reachable"]:
        out.add("/snapshot/base/reachable", "S9: base.sha が null なら reachable は false")
    if policy["version"] != report["inputs"]["policy_version"]:
        out.add("/policy/version", "inputs.policy_version と違う policy 版で評価した report は書かれない")

    if report["inputs"]["mode"] == "doc-only":
        for i, r in enumerate(rules):
            if r["id"].startswith(DOC_ONLY_NOT_APPLICABLE_PREFIXES) and r["status"] != "not_applicable":
                out.add(f"/rules/{i}/status", f"S10: doc-only では {r['id']} は not_applicable")

    check_expiry(policy, findings, out)


def _date(value: str, pointer: str, out: Violations) -> datetime.date | None:
    try:
        return datetime.date.fromisoformat(value)
    except ValueError:
        out.add(pointer, f"S13: 日付でない（{value!r}）")
        return None


def check_expiry(policy: dict, findings: list[dict], out: Violations) -> None:
    """S13: 期限付き例外（ADR 2026-11-07-repo-health-contract 付記 2026-11-10）。"""
    as_of = None
    if policy.get("exceptions_as_of") is not None:
        as_of = _date(policy["exceptions_as_of"], "/policy/exceptions_as_of", out)
    for i, f in enumerate(findings):
        p = f"/findings/{i}"
        exc, expired = f["exception"], f.get("expired_exception")
        if expired is not None and exc is not None:
            out.add(f"{p}/expired_exception", "S13: 期限切れの例外の finding は exception が null（免除しない）")
        dated = [(f"{p}/exception/expires", exc["expires"], False)] if exc is not None and "expires" in exc else []
        if expired is not None:
            dated.append((f"{p}/expired_exception/expires", expired["expires"], True))
        for pointer, value, is_expired in dated:
            if "exceptions_as_of" not in policy:
                out.add(pointer, "S13: 期限付きの例外があるのに policy.exceptions_as_of（基準日）が無い")
                continue
            expires = _date(value, pointer, out)
            if expires is None:
                continue
            if is_expired and as_of is not None and as_of <= expires:
                out.add(pointer, f"S13: 基準日 {as_of} は期限 {expires} を過ぎていない（期限内なら exception に書く）")
            if not is_expired and (as_of is None or as_of > expires):
                out.add(pointer, f"S13: 基準日 {as_of} は期限 {expires} を過ぎている（期限切れは免除しない）")



# ---------------------------------------------------------------- entry

def validate_file(path: str, schema: dict) -> list[str]:
    out = Violations(path)
    try:
        with open(path, encoding="utf-8") as fh:
            report = json.load(fh)
    except (OSError, ValueError) as exc:
        out.add("", f"読めない: {exc}")
        return out.items
    check_schema(schema, schema, report, "", out)
    if out.items:
        return out.items
    check_semantics(report, out)
    return out.items


def main(argv: list[str]) -> int:
    if not argv:
        print("使い方: python3 scripts/dev/repo-health/validate_report.py <report.json>...")
        return 1
    with open(SCHEMA_PATH, encoding="utf-8") as fh:
        schema = json.load(fh)
    bad = 0
    for path in argv:
        items = validate_file(path, schema)
        if items:
            bad += 1
            for line in items:
                print(line)
    print(f"validate_report: {len(argv) - bad} valid, {bad} invalid")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
