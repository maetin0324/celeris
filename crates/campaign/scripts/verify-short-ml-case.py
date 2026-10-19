#!/usr/bin/env python3
"""短い ML 研究キャンペーン case の状態検査（標準ライブラリのみ・LLM なし）。

使い方:
  python3 crates/campaign/scripts/verify-short-ml-case.py --check
  python3 crates/campaign/scripts/verify-short-ml-case.py --placement-check
  python3 crates/campaign/scripts/verify-short-ml-case.py --acceptance --evidence <ml-case-reassessment.json> [--grader-bin <celeris-campaign>]

--check は status.json の状態整合検査で、採用合格ではない（pending / deferred / adopted のどれでも
構造と外部参照の整合だけを見る）。成功しても case が成立した証明にはならない。
  - 外部判断資料（external_refs）が実在し、旧判断が撤回履歴として保存済みであること
  - 不在・不正な外部ファイルは失敗にする（握り潰さない）
--placement-check は status.json の許可 key と、判断材料が repo に残っていないことを検査する。
--acceptance は case の成立（adopted）か延期（deferred）の受入検査で、--check とは別物。evidence の status で分岐する。
  - pending / 未知の status / 承認不足 / 未検証は非 0（現状の pending 証拠は非 0 が正しい）
  - adopted: 固定 case・入力・環境 lock・grader の pin を実ファイルと照合し、評価側が実観測した job 記録（origin=live）と
    raw hash を確かめ、通常の grader CLI（celeris-campaign grade）を新しい workdir で正例 1 と独立な負例 3
    （誤計算 / 比較条件違い / 未実行）について実際に再実行する。各 exit と判定理由を検証し、報告 JSON の読み取りでは代替しない。
  - deferred: 実時計が確認点以後で、期限後の再確認（実日時・実コマンド・exit・出力 hash）が外部ファイルと一致し、不成立であること。
  実時計は上書きできない（--now は無い）。時計注入は Python API の test 専用で、synthetic 証拠にしか使えず本番の延期証拠にならない。
exit: 0 = 整合 / 1 = 不整合 / 2 = 使い方の誤り。
"""
import datetime
import hashlib
import json
import os
import re
import shutil
import struct
import subprocess
import sys
import tempfile
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


CHECKPOINT = "2026-10-22T00:00:00Z"
CASE_ID = "short-ml-campaign"
FIXTURES = CAMPAIGN / "fixtures"
NEGATIVES = (
    # name, 落ちるべき check, 通るべき check, 意図した拒否理由（その check の reasons に含まれる語）
    ("miscalc", "recompute", "evidence", "metric coef_x1"),
    ("wrong-condition", "evidence", "recompute", "condition queue"),
    ("unexecuted", "evidence", "recompute", "not recorded by the evaluator"),
)
ENV_KEYS = ("python", "numpy")
SHA = re.compile(r"^[0-9a-f]{64}$")


def parse_ts(v):
    if not isinstance(v, str) or not TS.match(v):
        return None
    return datetime.datetime.strptime(v, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=datetime.timezone.utc)


def sha_file(p):
    return hashlib.sha256(Path(p).read_bytes()).hexdigest()


def hash_tree(root):
    """crates/campaign/src/snapshot.rs の hash_tree と同じ（相対 path・内容・空 dir）。"""
    h = hashlib.sha256()
    root = str(root)

    def walk(d):
        for n in sorted(os.listdir(d), key=lambda x: x.encode()):
            p = os.path.join(d, n)
            rel = os.path.relpath(p, root).encode()
            if os.path.islink(p):
                raise ValueError(f"symlink is not allowed: {p}")
            if os.path.isdir(p):
                h.update(b"D" + struct.pack("<Q", len(rel)) + rel)
                walk(p)
            else:
                b = Path(p).read_bytes()
                h.update(b"F" + struct.pack("<Q", len(rel)) + rel + struct.pack("<Q", len(b)) + b)

    walk(root)
    return h.hexdigest()


def read_obj(path, what):
    try:
        v = json.loads(Path(path).read_text())
    except (OSError, ValueError) as e:
        return None, [f"{what} unreadable: {path}: {e}"]
    if not isinstance(v, dict):
        return None, [f"{what} must be a JSON object: {path}"]
    return v, []


