#!/usr/bin/env python3
"""短い ML 研究キャンペーン case の状態検査（標準ライブラリのみ・LLM なし）。

使い方:
  python3 crates/campaign/scripts/verify-short-ml-case.py --check
  python3 crates/campaign/scripts/verify-short-ml-case.py --placement-check

--check は status.json の状態整合検査で、採用合格ではない（pending / deferred / adopted のどれでも
構造と外部参照の整合だけを見る）。成功しても case が成立した証明にはならない。
  - 外部判断資料（external_refs）が実在し、旧判断が撤回履歴として保存済みであること
  - 不在・不正な外部ファイルは失敗にする（握り潰さない）
--placement-check は status.json の許可 key と、判断材料が repo に残っていないことを検査する。
exit: 0 = 整合 / 1 = 不整合 / 2 = 使い方の誤り。
"""
import json
import re
import sys
from pathlib import Path

CAMPAIGN = Path(__file__).resolve().parent.parent
STATUS_PATH = CAMPAIGN / "optional" / "short-ml-campaign" / "status.json"
STRAY_DIRS = (
    CAMPAIGN / "cases" / "dev" / "short-ml-campaign",
    CAMPAIGN / "cases" / "final" / "short-ml-campaign",
)

SCHEMA = "celeris.short-ml-case-status/2"
TASK_ID = "01M59KY5FTRJTJ80PACNW3K533"
STATUSES = ("pending", "deferred", "adopted")
CLAIM_KEYS = ("final_generalization", "real_pilot_confirmed", "adopted")
COMMON_KEYS = {"schema", "task_id", "status", "label", "claims", "external_refs"}
EXTRA_KEYS = {
    "pending": {"confirm_at"},
    "deferred": {"decided_at"},
    "adopted": {"decided_at", "case_dir"},
}
REF_KEYS = {"reassessment", "review"}
# 判断材料の欄。status.json のどの深さにも置かない（外部資料へ移した）。
JUDGMENT_KEYS = {
    "reasons", "unverified", "inventory_evidence", "resume_conditions", "pins",
    "impact", "evidence_paths", "no_main_comparison_impact", "base_commit",
    "deadline", "selection", "candidates",
}
TS = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")
DATE = re.compile(r"^\d{4}-\d{2}-\d{2}$")
SAVED_KEYS = ("reasons", "unverified", "inventory_evidence", "resume_conditions", "pins")


def fail(errors):
    for e in errors:
        print(f"FAIL: {e}", file=sys.stderr)
    return 1


def load_status(path):
    try:
        s = json.loads(Path(path).read_text())
    except (OSError, ValueError) as e:
        return None, [f"{path}: {e}"]
    if not isinstance(s, dict):
        return None, [f"{path}: top level must be an object"]
    return s, []


def walk_keys(v):
    if isinstance(v, dict):
        for k, x in v.items():
            yield k
            yield from walk_keys(x)
    elif isinstance(v, list):
        for x in v:
            yield from walk_keys(x)


def check_external(refs):
    errors = []
    if not isinstance(refs, dict) or set(refs) != REF_KEYS:
        return [f"external_refs must have exactly {sorted(REF_KEYS)}"]
    for k in sorted(REF_KEYS):
        if not isinstance(refs[k], str) or not Path(refs[k]).is_absolute():
            errors.append(f"external_refs.{k} must be an absolute path")
    if errors:
        return errors
    rp, vp = Path(refs["reassessment"]), Path(refs["review"])
    try:
        d = json.loads(rp.read_text())
    except (OSError, ValueError) as e:
        return [f"external reassessment unreadable: {rp}: {e}"]
    try:
        review = vp.read_text()
    except OSError as e:
        errors.append(f"external review unreadable: {vp}: {e}")
        review = ""
    if review and "撤回した判断" not in review:
        errors.append("external review lacks the withdrawn-decision history")
    try:
        saved = d["withdrawn_decisions"][0]["saved"]["status_json"]
    except (KeyError, IndexError, TypeError):
        return errors + ["reassessment lacks withdrawn_decisions[0].saved.status_json"]
    for k in SAVED_KEYS:
        if not saved.get(k):
            errors.append(f"withdrawn status_json lacks {k} (old judgment not preserved)")
    return errors


def check_state(status_path=STATUS_PATH, campaign=CAMPAIGN):
    s, errors = load_status(status_path)
    if s is None:
        return errors
    if s.get("schema") != SCHEMA:
        errors.append(f"schema must be {SCHEMA}")
    if s.get("task_id") != TASK_ID:
        errors.append(f"task_id must be {TASK_ID}")
    if s.get("label") != "dev":
        errors.append("label must be dev")
    st = s.get("status")
    if st not in STATUSES:
        errors.append(f"status {st!r} must be one of {list(STATUSES)}")
        return errors
    claims = s.get("claims")
    if not isinstance(claims, dict) or set(claims) != set(CLAIM_KEYS):
        errors.append(f"claims must have exactly {list(CLAIM_KEYS)}")
        claims = {}
    for k in ("final_generalization", "real_pilot_confirmed"):
        if claims.get(k) is not False:
            errors.append(f"claims.{k} must be false")
    # 構造上 adopted と書けても、成立の受入は --acceptance が別に見る。
    if claims.get("adopted") is not (st == "adopted"):
        errors.append("claims.adopted must be true only when status is adopted")
    if st == "pending":
        if not TS.match(str(s.get("confirm_at", ""))):
            errors.append("pending needs confirm_at as YYYY-MM-DDTHH:MM:SSZ")
    else:
        if not DATE.match(str(s.get("decided_at", ""))):
            errors.append(f"{st} needs decided_at as YYYY-MM-DD")
    if st == "adopted":
        cd = s.get("case_dir")
        if not isinstance(cd, str) or not (campaign / "cases" / "dev" / cd / "case.json").is_file():
            errors.append("adopted needs case_dir with cases/dev/<case_dir>/case.json")
    else:
        if (campaign / "optional" / "short-ml-campaign" / "case.json").exists():
            errors.append(f"a {st} record must not ship a case.json")
        # pending は準備中の dev case（cases/dev/short-ml-campaign）を持てる。deferred は何も置かない。final は常に置かない。
        for d in STRAY_DIRS:
            if st == "pending" and d == STRAY_DIRS[0]:
                continue
            sd = campaign / d.relative_to(CAMPAIGN)
            if sd.exists():
                errors.append(f"{sd.relative_to(campaign)} must not exist for status {st}")
    errors += check_external(s.get("external_refs"))
    return errors


def check_placement(status_path=STATUS_PATH):
    s, errors = load_status(status_path)
    if s is None:
        return errors
    st = s.get("status")
    allowed = COMMON_KEYS | EXTRA_KEYS.get(st, set())
    for k in sorted(set(s) - allowed):
        errors.append(f"status.json key not allowed for status {st!r}: {k}")
    for k in sorted(set(walk_keys(s)) & JUDGMENT_KEYS):
        errors.append(f"judgment field remains in the repo: {k}")
    return errors


def main(argv):
    if argv == ["--check"]:
        errors, ok = check_state(), "ok: status record is consistent (not an adoption pass)"
    elif argv == ["--placement-check"]:
        errors, ok = check_placement(), "ok: status.json holds only allowed keys"
    else:
        print("usage: verify-short-ml-case.py --check | --placement-check", file=sys.stderr)
        return 2
    if errors:
        return fail(errors)
    print(ok)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
