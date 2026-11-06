#!/usr/bin/env python3
"""P4-G: Project Controller の本番 1 案件の限定導入（dry_run → shadow → canary → enabled）の運用の道具。

ADR 2026-11-06-phase4-p4g-shadow-rollout。手順書は docs/ops/project-controller.md の「1 案件の限定導入（P4-G）」。

  preflight  go/no-go の判定。daemon の API（GET だけ）・config の file（読むだけ）・git・承認の file・監視の報告を読み、
             段（dry_run・shadow・canary・enabled）ごとの条件を 1 つずつ pass / fail / pending で出す。何も変えない。
  snapshot   監視の 1 標本（mode・判断の効果の数・controller の root の数・CoS の入力数・RSS・GET の遅れ）。GET だけ。
  diff       2 つの標本の差を見て、段の不変条件に反する差（効果の適用・root の起票・CoS の入力・他の案件や領域の変化・
             config の差し替え）を violations に出す。違反があれば exit 1。
  monitor    snapshot を間隔ごとに取り、基準との diff を毎回出す。違反で止まる（--undo-on-violation なら undo も行う）。
  apply      1 段だけ上げる。段の承認（approved_by=human:<名前>・期限・案件・API・scope）が要る。shadow の承認では
             canary・enabled に上げられない。canary・enabled の承認は shadow の監視の報告（go）の sha256 を引く。
  undo       戻す: 案件の mode を off に（誰でも可）、必要なら領域を compare か rust に。前後の標本を残す。
  e2e        非本番の一時 daemon（fake harness、一時 home、127.0.0.1）で上の操作を同じ順に通し、raw を書く。

境界:
- LLM を呼ばない。外部ネットワークに出ない。本番の DB・config・systemd・releases に触れない（e2e の home が本番の path
  の下なら exit 2。apply・undo は Celeris の worker の run の中では scope=production の承認を拒む）。
- 監視は GET だけで、CPU を焼く負荷をかけない（既定 60 秒に 1 回）。
"""
import argparse
import datetime
import hashlib
import json
import os
import pathlib
import re
import secrets
import signal
import socket
import subprocess
import sys
import tempfile
import time
import tomllib
import urllib.error
import urllib.request

SCHEMA = "celeris.pc-rollout/1"
STAGES = ["dry_run", "shadow", "canary", "enabled"]
# 段ごとに要る案件 mode と領域 mode（docs/ops/project-controller.md の「導入の checklist」4）。
STAGE_PROJECT_MODE = {"dry_run": "dry_run", "shadow": "shadow", "canary": "enabled", "enabled": "enabled"}
STAGE_DOMAIN_MODES = {"dry_run": ("compare",), "shadow": ("compare",), "canary": ("canary",), "enabled": ("ts",)}
# 承認の種類。shadow の承認（観察の開始）と effects を伴う承認（canary・enabled）を別の file にする。
APPROVAL_KINDS = {"dry_run": "observe", "shadow": "observe", "canary": "effects", "enabled": "effects"}
# canary・enabled に進む前に要る shadow の観察の長さ（秒）。
MIN_SHADOW_OBSERVE_SECS = 24 * 3600
# 本番の shadow の前に main に入っていることを求める修正（件名の先頭。commit hash は rebase で変わるので書かない）。
REQUIRED_SUBJECTS = [
    "phase4 cos-usage: 案件 CoS の chat run が usage_json を記録し予算の和に入ることを",
    "p4b: 達成の後の根拠の失効で Rust が数え直して waiting に戻す",
    "p4c: CoS の一次対応の受信箱を触れた task の分だけ作り",
    "p4d: retention GC は 3 表とも最大 id の行を残す",
]
PRODUCTION_PREFIXES = ("/local/celeris/state", "/local/celeris/data/db",
                       str(pathlib.Path.home() / ".local/celeris"), str(pathlib.Path.home() / ".config/celeris"))
# 効果を伴った（伴おうとした）判断の効果の状態。`none`（wait など効果の無い判断）と `suppressed` は観察の段で正しい。
EFFECTFUL = ("pending", "applied", "failed")
DOMAIN_RANK = {"rust": 0, "compare": 1, "canary": 2, "ts": 3}
BAD_WAIT_RULES = ("pc/invalid", "pc/script-unavailable")


# ---------------------------------------------------------------- 純粋な関数（test_pc_rollout.py が試す）

def now_utc():
    return datetime.datetime.now(datetime.timezone.utc)


def parse_time(text):
    return datetime.datetime.fromisoformat(str(text).replace("Z", "+00:00"))


def sha256_file(path):
    return hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()


def check_approval(approval, stage, project, api, env, now=None, shadow_report_sha=None):
    """段の承認の検査。(ok, reason)。"""
    if approval is None:
        return False, f"no approval file for stage {stage}"
    by = str(approval.get("approved_by", ""))
    if not re.fullmatch(r"human:\S+", by):
        return False, "approval.approved_by must be human:<name>"
    kind = APPROVAL_KINDS[stage]
    if approval.get("kind") != kind:
        return False, f"stage {stage} needs an approval of kind {kind} (got {approval.get('kind')})"
    if stage not in (approval.get("stages") or []):
        return False, f"stage {stage} is not in approval.stages"
    scope = approval.get("scope")
    if scope not in ("production", "non-production"):
        return False, "approval.scope must be production or non-production"
    if scope == "production" and env.get("CELERIS_RUN_ID"):
        return False, "a production approval is not used inside a Celeris worker run (CELERIS_RUN_ID is set)"
    if approval.get("project_id") != project:
        return False, f"approval.project_id ({approval.get('project_id')}) is not {project}"
    if approval.get("api") != api:
        return False, f"approval.api ({approval.get('api')}) is not {api}"
    if not re.fullmatch(r"[0-9a-f]{64}", str(approval.get("generation", ""))):
        return False, "approval.generation must be the bundle_sha256 (64 hex) the person approved"
    try:
        expires = parse_time(approval.get("expires_at"))
    except (TypeError, ValueError):
        return False, "approval.expires_at must be an ISO 8601 time"
    if expires <= (now or now_utc()):
        return False, "the approval has expired"
    if kind == "effects":
        want = approval.get("shadow_report_sha256")
        if not want:
            return False, "an effects approval must cite shadow_report_sha256 (the shadow monitor report)"
        if shadow_report_sha != want:
            return False, "the shadow report given (--shadow-report) does not match approval.shadow_report_sha256"
        if not isinstance(approval.get("canary_bps"), int) and stage == "canary":
            return False, "a canary approval must give canary_bps (1..9999)"
    return True, f"approved by {by} ({scope})"