def find_grader(explicit):
    cands = [explicit] if explicit else []
    if os.environ.get("CELERIS_CAMPAIGN_BIN"):
        cands.append(os.environ["CELERIS_CAMPAIGN_BIN"])
    td = os.environ.get("CARGO_TARGET_DIR")
    if td:
        cands += [Path(td) / "debug" / "celeris-campaign", Path(td) / "release" / "celeris-campaign"]
    for c in cands:
        if c and Path(c).is_file() and os.access(c, os.X_OK):
            return str(c)
    return None


def check_pins(campaign):
    """contract.json の pin を実ファイルと照合する（case / 入力 / 環境 lock / grader の改竄を拒否）。"""
    cd = campaign / "cases" / "dev" / CASE_ID
    errors = []
    k, e = read_obj(cd / "contract.json", "contract")
    if k is None:
        return e, None
    case, e = read_obj(cd / "case.json", "case.json")
    if case is None:
        return e, None
    pins = k.get("pins") if isinstance(k.get("pins"), dict) else {}
    gref, pref = case.get("grade_case_ref") or {}, case.get("private_ref") or {}
    fx = campaign / "fixtures"
    actual = {}
    try:
        actual = {
            "case_json": sha_file(cd / "case.json"),
            "env_lock": sha_file(cd / "env.lock.json"),
            "materials_sha256": hash_tree(cd / "materials"),
            "kb_snapshot": hash_tree(cd / "kb"),
            "grade_case": sha_file(fx / "grade-cases" / gref["path"]),
            "private": sha_file(fx / "grader-private" / pref["path"]),
        }
    except (OSError, KeyError, ValueError, TypeError) as ex:
        errors.append(f"pinned file unreadable: {ex}")
    for name, got in actual.items():
        if pins.get(name) != got:
            errors.append(f"pin {name} does not match the file (tampered or stale)")
    # case.json が持つ pin（grade-case / private / snapshot）も契約と一致していること。
    for name, ref in (("grade_case", gref), ("private", pref)):
        if ref.get("sha256") != pins.get(name):
            errors.append(f"case.json {name} hash disagrees with the contract pin")
    snap = case.get("snapshot") or {}
    for a, b in (("materials_sha256", "materials_sha256"), ("kb_snapshot", "kb_snapshot")):
        if snap.get(a) != pins.get(b):
            errors.append(f"case.json snapshot.{a} disagrees with the contract pin")
    ins = pins.get("inputs") if isinstance(pins.get("inputs"), dict) else {}
    if not ins:
        errors.append("contract pins no inputs")
    for name, want in ins.items():
        f = fx / "inputs" / CASE_ID / name
        if not f.is_file() or sha_file(f) != want:
            errors.append(f"fixed input {name} does not match its pin")
    if {i.get("name"): i.get("sha256") for i in case.get("inputs", [])} != ins:
        errors.append("case.json inputs disagree with the contract pins")
    priv, e = read_obj(fx / "grader-private" / pref.get("path", "x"), "private.json") if pref else (None, [])
    if priv is not None:
        for name, want in ((priv.get("expected") or {}).get("withheld") or {}).items():
            f = fx / "grader-private" / "dev" / CASE_ID / name
            if not f.is_file() or sha_file(f) != want:
                errors.append(f"grader-private {name} does not match its pin")
    lock, e = read_obj(cd / "env.lock.json", "env.lock")
    return errors, lock


def run_grader(grader, case_path, artifact, observed, work):
    cmd = [grader, "grade", "--case", str(case_path), "--artifact", str(artifact),
           "--observed", str(observed), "--work", str(work)]
    p = subprocess.run(cmd, capture_output=True, text=True, timeout=300, stdin=subprocess.DEVNULL)
    out = p.stdout + p.stderr
    report = None
    if "{" in out:
        try:
            report = json.loads(out[out.index("{"):])
        except ValueError:
            pass
    return {"cmd": cmd, "exit": p.returncode, "output": out, "report": report,
            "output_sha256": hashlib.sha256(out.encode()).hexdigest()}


def check_status(report, name):
    c = ((report or {}).get("checks") or {}).get(name) or {}
    return c.get("status"), c.get("reasons") or [], c.get("env_failure")


