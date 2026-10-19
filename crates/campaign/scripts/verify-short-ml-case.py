#!/usr/bin/env python3
"""短い ML 研究キャンペーン case の状態検査（標準ライブラリのみ・LLM なし）。

使い方: python3 crates/campaign/scripts/verify-short-ml-case.py --check

status.json が deferred のとき、延期記録の整合だけを検査する（採用合格ではない）:
  - task ID・inventory 証拠・再開条件・主比較影響なしの記録、必須欄・dev ラベル・final 汎化/実 pilot 確認を主張していないこと
  - 延期の前提（grader.rs の sha256、既存 case 集合、local に numpy が無いこと）が今も成立すること
  - 理由と未検証項目が空でなく、証拠パスが存在すること
adopted の記録は、通常 grader での正負例を実行する検査が未実装のためここでは受理しない。
"""
import hashlib
import json
import sys
from pathlib import Path

CAMPAIGN = Path(__file__).resolve().parent.parent
REPO = CAMPAIGN.parent.parent
STATUS_DIR = CAMPAIGN / "optional" / "short-ml-campaign"
STRAY = CAMPAIGN / "cases" / "dev" / "short-ml-campaign"


def fail(errors):
    for e in errors:
        print(f"FAIL: {e}", file=sys.stderr)
    return 1


def check():
    errors = []
    p = STATUS_DIR / "status.json"
    try:
        s = json.loads(p.read_text())
    except (OSError, ValueError) as e:
        return fail([f"{p}: {e}"])
    if s.get("schema") != "celeris.short-ml-case-status/1":
        errors.append("schema must be celeris.short-ml-case-status/1")
    if s.get("label") != "dev":
        errors.append("label must be dev")
    claims = s.get("claims", {})
    for k in ("final_generalization", "real_pilot_confirmed", "adopted"):
        if claims.get(k) is not False:
            errors.append(f"claims.{k} must be false")
    if s.get("status") != "deferred":
        errors.append(
            f"status {s.get('status')!r}: only a deferred record can be checked here; "
            "adoption needs the normal-grader positive/negative run, which is not implemented"
        )
        return fail(errors)
    for k in ("reasons", "unverified", "evidence_paths"):
        if not s.get(k):
            errors.append(f"{k} must be non-empty")
    for r in s.get("reasons", []):
        if not (r.get("id") and r.get("text")):
            errors.append("each reason needs id and text")
    if s.get("task_id") != "01M59KY5FTRJTJ80PACNW3K533":
        errors.append("task_id must be 01M59KY5FTRJTJ80PACNW3K533")
    if s.get("no_main_comparison_impact") is not True:
        errors.append("no_main_comparison_impact must be true")
    inv = s.get("inventory_evidence", {})
    if not (inv.get("commit") and inv.get("wu") == "inventory"):
        errors.append("inventory_evidence needs wu=inventory and commit")
    if not s.get("resume_conditions"):
        errors.append("resume_conditions must be non-empty")
    for k in ("decided_at", "deadline", "base_commit", "impact"):
        if not s.get(k):
            errors.append(f"{k} is required")
    for e in s.get("evidence_paths", []):
        if not (REPO / e).is_file():
            errors.append(f"evidence path missing: {e}")
    pins = s.get("pins", {})
    g = pins.get("grader_source", {})
    gp = REPO / g.get("path", "")
    if not gp.is_file() or hashlib.sha256(gp.read_bytes()).hexdigest() != g.get("sha256"):
        errors.append("grader.rs changed since the deferral (R2 premise must be re-examined)")
    for split, want in pins.get("existing_case_dirs", {}).items():
        d = CAMPAIGN / "cases" / split
        have = sorted(x.name for x in d.iterdir() if x.is_dir())
        if have != sorted(want):
            errors.append(f"{split} case set changed: {have} != {sorted(want)}")
    fd = CAMPAIGN / "cases" / "final"
    names = sorted(x.name for x in fd.iterdir() if x.is_dir())
    if hashlib.sha256("\n".join(names).encode()).hexdigest() != pins.get("final_case_dirs_sha256"):
        errors.append("final case set changed")
    if STRAY.exists():
        errors.append("cases/dev must not hold a deferral-only directory (short-ml-campaign)")
    if (STATUS_DIR / "case.json").exists():
        errors.append("a deferred record must not ship a case.json")
    if (CAMPAIGN / "cases" / "final" / "short-ml-campaign").exists():
        errors.append("cases/final must not hold a deferral-only directory")
    for m in pins.get("local_env", {}).get("missing_modules", []):
        try:
            __import__(m)
            errors.append(f"module {m} is now importable (R1 premise must be re-examined)")
        except ImportError:
            pass
    if errors:
        return fail(errors)
    print("ok: deferred record is consistent (not an adoption pass)")
    return 0


if __name__ == "__main__":
    if sys.argv[1:] != ["--check"]:
        print("usage: verify-short-ml-case.py --check", file=sys.stderr)
        sys.exit(2)
    sys.exit(check())