def shadow_report_ok(report, min_secs=MIN_SHADOW_OBSERVE_SECS):
    """shadow の監視の報告が canary に進む条件を満たすか。(ok, reason)。"""
    if report is None:
        return False, "no shadow monitor report (--shadow-report)"
    if report.get("schema") != SCHEMA or report.get("kind") != "monitor":
        return False, "not a pc-rollout monitor report"
    if report.get("stage") != "shadow":
        return False, f"the report is for stage {report.get('stage')}, not shadow"
    if report.get("scope") != "production" and not report.get("allow_non_production"):
        return False, "the report was not taken on production"
    secs = report.get("observed_secs") or 0
    if secs < min_secs:
        return False, f"observed {secs} s < {min_secs} s"
    if report.get("violations"):
        return False, f"{len(report['violations'])} violations in the report"
    return True, f"observed {secs} s, 0 violations"


def _loads(text):
    try:
        return json.loads(text or "null")
    except ValueError:
        return None


def domain_row(domains, name="project_controller"):
    for d in (domains or {}).get("domains", []):
        if d.get("domain") == name:
            return d
    return None


def find_key(doc, key):
    """入れ子の dict の中から最初の key の値を探す（daemon の応答の形が版で少し違うため）。"""
    if isinstance(doc, dict):
        if key in doc:
            return doc[key]
        for v in doc.values():
            r = find_key(v, key)
            if r is not None:
                return r
    elif isinstance(doc, list):
        for v in doc:
            r = find_key(v, key)
            if r is not None:
                return r
    return None


def criteria(stage, facts, scope):
    """go/no-go の条件。facts は collect() が集めた値。各行 {id, what, state(pass|fail|pending|waived), detail}。"""
    rows = []

    def add(cid, what, state, detail):
        rows.append({"id": cid, "what": what, "state": state, "detail": detail})

    # G1 必要な修正が main にあり、稼働中の release がそれを含む。
    git = facts.get("git") or {}
    missing = git.get("missing_subjects")
    if missing is None:
        add("G1", "必要な修正が main にある（cos-usage・P4-B・P4-C・P4-D）", "pending", "git を読めない")
    elif missing:
        add("G1", "必要な修正が main にある（cos-usage・P4-B・P4-C・P4-D）", "fail", f"main に無い: {missing}")
    else:
        add("G1", "必要な修正が main にある（cos-usage・P4-B・P4-C・P4-D）", "pass", "全件 main にある")
    rel = git.get("release_contains_required")
    if scope == "non-production":
        add("G1r", "稼働中の release が必要な修正を含む", "waived", "非本番の予行（一時 daemon の release 名は sha でない）")
    elif rel is None:
        add("G1r", "稼働中の release が必要な修正を含む", "pending", git.get("release_detail", "release の sha を読めない"))
    else:
        add("G1r", "稼働中の release が必要な修正を含む", "pass" if rel else "fail", git.get("release_detail", ""))
    # G2 config: 有効・対象は 1 案件だけ・daemon が読んだ config と同じ file。
    cfg = facts.get("config") or {}
    if not cfg.get("read"):
        add("G2", "[project_controller] enabled=true・allowed_project_ids が対象の 1 案件だけ", "pending",
            cfg.get("error", "config を読んでいない（--config）"))
    else:
        ok = cfg.get("enabled") is True and cfg.get("allowed_project_ids") == [facts["project"]]
        add("G2", "[project_controller] enabled=true・allowed_project_ids が対象の 1 案件だけ", "pass" if ok else "fail",
            f"enabled={cfg.get('enabled')} allowed={cfg.get('allowed_project_ids')}")
        same = cfg.get("sha256") == facts.get("daemon_config_sha256")
        add("G2s", "読んだ config が daemon の起動時の config と同じ（sha256）", "pass" if same else "fail",
            f"file={cfg.get('sha256')} daemon={facts.get('daemon_config_sha256')}")
        b = cfg.get("budget_present")
        add("G2b", "[project_controller.budget] の上限が summary に出ている", "pass" if b else "fail",
            f"budget_limits={facts.get('budget_limits')}")
    # G3 他の案件は全部 off。
    others = facts.get("other_modes") or {}
    on = {p: m for p, m in others.items() if m != "off"}
    add("G3", "対象以外の案件の controller が全部 off", "fail" if on else "pass", f"off でない: {on}" if on else
        f"{len(others)} 件とも off")
    # G4 領域 mode。
    dom = facts.get("domain") or {}
    want = STAGE_DOMAIN_MODES[stage]
    prev = {"dry_run": ("rust", "compare"), "shadow": ("compare",), "canary": ("compare", "canary"),
            "enabled": ("canary", "ts")}[stage]
    mode = dom.get("mode")
    if facts.get("guard_halted"):
        add("G4", "script guard が止めていない", "fail", f"guard_halted={facts['guard_halted']}")
    else:
        add("G4", "script guard が止めていない", "pass", "guard_halted=null")
    add("G4m", f"領域 project_controller が {'/'.join(want)} にできる状態", "pass" if mode in prev or mode in want
        else "fail", f"mode={mode} generation_state={dom.get('generation_state')}")
    # G5 前の段の結果。
    st = facts.get("controller") or {}
    cur_mode = st.get("mode")
    if stage == "dry_run":
        add("G5", "案件の controller が off か dry_run", "pass" if cur_mode in ("off", "dry_run") else "fail",
            f"mode={cur_mode}")
    elif stage == "shadow":
        dec = facts.get("decisions") or {}
        ok = cur_mode in ("dry_run", "shadow") and dec.get("dry_run", 0) >= 1 and dec.get("not_suppressed", 0) == 0
        add("G5", "dry_run の判断が 1 件以上あり、全部 suppressed", "pass" if ok else "fail",
            f"mode={cur_mode} decisions={dec}")
        bad = facts.get("bad_wait_rule")
        add("G5w", "待機の規則に pc/invalid・pc/script-unavailable が無い", "fail" if bad else "pass",
            f"rule={bad}" if bad else "なし")
        roots = facts.get("controller_roots", 0)
        add("G5r", "controller の起票（label project-controller）が 0", "pass" if roots == 0 else "fail",
            f"roots={roots}")
    else:
        ok, why = shadow_report_ok(facts.get("shadow_report"), facts.get("min_shadow_secs", MIN_SHADOW_OBSERVE_SECS))
        add("G5", "shadow の監視の報告が 24 時間以上・違反 0", "pass" if ok else "fail", why)
    # G6 承認（段ごと）。
    ok, why = facts.get("approval_check", (False, "not checked"))
    add("G6", f"{stage} の人の承認（kind={APPROVAL_KINDS[stage]}）", "pass" if ok else "fail", why)
    # G7 実 LLM の縮小 pilot で案件 CoS の usage が数えられた（cos_usage_null が 0）。
    pf = facts.get("pilot_facts")
    if scope == "non-production":
        add("G7", "実 LLM pilot の再実行で cos_usage_null 0", "waived", "非本番の予行では要らない")
    elif pf is None:
        add("G7", "実 LLM pilot の再実行で cos_usage_null 0", "pending",
            "pilot の facts（--pilot-facts）が無い。operator の再実行（--cos-route 付き）待ち")
    elif pf.get("schema") != "celeris.long-horizon-pilot-gate/1":
        # pilot_gate.py judge --pilot-facts の出力だけを受ける（driver の facts や手書きの JSON で通さない）。
        add("G7", "実 LLM pilot の再実行で cos_usage_null 0", "fail",
            "pilot の facts が pilot_gate.py の出力でない（scripts/dev/long-horizon-pilot/pilot_gate.py judge --pilot-facts）")
    elif pf.get("g7") == "pending":
        add("G7", "実 LLM pilot の再実行で cos_usage_null 0", "pending",
            f"実 LLM の facts でない（missing={pf.get('missing')}）。operator の再実行（--cos-route 付き）待ち")
    else:
        # g7 は pilot_gate.py が実 LLM の facts で決める（usage・cache・USD の欠測 0、CoS の run が 1 本以上）。
        ok = pf.get("g7") == "pass" and pf.get("cos_usage_null") == 0 and (pf.get("cos_runs") or 0) >= 1
        add("G7", "実 LLM pilot の再実行で cos_usage_null 0", "pass" if ok else "fail",
            f"g7={pf.get('g7')} cos_runs={pf.get('cos_runs')} cos_usage_null={pf.get('cos_usage_null')} "
            f"missing={pf.get('missing')}")
    return rows