def tree_files(root):
    out = {}
    for d, _, fs in os.walk(root):
        for f in fs:
            p = Path(d) / f
            out[str(p.relative_to(root))] = sha_file(p)
    return out


def mutate_report(path):
    lines, hit = [], False
    for ln in Path(path).read_text().splitlines():
        m = re.match(r"^(\s*metric\s+coef_x1\s*=\s*)(\S+)(.*)$", ln)
        if m and not hit:
            try:
                ln, hit = f"{m.group(1)}{float(m.group(2)) * 2 + 1.0!r}{m.group(3)}", True
            except ValueError:
                pass
        lines.append(ln)
    if not hit:
        raise ValueError("report.md has no numeric `metric coef_x1 = <v>` line to miscalculate")
    Path(path).write_text("\n".join(lines) + "\n")


def build_trials(artifact, observed, base):
    """正例と独立な負例 3 件の fresh workdir を作る。負例は正例の 1 点だけを変える。"""
    trials = {}
    obs = json.loads(Path(observed).read_text())
    variants = {
        "positive": (lambda a, o: o),
        "miscalc": (lambda a, o: (mutate_report(a / "report.md"), o)[1]),
        "wrong-condition": (lambda a, o: {**o, "jobs": [{**j, "conditions": {"queue": "workq", "ncpus": 4}} for j in o["jobs"]]}),
        "unexecuted": (lambda a, o: {**o, "jobs": []}),
    }
    for name, fn in variants.items():
        wd = base / name
        a = wd / "artifact"
        shutil.copytree(artifact, a, symlinks=True)
        o = fn(a, json.loads(json.dumps(obs)))
        (wd / "observed.json").write_text(json.dumps(o))
        (wd / "work").mkdir()
        trials[name] = wd
    return trials