def verdict(rows):
    if any(r["state"] == "fail" for r in rows):
        return "no-go"
    if any(r["state"] == "pending" for r in rows):
        return "no-go (pending)"
    return "go"


def diff(base, cur, stage):
    """2 つの標本の差。段の不変条件に反するものを violations に、観測を changes に出す。"""
    v, ch = [], {}
    if cur.get("error"):
        v.append(f"snapshot_error:{cur['error']}")
        return {"violations": v, "changes": ch}
    if cur.get("project_mode") != STAGE_PROJECT_MODE[stage]:
        v.append(f"project_mode:{cur.get('project_mode')}!={STAGE_PROJECT_MODE[stage]}")
    if (cur.get("domain_mode"), cur.get("domain_generation")) != (base.get("domain_mode"), base.get("domain_generation")):
        v.append(f"domain_changed:{base.get('domain_mode')}->{cur.get('domain_mode')}")
    if cur.get("other_modes") != base.get("other_modes"):
        v.append("other_projects_changed")
    if cur.get("daemon_config_sha256") != base.get("daemon_config_sha256"):
        v.append("daemon_config_changed")
    if cur.get("daemon_instance") != base.get("daemon_instance"):
        ch["daemon_restarted"] = True
    if stage in ("dry_run", "shadow"):
        eff = cur.get("effects", {})
        bad = {k: n for k, n in eff.items() if k in EFFECTFUL and n - base.get("effects", {}).get(k, 0) > 0}
        if bad:
            v.append(f"effects_not_suppressed:{bad}")
        if cur.get("controller_roots", 0) > base.get("controller_roots", 0):
            v.append(f"controller_roots:{base.get('controller_roots', 0)}->{cur.get('controller_roots')}")
        if cur.get("cos_inputs_total", 0) > base.get("cos_inputs_total", 0):
            v.append(f"controller_cos_inputs:{base.get('cos_inputs_total', 0)}->{cur.get('cos_inputs_total')}")
        if cur.get("wait_rule") and cur["wait_rule"].startswith(BAD_WAIT_RULES):
            v.append(f"bad_wait_rule:{cur['wait_rule']}")
    ch["decisions"] = cur.get("decisions_total", 0) - base.get("decisions_total", 0)
    if cur.get("rss_kb") and base.get("rss_kb"):
        ch["rss_kb"] = cur["rss_kb"] - base["rss_kb"]
    if cur.get("rss_anon_kb") and base.get("rss_anon_kb"):
        ch["rss_anon_kb"] = cur["rss_anon_kb"] - base["rss_anon_kb"]
    return {"violations": v, "changes": ch}


def production_path(path):
    p = str(pathlib.Path(path).resolve())
    return any(p == x or p.startswith(x + "/") for x in PRODUCTION_PREFIXES)


# ---------------------------------------------------------------- API と収集