def check_adoption(ev, campaign, status, grader, runs_out):
    errors = []
    ad = ev.get("adoption")
    if not isinstance(ad, dict):
        return ["adopted evidence lacks the adoption block"]
    pin_errors, lock = check_pins(campaign)
    errors += pin_errors
    # 環境: lock の要求値を評価側が測定した値と照合する。lock の requested_unverified を測定済みと扱わない。
    em = ad.get("env_measured")
    req = (lock or {}).get("requested") or {}
    if not isinstance(em, dict) or any(not em.get(k) for k in ENV_KEYS) or em.get("exit") != 0 or not em.get("command"):
        errors.append("env_measured needs command, exit 0, python and numpy (environment not verified)")
    else:
        for k in ENV_KEYS:
            if em[k] != req.get(k):
                errors.append(f"measured {k} {em[k]!r} differs from the locked request {req.get(k)!r}")
    # 承認・残枠: survey 時点の数字を現残枠とみなさない。承認と、新しい as_of の残枠を要求する。
    ap = ad.get("approval")
    if not isinstance(ap, dict) or ap.get("approved") is not True:
        errors.append("dependency approval is missing or not granted")
    else:
        rem = ap.get("remaining")
        as_of = parse_ts(rem.get("as_of")) if isinstance(rem, dict) else None
        if as_of is None or not isinstance(rem.get("tokens"), int) or isinstance(rem.get("tokens"), bool) or rem["tokens"] < 0:
            errors.append("approval.remaining needs integer tokens and as_of timestamp (stale survey numbers are not the current balance)")
        elif rem.get("source") in (None, "", "survey", "ml-case-reassessment"):
            errors.append("approval.remaining needs a current source, not the survey")
    # 評価側の観測と raw。
    art, obs = ad.get("artifact"), ad.get("observed_jobs")
    for k, v in (("artifact", art), ("observed_jobs", obs)):
        if not isinstance(v, str) or not Path(v).is_absolute():
            errors.append(f"adoption.{k} must be an absolute path")
    if errors or not (Path(art).is_dir() and Path(obs).is_file()):
        return errors or ["adoption artifact or observed_jobs is missing"]
    od, e = read_obj(obs, "observed_jobs")
    if od is None:
        return errors + e
    if od.get("schema") != "celeris.observed-jobs/1":
        errors.append("observed_jobs schema must be celeris.observed-jobs/1")
    want_origin = "synthetic" if ev.get("origin") == "synthetic" else "live"
    if od.get("origin") != want_origin:
        errors.append(f"observed_jobs origin {od.get('origin')!r} must be {want_origin!r} (synthetic is not production evidence)")
    jobs = od.get("jobs")
    if not isinstance(jobs, list) or not jobs:
        errors.append("observed_jobs has no job (not executed)")
    else:
        for j in jobs:
            for r in j.get("raw", []):
                f = Path(art) / str(r.get("path"))
                if not f.is_file() or sha_file(f) != r.get("sha256"):
                    errors.append(f"raw {r.get('path')} does not match its observed hash")
            if j.get("exit_status") != 0:
                errors.append(f"job {j.get('job_id')} exit_status is not 0")
    tp = Path(art) / "inputs" / "train.csv"
    want = (json.loads((campaign / "cases/dev" / CASE_ID / "contract.json").read_text()).get("pins") or {}).get("inputs", {}).get("train.csv")
    if not tp.is_file() or sha_file(tp) != want:
        errors.append("artifact inputs/train.csv does not match the pinned input")
    if errors:
        return errors
    if grader is None:
        return ["grader binary not found (build celeris-campaign or pass --grader-bin)"]
    gcase = campaign / "fixtures" / "grade-cases" / "dev" / CASE_ID / f"{CASE_ID}.json"
    with tempfile.TemporaryDirectory(prefix="smlacc-") as td:
        try:
            trials = build_trials(art, obs, Path(td))
        except (OSError, ValueError, KeyError) as ex:
            return [f"cannot build fresh trials: {ex}"]
        pos = tree_files(trials["positive"] / "artifact")
        for name, _f, _p, _w in NEGATIVES:
            diff = {k for k in set(pos) | set(tree_files(trials[name] / "artifact")) if pos.get(k) != tree_files(trials[name] / "artifact").get(k)}
            obs_same = (trials[name] / "observed.json").read_text() == (trials["positive"] / "observed.json").read_text()
            want_diff = {"report.md"} if name == "miscalc" else set()
            if diff != want_diff or (name == "miscalc") != obs_same:
                errors.append(f"negative {name} is not independent (differs from the positive in more than its own point)")
        res = {}
        for name in ("positive", *(n for n, _, _, _ in NEGATIVES)):
            wd = trials[name]
            r = run_grader(grader, gcase, wd / "artifact", wd / "observed.json", wd / "work")
            res[name] = r
            runs_out.append({"name": name, "cmd": r["cmd"], "exit": r["exit"], "output_sha256": r["output_sha256"]})
        p = res["positive"]
        if (p["exit"] != 0 or not (p["report"] or {}).get("final_pass") is True
                or check_status(p["report"], "evidence")[0] != "pass" or check_status(p["report"], "recompute")[0] != "pass"):
            errors.append(f"positive run was not accepted by the grader (exit {p['exit']})")
        for name, bad, good, why in NEGATIVES:
            r = res[name]
            if r["exit"] == 0 or (r["report"] or {}).get("final_pass") is True:
                errors.append(f"negative {name} was accepted by the grader (must be rejected)")
                continue
            bs, reasons, envf = check_status(r["report"], bad)
            if bs != "fail" or envf or not any(why in x for x in reasons):
                errors.append(f"negative {name} was not rejected by {bad} for the intended reason ({why!r})")
            if check_status(r["report"], good)[0] != "pass":
                errors.append(f"negative {name} also failed {good}: reasons are not independent")
        claimed = ad.get("runs")
        if claimed is not None:
            ok = isinstance(claimed, list) and {c.get("name"): c.get("exit") for c in claimed if isinstance(c, dict)} == {r["name"]: r["exit"] for r in runs_out}
            if not ok:
                errors.append("claimed runs disagree with the re-executed grader exits")
    return errors


def check_deferral(ev, status, now):
    errors = []
    cp = parse_ts(CHECKPOINT)
    if (ev.get("deadline") or {}).get("checkpoint") != CHECKPOINT:
        errors.append(f"evidence deadline.checkpoint must be {CHECKPOINT}")
    if now < cp:
        return errors + [f"real clock {now:%Y-%m-%dT%H:%M:%SZ} is before the checkpoint {CHECKPOINT}: deferral cannot be recorded yet"]
    d = ev.get("deferral")
    rc = d.get("recheck") if isinstance(d, dict) else None
    if not isinstance(rc, dict):
        return errors + ["deferred evidence lacks deferral.recheck"]
    at = parse_ts(rc.get("at"))
    if at is None:
        errors.append("recheck.at must be YYYY-MM-DDTHH:MM:SSZ")
    elif at < cp:
        errors.append("recheck was recorded before the checkpoint (not a post-deadline recheck)")
    elif at > now:
        errors.append("recheck.at is in the future of the real clock")
    if not isinstance(rc.get("command"), str) or not rc["command"].strip():
        errors.append("recheck.command is missing")
    if not isinstance(rc.get("exit"), int) or isinstance(rc.get("exit"), bool):
        errors.append("recheck.exit must be an integer")
    elif rc["exit"] == 0:
        errors.append("recheck.exit is 0: the case was established, so deferral is not supported")
    if rc.get("result") != "not_established":
        errors.append("recheck.result must be not_established")
    op, sh = rc.get("output_path"), rc.get("output_sha256")
    if not isinstance(op, str) or not Path(op).is_absolute() or not Path(op).is_file():
        errors.append("recheck.output_path must be an existing absolute external file")
    elif not isinstance(sh, str) or not SHA.match(sh) or sha_file(op) != sh:
        errors.append("recheck output hash does not match the external output file")
    elif not Path(op).read_text(errors="replace").strip():
        errors.append("recheck output file is empty")
    da = DATE.match(str(status.get("decided_at", "")))
    if not da:
        errors.append("status.json deferred needs decided_at")
    elif at is not None and status["decided_at"] < f"{at:%Y-%m-%d}":
        errors.append("status.json decided_at precedes the recheck date")
    return errors


def acceptance(evidence_path, status_path=STATUS_PATH, campaign=CAMPAIGN, grader_bin=None,
               now=None, allow_synthetic=False):
    """(errors, runs)。now / allow_synthetic は test 専用（CLI からは渡せない）。"""
    runs = []
    if now is not None and not allow_synthetic:
        return ["an injected clock is test-only"], runs
    clock = now if now is not None else datetime.datetime.now(datetime.timezone.utc)
    ev, errors = read_obj(evidence_path, "evidence")
    if ev is None:
        return errors, runs
    origin = ev.get("origin")
    synthetic = origin == "synthetic"
    if synthetic and not allow_synthetic:
        return ["synthetic evidence is not production evidence"], runs
    if now is not None and not synthetic:
        return ["an injected clock cannot validate real evidence"], runs
    s, errors = load_status(status_path)
    if s is None:
        return errors, runs
    errors = check_state(status_path, campaign)
    st = ev.get("status")
    if st not in ("adopted", "deferred"):
        return errors + [f"evidence status {st!r} is not adopted or deferred (pending / unverified is not accepted)"], runs
    if s.get("status") != st:
        errors.append(f"status.json status {s.get('status')!r} disagrees with evidence status {st!r}")
    if errors:
        return errors, runs
    if st == "adopted":
        errors += check_adoption(ev, campaign, s, find_grader(grader_bin), runs)
    else:
        errors += check_deferral(ev, s, clock)
    return errors, runs


def usage():
    print("usage: verify-short-ml-case.py --check | --placement-check | --acceptance --evidence <file> [--grader-bin <path>]", file=sys.stderr)
    return 2


def main(argv):
    if argv == ["--check"]:
        errors, ok = check_state(), "ok: status record is consistent (not an adoption pass)"
    elif argv == ["--placement-check"]:
        errors, ok = check_placement(), "ok: status.json holds only allowed keys"
    elif argv[:1] == ["--acceptance"]:
        rest = argv[1:]
        opts = {}
        while rest:
            if rest[0] not in ("--evidence", "--grader-bin") or len(rest) < 2 or rest[0] in opts or rest[1].startswith("--"):
                return usage()
            opts[rest[0]], rest = rest[1], rest[2:]
        if "--evidence" not in opts:
            return usage()
        errors, runs = acceptance(opts["--evidence"], grader_bin=opts.get("--grader-bin"))
        ok = "ok: acceptance passed (" + (f"{len(runs)} grader runs re-executed" if runs else "post-deadline deferral verified") + ")"
    else:
        return usage()
    if errors:
        return fail(errors)
    print(ok)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