class Api:
    def __init__(self, base, token):
        self.base = base.rstrip("/")
        self.token = token

    def call(self, method, path, body=None):
        req = urllib.request.Request(self.base + path, method=method,
                                     data=None if body is None else json.dumps(body).encode(),
                                     headers={"Authorization": f"Bearer {self.token}",
                                              "Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                data = r.read()
                return r.status, (json.loads(data) if data else None)
        except urllib.error.HTTPError as e:
            data = e.read()
            try:
                return e.code, json.loads(data)
            except ValueError:
                return e.code, {"raw": data.decode(errors="replace")}

    def get(self, path):
        code, body = self.call("GET", path)
        if code != 200:
            raise RuntimeError(f"GET {path} -> {code} {json.dumps(body, ensure_ascii=False)[:300]}")
        return body


def read_token(path):
    return pathlib.Path(path).read_text().strip()


def list_projects(api):
    body = api.get("/api/v1/projects")
    items = body if isinstance(body, list) else body.get("projects") or body.get("items") or []
    return [p.get("id") or (p.get("project") or {}).get("id") for p in items]


def git_facts(repo, release):
    out = {}
    try:
        subs = subprocess.run(["git", "-C", repo, "log", "refs/heads/main", "--format=%H %s"], check=True,
                              capture_output=True, text=True).stdout.splitlines()
    except (subprocess.CalledProcessError, FileNotFoundError):
        return out
    found = {}
    for line in subs:
        h, _, s = line.partition(" ")
        for want in REQUIRED_SUBJECTS:
            if s.startswith(want) and want not in found:
                found[want] = h
    out["missing_subjects"] = [w for w in REQUIRED_SUBJECTS if w not in found]
    m = re.search(r"[0-9a-f]{12,40}", release or "")
    if not m or out["missing_subjects"]:
        out["release_contains_required"] = None
        out["release_detail"] = f"release={release!r}（sha を含まない）" if not m else "main に足りない修正がある"
        return out
    sha = m.group(0)
    ok = all(subprocess.run(["git", "-C", repo, "merge-base", "--is-ancestor", h, sha],
                            capture_output=True).returncode == 0 for h in found.values())
    out["release_contains_required"] = ok
    out["release_detail"] = f"release {sha[:12]} {'は含む' if ok else 'に足りない修正がある'}"
    return out


def config_facts(path):
    try:
        raw = pathlib.Path(path).read_bytes()
        doc = tomllib.loads(raw.decode())
    except (OSError, tomllib.TOMLDecodeError, UnicodeDecodeError) as e:
        return {"read": False, "error": f"{type(e).__name__}: {e}"}
    pc = doc.get("project_controller") or {}
    return {"read": True, "sha256": hashlib.sha256(raw).hexdigest(), "enabled": pc.get("enabled"),
            "allowed_project_ids": pc.get("allowed_project_ids"), "budget_present": True}


def proc_status(pid):
    out = {}
    try:
        for line in pathlib.Path(f"/proc/{pid}/status").read_text().splitlines():
            k, _, v = line.partition(":")
            if k in ("VmRSS", "RssAnon", "Threads"):
                out[k] = int(v.split()[0])
    except (OSError, ValueError):
        pass
    return out


def snapshot(api, project, pid=None):
    """監視の 1 標本。GET だけ。"""
    s = {"schema": SCHEMA, "kind": "snapshot", "at": now_utc().isoformat(timespec="seconds"), "project": project}
    try:
        t0 = time.monotonic()
        view = api.get(f"/api/v1/projects/{project}/controller")
        s["get_controller_ms"] = round((time.monotonic() - t0) * 1000, 1)
        st = view.get("state") or {}
        s["project_mode"] = st.get("mode")
        s["phase"] = st.get("phase")
        s["revision"] = st.get("revision")
        s["mode_changed_at"] = st.get("updated_at")
        for k in ("lease_expires_at", "backoff_until", "coalesce_until", "next_wake_at"):
            s[k] = st.get(k)
        bt = view.get("budget_today") or {}
        s["budget_today"] = bt
        s["cos_inputs_total"] = bt.get("cos_inputs", 0)
        summ = view.get("summary") or {}
        s["wait_rule"] = (summ.get("waiting") or {}).get("rule_id")
        s["budget_limits"] = summ.get("budget_limits")
        s["pending_wakes"] = len(view.get("pending_wakes") or [])
        decs = api.get(f"/api/v1/projects/{project}/controller/decisions?limit=200")
        items = decs if isinstance(decs, list) else decs.get("items") or decs.get("decisions") or []
        s["decisions_total"] = len(items)
        eff, after = {}, {}
        for d in items:
            eff[d.get("effect_state")] = eff.get(d.get("effect_state"), 0) + 1
            if d.get("mode") == s["project_mode"]:
                after[d.get("effect_state")] = after.get(d.get("effect_state"), 0) + 1
        s["effects"] = eff
        s["decisions_after_mode"] = after
        s["decision_modes"] = sorted({d.get("mode") for d in items if d.get("mode")})
        s["decision_rules"] = [[d.get("mode"), d.get("effect_state"), find_key(_loads(d.get("proposal_json")), "rule_id")]
                               for d in items[:20]]
        t0 = time.monotonic()
        roots = api.get(f"/api/v1/tasks?project={project}&label=project-controller&limit=200")
        s["get_tasks_ms"] = round((time.monotonic() - t0) * 1000, 1)
        ritems = roots if isinstance(roots, list) else roots.get("tasks") or roots.get("items") or []
        s["controller_roots"] = len(ritems)
        dom = api.get("/api/v1/script/domains")
        row = domain_row(dom) or {}
        s["domain_mode"] = row.get("mode")
        s["domain_generation"] = row.get("bundle_sha256")
        s["domain_version"] = row.get("version")
        s["guard_halted"] = dom.get("guard_halted")
        others = {}
        for p in list_projects(api):
            if p and p != project:
                others[p] = (api.get(f"/api/v1/projects/{p}/controller").get("state") or {}).get("mode")
        s["other_modes"] = others
        ident = (api.get("/api/v1/config") or {}).get("identity") or {}
        s["daemon_config_sha256"] = ident.get("config_sha256")
        s["daemon_release"] = ident.get("release")
        s["daemon_instance"] = ident.get("instance_id")
        dpid = pid or ident.get("pid")
        ps = proc_status(dpid) if dpid else {}
        s["daemon_pid"] = dpid
        s["rss_kb"] = ps.get("VmRSS")
        s["rss_anon_kb"] = ps.get("RssAnon")
        s["threads"] = ps.get("Threads")
    except (RuntimeError, OSError, urllib.error.URLError) as e:
        s["error"] = str(e)[:300]
    return s


def collect(a, api, stage, scope):
    facts = {"project": a.project, "min_shadow_secs": a.min_shadow_secs}
    snap = snapshot(api, a.project)
    if snap.get("error"):
        raise SystemExit(f"pc-rollout: the API did not answer: {snap['error']}")
    facts["snapshot"] = snap
    facts["controller"] = {"mode": snap.get("project_mode"), "phase": snap.get("phase")}
    facts["other_modes"] = snap.get("other_modes")
    facts["domain"] = {"mode": snap.get("domain_mode"), "generation_state": None}
    dom = domain_row(api.get("/api/v1/script/domains")) or {}
    facts["domain"]["generation_state"] = dom.get("generation_state")
    facts["guard_halted"] = snap.get("guard_halted")
    facts["daemon_config_sha256"] = snap.get("daemon_config_sha256")
    facts["budget_limits"] = snap.get("budget_limits")
    dm = snap.get("decision_modes") or []
    eff = snap.get("effects") or {}
    facts["decisions"] = {"total": snap.get("decisions_total", 0),
                          "dry_run": snap.get("decisions_total", 0) if "dry_run" in dm else 0,
                          "not_suppressed": sum(n for k, n in eff.items() if k in EFFECTFUL)}
    facts["bad_wait_rule"] = snap["wait_rule"] if (snap.get("wait_rule") or "").startswith(BAD_WAIT_RULES) else None
    facts["controller_roots"] = snap.get("controller_roots", 0)
    facts["git"] = git_facts(a.repo, snap.get("daemon_release"))
    if a.config:
        facts["config"] = config_facts(a.config)
        if facts["config"].get("read"):
            facts["config"]["budget_present"] = bool(snap.get("budget_limits"))
    approval = json.loads(pathlib.Path(a.approval).read_text()) if a.approval else None
    report = json.loads(pathlib.Path(a.shadow_report).read_text()) if a.shadow_report else None
    facts["shadow_report"] = report
    rsha = sha256_file(a.shadow_report) if a.shadow_report else None
    facts["approval_check"] = check_approval(approval, stage, a.project, api.base, os.environ, shadow_report_sha=rsha)
    facts["pilot_facts"] = json.loads(pathlib.Path(a.pilot_facts).read_text()) if a.pilot_facts else None
    return facts, approval


def cmd_preflight(a):
    api = Api(a.api, read_token(a.token_file))
    approval = json.loads(pathlib.Path(a.approval).read_text()) if a.approval else None
    scope = (approval or {}).get("scope", "production")
    facts, _ = collect(a, api, a.stage, scope)
    rows = criteria(a.stage, facts, scope)
    doc = {"schema": SCHEMA, "kind": "preflight", "stage": a.stage, "scope": scope, "project": a.project,
           "api": api.base, "at": now_utc().isoformat(timespec="seconds"), "verdict": verdict(rows),
           "criteria": rows, "snapshot": facts["snapshot"]}
    write_out(a.out, doc)
    for r in rows:
        print(f"{r['state']:8} {r['id']:4} {r['what']} — {r['detail']}")
    print(f"verdict: {doc['verdict']}")
    return 0 if doc["verdict"] == "go" else 1


def write_out(path, doc):
    text = json.dumps(doc, ensure_ascii=False, indent=2) + "\n"
    if path:
        pathlib.Path(path).parent.mkdir(parents=True, exist_ok=True)
        pathlib.Path(path).write_text(text)
    return text


def cmd_snapshot(a):
    api = Api(a.api, read_token(a.token_file))
    s = snapshot(api, a.project, a.pid)
    print(write_out(a.out, s), end="")
    return 1 if s.get("error") else 0


def cmd_diff(a):
    base = json.loads(pathlib.Path(a.base).read_text())
    cur = json.loads(pathlib.Path(a.cur).read_text())
    d = diff(base, cur, a.stage)
    print(json.dumps(d, ensure_ascii=False, indent=2))
    return 1 if d["violations"] else 0


def monitor(api, project, stage, secs, interval, out, pid=None, scope="production", on_violation=None,
            between=None):
    """監視。標本を out（jsonl）に追記し、報告（dict）を返す。between(i) は標本の間に呼ぶ（e2e の操作の差し込み）。"""
    base = snapshot(api, project, pid)
    rows = [base]
    viol = []
    t0 = time.monotonic()
    i = 0
    with open(out, "a") as f:
        f.write(json.dumps({"base": base}, ensure_ascii=False) + "\n")
        while time.monotonic() - t0 < secs:
            time.sleep(interval)
            if between:
                between(i)
            i += 1
            cur = snapshot(api, project, pid)
            d = diff(base, cur, stage)
            f.write(json.dumps({"sample": cur, "diff": d}, ensure_ascii=False) + "\n")
            f.flush()
            rows.append(cur)
            if d["violations"]:
                viol = d["violations"]
                if on_violation:
                    on_violation(viol)
                break
    lat = sorted(r.get("get_controller_ms") or 0 for r in rows)
    rss = [r.get("rss_anon_kb") for r in rows if r.get("rss_anon_kb")]
    return {"schema": SCHEMA, "kind": "monitor", "stage": stage, "scope": scope, "project": project,
            "allow_non_production": scope == "non-production",
            "observed_secs": round(time.monotonic() - t0), "samples": len(rows), "violations": viol,
            "decisions_delta": rows[-1].get("decisions_total", 0) - base.get("decisions_total", 0),
            "get_controller_ms_p50": lat[len(lat) // 2] if lat else None, "get_controller_ms_max": lat[-1] if lat else None,
            "rss_anon_kb_first": rss[0] if rss else None, "rss_anon_kb_last": rss[-1] if rss else None,
            "daemon_restarts": sum(1 for a, b in zip(rows, rows[1:]) if a.get("daemon_instance") != b.get("daemon_instance"))}


def cmd_monitor(a):
    api = Api(a.api, read_token(a.token_file))
    approval = json.loads(pathlib.Path(a.approval).read_text()) if a.approval else None
    scope = (approval or {}).get("scope", "production")
    undo_fn = (lambda v: undo(api, a.project, f"monitor violation: {v}", None)) if a.undo_on_violation else None
    rep = monitor(api, a.project, a.stage, a.secs, a.interval, a.samples, a.pid, scope, undo_fn)
    print(write_out(a.out, rep), end="")
    return 1 if rep["violations"] else 0


def apply_stage(api, project, stage, approval, reason, shadow_report_sha=None, env=None):
    """1 段だけ上げる。承認を確かめ、案件 mode（と canary・enabled では領域 mode）を変える。戻り値は記録の dict。"""
    ok, why = check_approval(approval, stage, project, api.base, env if env is not None else os.environ,
                             shadow_report_sha=shadow_report_sha)
    rec = {"schema": SCHEMA, "kind": "apply", "stage": stage, "project": project, "approval": why, "steps": []}
    if not ok:
        rec["refused"] = why
        return rec
    rec["before"] = snapshot(api, project)
    dom = domain_row(api.get("/api/v1/script/domains")) or {}
    want_dom = STAGE_DOMAIN_MODES[stage][0]
    if dom.get("mode") != want_dom:
        if dom.get("bundle_sha256") not in (None, approval["generation"]):
            rec["refused"] = f"domain generation {dom.get('bundle_sha256')} is not the approved {approval['generation']}"
            return rec
        body = {"mode": want_dom, "expected_version": dom.get("version"), "reason": reason,
                "generation": approval["generation"]}
        if want_dom == "canary":
            body["canary_bps"] = approval["canary_bps"]
        code, resp = api.call("PUT", "/api/v1/script/domains/project_controller/mode", body)
        rec["steps"].append({"domain_mode": want_dom, "status": code, "body": resp})
        if code != 200:
            rec["refused"] = f"domain mode {want_dom} -> {code}"
            return rec
    st = api.get(f"/api/v1/projects/{project}/controller")["state"]
    want = STAGE_PROJECT_MODE[stage]
    if st.get("mode") != want:
        code, resp = api.call("PUT", f"/api/v1/projects/{project}/controller",
                              {"mode": want, "expected_revision": st["revision"], "reason": reason})
        rec["steps"].append({"project_mode": want, "status": code, "body": resp})
        if code != 200:
            rec["refused"] = f"project mode {want} -> {code}"
            return rec
    rec["after"] = snapshot(api, project)
    return rec


def undo(api, project, reason, domain_to):
    rec = {"schema": SCHEMA, "kind": "undo", "project": project, "reason": reason, "steps": []}
    rec["before"] = snapshot(api, project)
    st = api.get(f"/api/v1/projects/{project}/controller")["state"]
    if st.get("mode") != "off":
        code, resp = api.call("PUT", f"/api/v1/projects/{project}/controller",
                              {"mode": "off", "expected_revision": st["revision"], "reason": reason})
        rec["steps"].append({"project_mode": "off", "status": code, "body": resp})
    if domain_to:
        dom = domain_row(api.get("/api/v1/script/domains")) or {}
        if DOMAIN_RANK.get(dom.get("mode"), 0) > DOMAIN_RANK[domain_to]:
            body = {"mode": domain_to, "expected_version": dom.get("version"), "reason": reason}
            if domain_to != "rust":
                body["generation"] = dom.get("bundle_sha256")
            code, resp = api.call("PUT", "/api/v1/script/domains/project_controller/mode", body)
            rec["steps"].append({"domain_mode": domain_to, "status": code, "body": resp})
    rec["after"] = snapshot(api, project)
    rec["ok"] = rec["after"].get("project_mode") == "off" and (
        domain_to is None or DOMAIN_RANK.get(rec["after"].get("domain_mode"), 9) <= DOMAIN_RANK[domain_to])
    return rec


def cmd_apply(a):
    api = Api(a.api, read_token(a.token_file))
    approval = json.loads(pathlib.Path(a.approval).read_text())
    rsha = sha256_file(a.shadow_report) if a.shadow_report else None
    rec = apply_stage(api, a.project, a.stage, approval, a.reason, rsha)
    print(write_out(a.out, rec), end="")
    return 1 if rec.get("refused") else 0


def cmd_undo(a):
    api = Api(a.api, read_token(a.token_file))
    rec = undo(api, a.project, a.reason, a.domain_to)
    print(write_out(a.out, rec), end="")
    return 0 if rec["ok"] else 1


# ---------------------------------------------------------------- e2e（非本番の一時 daemon）

class TempDaemon:
    """一時 home の celeris（fake harness、127.0.0.1）。daemon_soak.py の Soak と同じ config の形。"""

    def __init__(self, home, celeris):
        self.home = pathlib.Path(home).resolve()
        self.home.mkdir(parents=True, exist_ok=True)
        self.celeris = celeris
        self.token = secrets.token_urlsafe(32)
        self.secret_dir = pathlib.Path(tempfile.mkdtemp(prefix="pc-rollout-token-"))
        self.token_file = self.secret_dir / "human.token"
        self.op_token = secrets.token_urlsafe(32)
        self.op_token_file = self.secret_dir / "operator.token"
        for f, t in ((self.token_file, self.token), (self.op_token_file, self.op_token)):
            fd = os.open(f, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
            with os.fdopen(fd, "w") as w:
                w.write(t + "\n")
        with socket.socket() as s:
            s.bind(("127.0.0.1", 0))
            self.port = s.getsockname()[1]
        self.base = f"http://127.0.0.1:{self.port}"
        self.proc = None
        self.allowed = []

    @property
    def config(self):
        return self.home / "config.toml"

    def write_config(self):
        harness = [sys.executable, "-I", str(pathlib.Path(__file__).parent.parent / "daemon-soak" / "soak_harness.py"),
                   "--sleep", "0.2"]
        for d in ("ws", "knowledge", "build-cache", "scratch", "releases"):
            (self.home / d).mkdir(exist_ok=True)
        self.config.write_text(f"""
db = "celeris.sqlite3"
workspace_root = "ws"
tick_ms = 200
max_concurrency = 2

[api]
listen = "127.0.0.1:{self.port}"

[[api.tokens]]
principal = "human:p4g-e2e"
token_file = {json.dumps(str(self.token_file))}

[[api.tokens]]
principal = "operator:claude-code"
token_file = {json.dumps(str(self.op_token_file))}

[cos]
enabled = true
max_turns = 4
max_wall_secs = 120
route_candidates = [{{ source = "local", model = "model-1" }}]

[knowledge]
root = {json.dumps(str(self.home / "knowledge"))}

[workspace]
build_cache_dir = {json.dumps(str(self.home / "build-cache"))}

[scratch]
dir = {json.dumps(str(self.home / "scratch"))}

[selfdeploy]
releases_dir = {json.dumps(str(self.home / "releases"))}

[maintenance]
disk_watch = []

[script]
state_dir = "state"

[project_controller]
enabled = true
allowed_project_ids = {json.dumps(self.allowed)}

[adapters.fake]
command = {json.dumps(harness)}

[llm_sources.local]
kind = "fake"
accounts = ["a1"]
models = ["model-1"]

[harnesses.claude_code]
kind = "fake"
api_formats = ["fake"]
source_kinds = ["fake"]

[route_defaults.claude_code]
candidates = [{{ source = "local", model = "model-1" }}]

[harness_selection]
execution_class_defaults = {{}}
rules = [{{ harness = "claude_code", required_capabilities = [] }}]
""")

    def start(self):
        env = {k: v for k, v in os.environ.items() if not k.startswith("CELERIS_")
               and k not in ("CLAUDECODE", "CLAUDE_CODE_SESSION_ID")}
        env.setdefault("RUST_LOG", "info")
        env["NO_COLOR"] = "1"
        logf = open(self.home / "daemon.log", "ab")
        self.proc = subprocess.Popen([self.celeris, "--config", str(self.config), "--log-format", "text",
                                      "--release", "p4g-e2e"], stdout=logf, stderr=logf, stdin=subprocess.DEVNULL,
                                     env=env, cwd=self.home)
        api = Api(self.base, self.token)
        for _ in range(600):
            if self.proc.poll() is not None:
                sys.stderr.write((self.home / "daemon.log").read_text(errors="replace")[-3000:])
                raise SystemExit("pc-rollout e2e: the daemon exited at startup")
            try:
                api.get("/api/v1/daemon")
                return
            except (RuntimeError, urllib.error.URLError, ConnectionError, OSError):
                time.sleep(0.2)
        raise SystemExit("pc-rollout e2e: the daemon API did not answer in 120 s")

    def stop(self, sig=signal.SIGTERM):
        if self.proc is not None and self.proc.poll() is None:
            self.proc.send_signal(sig)
            try:
                self.proc.wait(timeout=60)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        self.proc = None


def approval_doc(kind, stages, project, api, scope="non-production", **extra):
    exp = (now_utc() + datetime.timedelta(hours=2)).isoformat(timespec="seconds")
    return {"approved_by": "human:p4g-e2e-fixture", "kind": kind, "stages": stages, "scope": scope,
            "project_id": project, "api": api, "expires_at": exp, **extra}


def cmd_e2e(a):
    out = pathlib.Path(a.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    if production_path(a.state):
        raise SystemExit(2)
    d = TempDaemon(a.state, a.celeris)
    log = open(out / "ops.jsonl", "w")
    t_start = time.monotonic()

    def op(step, **detail):
        row = {"schema": SCHEMA, "step": step, "t": round(time.monotonic() - t_start, 2), **detail}
        log.write(json.dumps(row, ensure_ascii=False) + "\n")
        log.flush()
        print(f"pc-rollout e2e: {step} {json.dumps(detail, ensure_ascii=False)[:240]}", flush=True)
        return row

    fails = []

    def expect(cond, what):
        op("expect", what=what, ok=bool(cond))
        if not cond:
            fails.append(what)

    d.write_config()
    d.start()
    human = Api(d.base, d.token)
    operator = Api(d.base, d.op_token)
    ws = {"kind": "local", "path": str(d.home / "ws")}
    code, p = human.call("POST", "/api/v1/projects", {
        "title": "p4g:target", "workspace": ws,
        "request": "P4-G の限定導入の予行の案件（非本番・fake harness）。\n\n```celeris-outcomes\n"
                   "[{\"id\":\"o1\",\"title\":\"予行の途中目標\",\"acceptance\":[\"root が done\"]}]\n```\n"})
    pid = p["id"] if "id" in p else p["project"]["id"]
    code, q = human.call("POST", "/api/v1/projects", {"title": "p4g:other", "workspace": ws,
                                                      "request": "対象外の案件（off のまま）。"})
    other = q["id"] if "id" in q else q["project"]["id"]
    # 本番の対象は進行中（active）の案件。作った直後の proposed のままだと判定は pc/project-not-active で止まるので、
    # 人の API（PATCH /projects）で active にする。
    code, _ = human.call("PATCH", f"/api/v1/projects/{pid}", {"status": "active"})
    op("projects", target=pid, other=other, activate=code)
    expect(code == 200, "target project is active")
    # 対象を 1 案件だけ allowed にして再起動する（本番の段 0 と同じ操作。人が config を書き再起動する）。
    d.stop()
    d.allowed = [pid]
    d.write_config()
    d.start()
    # 登録は再起動の後（再起動は領域が指していない loaded の generation を retire する: startup_no_isolate）。
    sha = subprocess.run([a.script_deploy, "--dist", a.bundle, "--state-dir", str(d.home / "state")], check=True,
                         capture_output=True, text=True).stdout.strip()
    code, g = human.call("POST", "/api/v1/script/generations", {"bundle_sha256": sha})
    op("generation", status=code, sha=sha)
    expect(code in (200, 201), "generation registered")
    pf = argparse.Namespace(project=pid, repo=a.repo, config=str(d.config), approval=None, shadow_report=None,
                            pilot_facts=None, min_shadow_secs=a.min_shadow_secs)

    def preflight(stage, approval=None, report_path=None, name=None):
        ap = None
        if approval is not None:
            ap = out / f"approval-{name or stage}.json"
            ap.write_text(json.dumps(approval, ensure_ascii=False, indent=2) + "\n")
        pf.approval, pf.shadow_report = (str(ap) if ap else None), report_path
        scope = (approval or {}).get("scope", "production")
        facts, _ = collect(pf, human, stage, scope)
        rows = criteria(stage, facts, scope)
        doc = {"schema": SCHEMA, "kind": "preflight", "stage": stage, "scope": scope, "project": pid,
               "verdict": verdict(rows), "criteria": rows, "snapshot": facts["snapshot"]}
        write_out(out / f"preflight-{name or stage}.json", doc)
        op("preflight", stage=stage, name=name or stage, verdict=doc["verdict"],
           failed=[r["id"] for r in rows if r["state"] in ("fail", "pending")])
        return doc

    # 0. 承認なし・本番 scope の preflight は no-go（G6 fail、G7 pending）。
    pre0 = preflight("dry_run", None, name="dry_run-no-approval")
    expect(pre0["verdict"].startswith("no-go"), "preflight without approval is no-go")
    # 1. 領域 compare（観察の承認で）→ dry_run。
    obs = approval_doc("observe", ["dry_run", "shadow"], pid, d.base, generation=sha)
    pre1 = preflight("dry_run", obs)
    expect(pre1["verdict"] == "go", "dry_run preflight with observe approval is go")
    rec = apply_stage(human, pid, "dry_run", obs, "P4-G e2e: dry_run", env={})
    write_out(out / "apply-dry_run.json", rec)
    expect(not rec.get("refused"), f"apply dry_run ({rec.get('refused')})")
    for i in range(30):
        s = snapshot(human, pid)
        if s.get("decisions_total", 0) >= 1:
            break
        time.sleep(0.5)
    code, _ = human.call("POST", f"/api/v1/projects/{pid}/controller/wake",
                         {"external_ref": "dry-run", "idempotency_key": "p4g-e2e-dry-1", "note": "P4-G e2e"})
    time.sleep(2)
    s = snapshot(human, pid)
    write_out(out / "snapshot-dry_run.json", s)
    expect(s.get("decisions_total", 0) >= 1 and not set(s.get("effects", {})) & set(EFFECTFUL),
           f"dry_run decisions all suppressed ({s.get('effects')})")
    expect(s.get("controller_roots") == 0 and s.get("cos_inputs_total") == 0, "dry_run: no roots and no CoS inputs")
    # 2. operator は上げられない（403）。shadow の preflight → apply。
    st = human.get(f"/api/v1/projects/{pid}/controller")["state"]
    code, body = operator.call("PUT", f"/api/v1/projects/{pid}/controller",
                               {"mode": "shadow", "expected_revision": st["revision"], "reason": "operator tries"})
    op("operator_raise", status=code, code=(body or {}).get("code") or (body or {}).get("type"))
    expect(code == 403, "operator cannot raise to shadow (403)")
    pre2 = preflight("shadow", obs)
    expect(pre2["verdict"] == "go", "shadow preflight is go")
    rec = apply_stage(human, pid, "shadow", obs, "P4-G e2e: shadow", env={})
    write_out(out / "apply-shadow.json", rec)
    expect(not rec.get("refused"), f"apply shadow ({rec.get('refused')})")
    # 3. shadow を監視する。途中で wake を 2 回入れ、daemon を SIGKILL して起こし直す（回復）。
    restart, wake = {}, {}

    def between(i):
        if i in (1, 10):
            wake[i] = human.call("POST", f"/api/v1/projects/{pid}/controller/wake",
                       {"external_ref": "p4g-e2e", "idempotency_key": f"p4g-e2e-shadow-{i}", "note": "P4-G e2e"})
        if i == 3:  # wake の coalesce（約 30 秒）の途中で落とす: 未消費の wake が再起動をまたいで残るか
            t0 = time.monotonic()
            d.stop(signal.SIGKILL)
            d.start()
            restart["secs"] = round(time.monotonic() - t0, 2)
            op("restart", how="SIGKILL", secs=restart["secs"])

    rep = monitor(human, pid, "shadow", a.monitor_secs, a.interval, out / "monitor-shadow.jsonl",
                  scope="non-production", between=between)
    rep["restart_secs"] = restart.get("secs")
    op("shadow_wakes", results={k: v[0] for k, v in wake.items()})
    write_out(out / "monitor-shadow.json", rep)
    expect(not rep["violations"], f"shadow monitor has no violations ({rep['violations']})")
    expect(rep["daemon_restarts"] >= 1, "monitor saw the restart")
    expect(rep["decisions_delta"] >= 1, f"shadow made decisions after the restart ({rep['decisions_delta']})")
    last = snapshot(human, pid)
    op("shadow_decisions", rules=last.get("decision_rules"), effects=last.get("effects"))
    s = snapshot(human, pid)
    expect(s.get("project_mode") == "shadow", "mode stays shadow across SIGKILL")
    # 4. 差分の検出: 対象外の案件を人が dry_run にすると diff が違反を出す（監視の検出の試験）。
    base = snapshot(human, pid)
    st = human.get(f"/api/v1/projects/{other}/controller")["state"]
    code, _ = human.call("PUT", f"/api/v1/projects/{other}/controller",
                         {"mode": "dry_run", "expected_revision": st["revision"], "reason": "P4-G e2e drift"})
    op("drift_other_project", status=code)
    dd = diff(base, snapshot(human, pid), "shadow")
    write_out(out / "diff-drift.json", dd)
    expect(code == 403 or "other_projects_changed" in dd["violations"],
           f"drift is refused by allowed_project_ids or detected ({code}, {dd['violations']})")
    # 5. shadow の承認では canary に上げられない。短い報告（24 時間に満たない）では canary の preflight は no-go。
    rec = apply_stage(human, pid, "canary", obs, "P4-G e2e: canary with observe approval", env={})
    write_out(out / "apply-canary-observe-approval.json", rec)
    expect(rec.get("refused"), f"canary with the observe approval is refused ({rec.get('refused')})")
    rsha = sha256_file(out / "monitor-shadow.json")
    eff = approval_doc("effects", ["canary"], pid, d.base, generation=sha, shadow_report_sha256=rsha, canary_bps=100)
    pre3 = preflight("canary", eff, str(out / "monitor-shadow.json"))
    expect(pre3["verdict"].startswith("no-go") and any(r["id"] == "G5" and r["state"] == "fail"
                                                       for r in pre3["criteria"]),
           "canary preflight is no-go on a short shadow report")
    # 6. undo: operator が off に戻せる（誰でも可）。領域は compare のまま。
    rec = undo(operator, pid, "P4-G e2e: undo by operator", "compare")
    write_out(out / "undo.json", rec)
    expect(rec["ok"], "undo by operator turns the project off")
    time.sleep(2)
    s1 = snapshot(human, pid)
    time.sleep(2)
    s2 = snapshot(human, pid)
    expect(s2.get("pending_wakes", 0) <= s1.get("pending_wakes", 0) and s2.get("decisions_total") ==
           s1.get("decisions_total"), "after undo no new decisions")
    rb = undo(human, pid, "P4-G e2e: domain back to rust", "rust")
    write_out(out / "undo-rust.json", rb)
    expect(rb["ok"], "domain back to rust")
    d.stop()
    summary = {"schema": SCHEMA, "kind": "e2e", "ok": not fails, "failed": fails, "project": pid,
               "monitor": {k: rep.get(k) for k in ("observed_secs", "samples", "violations", "decisions_delta",
                                                   "get_controller_ms_p50", "get_controller_ms_max",
                                                   "rss_anon_kb_first", "rss_anon_kb_last", "restart_secs")},
               "note": "非本番の一時 daemon（fake harness）での操作の予行。本番の go/no-go の根拠にはしない。"}
    write_out(out / "summary.json", summary)
    print(json.dumps(summary, ensure_ascii=False))
    return 0 if not fails else 1


# ---------------------------------------------------------------- CLI

def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)

    def common(p):
        p.add_argument("--api", required=True)
        p.add_argument("--token-file", required=True)
        p.add_argument("--project", required=True)
        p.add_argument("--out")

    p = sub.add_parser("preflight")
    common(p)
    p.add_argument("--stage", required=True, choices=STAGES)
    p.add_argument("--config", help="daemon の config.toml（読むだけ）")
    p.add_argument("--repo", default=".")
    p.add_argument("--approval")
    p.add_argument("--shadow-report")
    p.add_argument("--pilot-facts")
    p.add_argument("--min-shadow-secs", type=int, default=MIN_SHADOW_OBSERVE_SECS)
    p.set_defaults(fn=cmd_preflight)
    p = sub.add_parser("snapshot")
    common(p)
    p.add_argument("--pid", type=int)
    p.set_defaults(fn=cmd_snapshot)
    p = sub.add_parser("diff")
    p.add_argument("--base", required=True)
    p.add_argument("--cur", required=True)
    p.add_argument("--stage", required=True, choices=STAGES)
    p.set_defaults(fn=cmd_diff)
    p = sub.add_parser("monitor")
    common(p)
    p.add_argument("--stage", required=True, choices=STAGES)
    p.add_argument("--secs", type=int, required=True)
    p.add_argument("--interval", type=float, default=60)
    p.add_argument("--samples", required=True)
    p.add_argument("--pid", type=int)
    p.add_argument("--approval")
    p.add_argument("--undo-on-violation", action="store_true")
    p.set_defaults(fn=cmd_monitor)
    p = sub.add_parser("apply")
    common(p)
    p.add_argument("--stage", required=True, choices=STAGES)
    p.add_argument("--approval", required=True)
    p.add_argument("--shadow-report")
    p.add_argument("--reason", required=True)
    p.set_defaults(fn=cmd_apply)
    p = sub.add_parser("undo")
    common(p)
    p.add_argument("--reason", required=True)
    p.add_argument("--domain-to", choices=["compare", "rust"])
    p.set_defaults(fn=cmd_undo)
    p = sub.add_parser("e2e")
    p.add_argument("--out", required=True)
    p.add_argument("--state", required=True)
    p.add_argument("--celeris", required=True)
    p.add_argument("--script-deploy", required=True)
    p.add_argument("--bundle", required=True)
    p.add_argument("--repo", default=".")
    p.add_argument("--monitor-secs", type=int, default=90)
    p.add_argument("--interval", type=float, default=5)
    p.add_argument("--min-shadow-secs", type=int, default=MIN_SHADOW_OBSERVE_SECS)
    p.set_defaults(fn=cmd_e2e)
    a = ap.parse_args(argv)
    return a.fn(a)


if __name__ == "__main__":
    sys.exit(main())
