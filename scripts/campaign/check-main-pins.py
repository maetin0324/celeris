#!/usr/bin/env python3
"""主比較（A0/B0, mlsys-main-1）の固定 root・設定・lock・実証跡を内容検証する checker。

    python3 scripts/campaign/check-main-pins.py --root <artifacts dir>

読むだけ。LLM・daemon・ネットワークは使わず、config / release / DB も書かない。
不足・改変・公開側への private grader 混入が 1 つでもあれば非 0 で、問題を 1 行ずつ stderr に出す。
秘密値は読んでも出さない（env の値は path のみ検証し、鍵らしい欄は値の有無だけ見る）。
"""
import argparse
import hashlib
import json
import os
import re
import sys
import tomllib

MODEL = "claude-sonnet-5-5"
HARNESS = "claude-code"
CAMPAIGN_ID = "mlsys-main-1"
RELEASE = "c9654bff5633"
A_VERSION = "2.1.303"
INPUT_MANIFEST_SHA = "6fa8f0ecf197d731a36ea7c7152b088be99a18c0cf229cdf01340aec28b06e4b"
MATERIALS_SHA = "dab3ca05d01adf1da268c8ec35e24f33249ca3cc02083e0a2e6b487d0c5a800d"
KB_SHA = "741f24146f5c02f44acf5291e23806416b9956809e55a1474488ecbcc9fe0702"
CAMPAIGN_SHA = "8ca0e1984bdaf1140f4a849c53c1f01522099a1ec555816bb2472f5c039ea999"
CAMPAIGN_BIN_SHA = "de339f1c4af25bcb2b3ec8d53222ca05c010a11fb7343f16f69d514559161394"
CAMPAIGN_BIN_REL = os.path.join("pinned", "evidence", "main-k2", "bin", "celeris-campaign-fixed")
K2_COMMIT = "086c4618e2a4d94ea56da78ceeed0f8d6d568d67"
PROBLEMS = ["k2-bf-xattr-restart-campaign", "k4-cmp4-validity-analysis"]
PREFLIGHT_STAGES = {"manifest", "case-inputs", "graders", "capability", "env-check", "lock"}
SECRET_KEY = re.compile(r"(KEY|TOKEN|SECRET|PASSWORD|CREDENTIAL)", re.I)
# 寿命の短い場所（旧 WU worktree 等）への参照
EPHEMERAL = re.compile(r"/wu/|/pin-values/")
LEAK_NAMES = ("private.json", "grader-private", ".grader", "grade-cases")
MIN_HASH_SIZE = 32


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def load_json(path, problems, label):
    try:
        with open(path, encoding="utf-8") as f:
            return json.load(f)
    except FileNotFoundError:
        problems.append(f"{label}: missing: {path}")
    except (ValueError, OSError) as e:
        problems.append(f"{label}: unreadable ({e.__class__.__name__}): {path}")
    return None


def parse_env(path, problems):
    env = {}
    try:
        with open(path, encoding="utf-8") as f:
            lines = f.read().splitlines()
    except OSError:
        problems.append(f"launcher.env: missing: {path}")
        return None
    for line in lines:
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        k, sep, v = line.partition("=")
        if sep:
            env[k.strip()] = v.strip().strip('"').strip("'")
    return env


def files_under(root):
    for d, _dirs, names in os.walk(root):
        for n in names:
            yield os.path.join(d, n)


def check_env(layout, env, problems):
    for k in ("CASES_ROOT", "GRADER_ROOT", "GRADE_CASES_ROOT"):
        want = layout["fixed"][k]
        if env.get(k) != want:
            problems.append(f"launcher.env: {k} must be {want} (got {env.get(k)!r})")
    if env.get("MEASUREMENT_ID") != CAMPAIGN_ID:
        problems.append(f"launcher.env: MEASUREMENT_ID must be {CAMPAIGN_ID}")
    if env.get("RELEASE") != RELEASE:
        problems.append(f"launcher.env: RELEASE must be {RELEASE}")
    if not env.get("CELERIS_BIN", "").endswith(f"/{RELEASE}/bin/celeris"):
        problems.append(f"launcher.env: CELERIS_BIN must be the approved release {RELEASE}/bin/celeris")
    want_cmp = os.path.join(layout["root"], CAMPAIGN_BIN_REL)
    if env.get("CAMPAIGN_BIN") != want_cmp:
        problems.append(f"launcher.env: CAMPAIGN_BIN must be {want_cmp}")
    elif os.path.isfile(want_cmp) and sha256_file(want_cmp) != CAMPAIGN_BIN_SHA:
        problems.append(f"launcher.env: CAMPAIGN_BIN sha256 differs from the approved fixed binary {CAMPAIGN_BIN_SHA}")
    want_cap = os.path.join(layout["pinned"], "cap-env.json")
    if env.get("CAP_ENV") != want_cap:
        problems.append(f"launcher.env: CAP_ENV must be {want_cap}")
    if not env.get("A_ACCOUNT"):
        problems.append("launcher.env: A_ACCOUNT is required")
    for k, v in env.items():
        if EPHEMERAL.search(v):
            problems.append(f"launcher.env: {k} points into an ephemeral work tree")
        if SECRET_KEY.search(k) and k != "CAP_ENV" and v:
            problems.append(f"launcher.env: {k} looks like a secret value (use a reference name only)")
    for k in ("CELERIS_BIN", "CAMPAIGN_BIN", "CAP_ENV", "NORMAL_CONFIG", "MATERIALS_SRC", "KB_SRC",
              "REPO_SRC", "INPUTS_SRC", "CASES_ROOT", "GRADER_ROOT", "GRADE_CASES_ROOT"):
        v = env.get(k)
        if not v:
            problems.append(f"launcher.env: {k} is required")
        elif not os.path.exists(v):
            problems.append(f"launcher.env: {k} does not exist: {v}")
    for k in ("OUT", "STATE_DIR", "DAEMON_CONFIG"):
        if not env.get(k, "").startswith("/"):
            problems.append(f"launcher.env: {k} must be an absolute path")


def check_manifest(layout, problems):
    m = load_json(os.path.join(layout["main"], "final.json"), problems, "final.json")
    if m is None:
        return
    if m.get("campaign_id") != CAMPAIGN_ID:
        problems.append(f"final.json: campaign_id must be {CAMPAIGN_ID}")
    if m.get("primary") != ["A", "B"]:
        problems.append("final.json: primary must be exactly [A, B] (C must not be in the main comparison)")
    if m.get("problems") != PROBLEMS:
        problems.append(f"final.json: problems must be {PROBLEMS}")
    if m.get("repeats") != 1:
        problems.append("final.json: repeats must be 1")
    cfgs = {c.get("id"): c for c in m.get("configs", [])}
    for cid in ("A", "B"):
        c = cfgs.get(cid)
        if c is None:
            problems.append(f"final.json: config {cid} missing")
            continue
        for k in ("model", "model_version"):
            if c.get(k) != MODEL:
                problems.append(f"final.json: {cid}.{k} must be {MODEL}")
    a, b = cfgs.get("A", {}), cfgs.get("B", {})
    if a.get("harness") != HARNESS or a.get("harness_version") != A_VERSION:
        problems.append(f"final.json: A must be {HARNESS}@{A_VERSION}")
    if b.get("harness") != "celeris" or b.get("harness_version") != RELEASE:
        problems.append(f"final.json: B must be celeris@{RELEASE} (approved release)")
    r = b.get("routing") or {}
    if r.get("models") != [MODEL]:
        problems.append(f"final.json: B.routing.models must be [{MODEL}] (all roles Sonnet)")
    if r.get("run_harnesses") != [HARNESS]:
        problems.append(f"final.json: B.routing.run_harnesses must be [{HARNESS}] (same harness as A)")
    if r.get("online_update") is not False:
        problems.append("final.json: B.routing.online_update must be false")
    if r.get("evidence_table") or r.get("evidence_table_id"):
        problems.append("final.json: B must have no evidence table (evidence none)")
    c = cfgs.get("C")
    if c is not None:
        cr = c.get("routing") or {}
        if cr.get("models") != [MODEL] or cr.get("run_harnesses") != [HARNESS] or not cr.get("no_evidence_table"):
            problems.append("final.json: C (secondary only) must stay Sonnet-only, claude-code, no_evidence_table")
    # 承認枠は主副で二重計上しない: 2 arm 分の per_arm が全体上限以内
    bud = m.get("budget") or {}
    pa = (bud.get("per_arm") or {}).get("max_tokens")
    if isinstance(pa, int) and isinstance(bud.get("max_tokens"), int) and 2 * pa > bud["max_tokens"]:
        problems.append("final.json: 2 x per_arm.max_tokens exceeds budget.max_tokens (double counting)")
    inp = m.get("input") or {}
    if inp.get("sha256") != INPUT_MANIFEST_SHA:
        problems.append("final.json: input.sha256 differs from the approved pin")
    snap = m.get("snapshot") or {}
    if snap.get("materials_sha256") != MATERIALS_SHA:
        problems.append("final.json: snapshot.materials_sha256 differs from the approved pin")
    if snap.get("kb_snapshot") != KB_SHA:
        problems.append("final.json: snapshot.kb_snapshot differs from the approved pin")
    if K2_COMMIT not in str(snap.get("repo_commit", "")):
        problems.append("final.json: snapshot.repo_commit lost the approved K2 commit")
    im = load_json(os.path.join(layout["main"], inp.get("path", "final-input-manifest.json")), problems,
                   "final-input-manifest.json")
    if im is not None:
        if im.get("sha256") != INPUT_MANIFEST_SHA:
            problems.append("final-input-manifest.json: embedded sha256 differs from the approved pin")
        if [p.get("id") for p in im.get("problems", [])] != PROBLEMS:
            problems.append("final-input-manifest.json: problems differ from the approved set")


def check_task_and_canaries(layout, problems):
    t = load_json(os.path.join(layout["main"], "task-template.json"), problems, "task-template.json")
    if t is not None and not t.get("project_id"):
        problems.append("task-template.json: project_id missing")
    c = load_json(os.path.join(layout["main"], "canaries.json"), problems, "canaries.json")
    if c is not None and not (c.get("forbidden") and c.get("required")):
        problems.append("canaries.json: forbidden and required must both be non-empty")
    return c or {}


def check_cap_env(layout, problems):
    ce = load_json(os.path.join(layout["pinned"], "cap-env.json"), problems, "cap-env.json")
    if ce is None:
        return
    if ce.get("schema") != "celeris.campaign-cap-env/1":
        problems.append("cap-env.json: schema must be celeris.campaign-cap-env/1")
    if not ce.get("a") or ce.get("a") != ce.get("worker"):
        problems.append("cap-env.json: A and worker (B) environments must be identical (resource parity)")


def check_daemon(layout, env, problems):
    path = os.path.join(layout["main"], "daemon.toml")
    try:
        with open(path, encoding="utf-8") as f:
            raw = f.read()
        d = tomllib.loads(raw)
    except OSError:
        problems.append(f"daemon.toml: missing: {path}")
        return
    except tomllib.TOMLDecodeError as e:
        problems.append(f"daemon.toml: invalid TOML: {e}")
        return
    if re.search(r"jev", raw, re.I):
        problems.append("daemon.toml: Jev must be disabled (found a jev reference)")
    if re.search(r"(api[_-]?key|secret|password)\s*=\s*\"[^\"]+\"", raw, re.I):
        problems.append("daemon.toml: contains an inline secret value")
    for k in ("db", "workspace_root"):
        if not str(d.get(k, "")).startswith("/local/celeris/data/measure/main/"):
            problems.append(f"daemon.toml: {k} must live under /local/celeris/data/measure/main/")
    listen = (d.get("api") or {}).get("listen", "")
    if not listen.startswith("127.0.0.1:") or listen.endswith((":7710", ":7720", ":7721")):
        problems.append(f"daemon.toml: api.listen must be loopback and not a production port (got {listen!r})")
    kn = d.get("knowledge") or {}
    if (kn.get("langmem") or {}).get("enabled") is not False or (kn.get("gc") or {}).get("enabled") is not False:
        problems.append("daemon.toml: knowledge.langmem and knowledge.gc must be disabled")
    if (d.get("cron") or {}).get("seed") or "memory" in d:
        problems.append("daemon.toml: cron.seed and [memory] must not be set")
    harnesses = d.get("harnesses") or {}
    real = sorted(n for n, h in harnesses.items() if h.get("kind") != "fake")
    if real != [HARNESS]:
        problems.append(f"daemon.toml: real harnesses must be exactly [{HARNESS}] (got {real})")
    cp = harnesses.get(f"cap-probe-{HARNESS}") or {}
    if cp.get("kind") != "fake" or cp.get("env_of") != HARNESS:
        problems.append(f"daemon.toml: cap-probe-{HARNESS} must be a fake harness with env_of={HARNESS}")
    if env and cp.get("command") != env.get("CAMPAIGN_BIN"):
        problems.append("daemon.toml: cap-probe command must equal CAMPAIGN_BIN")
    for n, s in (d.get("llm_sources") or {}).items():
        if s.get("kind") != "fake" and s.get("models") != [MODEL]:
            problems.append(f"daemon.toml: llm_sources.{n}.models must be [{MODEL}]")
    for n, rd in (d.get("route_defaults") or {}).items():
        if n.startswith("cap-probe-"):
            continue
        for cand in rd.get("candidates", []):
            if cand.get("model") != MODEL:
                problems.append(f"daemon.toml: route_defaults.{n} candidate model must be {MODEL}")
    rules = (d.get("harness_selection") or {}).get("rules", [])
    if not rules or rules[0].get("genre") != f"cap-probe-{HARNESS}":
        problems.append("daemon.toml: harness_selection.rules must start with the cap-probe rule")
    if any(r.get("harness") not in (HARNESS, f"cap-probe-{HARNESS}") for r in rules):
        problems.append("daemon.toml: harness_selection routes to a harness other than claude-code")
    if (d.get("selfdeploy") or {}).get("releases_dir") != "/local/celeris/state/releases":
        problems.append("daemon.toml: selfdeploy.releases_dir must be the release store (read only)")


def check_fixed(layout, canaries, problems):
    fx = layout["fixed"]
    for p in PROBLEMS:
        for sub, name in (("cases", "case.json"), ("grade-cases", None), ("grader-private", "private.json")):
            d = os.path.join(layout["fixed_dir"], sub, "final", p)
            if not os.path.isdir(d):
                problems.append(f"fixed: missing {sub}/final/{p}")
            elif name and not os.path.isfile(os.path.join(d, name)):
                problems.append(f"fixed: missing {sub}/final/{p}/{name}")
    lock = load_json(os.path.join(layout["fixed_dir"], "cases", "final", PROBLEMS[0], "initial.lock.json"),
                     problems, "K2 initial.lock.json")
    if lock is not None and lock.get("commit") != K2_COMMIT:
        problems.append("K2 initial.lock.json: commit differs from the approved K2 tree")
    private = [fx["GRADER_ROOT"], fx["GRADE_CASES_ROOT"]]
    public = [fx["CASES_ROOT"]] + [layout["src"][k] for k in ("materials", "kb", "repo", "inputs")]
    for pr in private:
        for pu in public:
            a, b = os.path.realpath(pr), os.path.realpath(pu)
            if a == b or a.startswith(b + os.sep) or b.startswith(a + os.sep):
                problems.append(f"isolation: private root overlaps public root: {pr} vs {pu}")
    private_hashes = set()
    for pr in private:
        for f in files_under(pr):
            if os.path.isfile(f) and not os.path.islink(f) and os.path.getsize(f) >= MIN_HASH_SIZE:
                private_hashes.add(sha256_file(f))
    private_real = [os.path.realpath(p) for p in private]
    forbidden = [s.encode() for s in canaries.get("forbidden", [])]
    for pu in public:
        if not os.path.isdir(pu):
            continue
        for f in files_under(pu):
            rel = os.path.relpath(f, pu)
            if any(part in LEAK_NAMES for part in rel.split(os.sep)):
                problems.append(f"leak: grader-looking path in public input: {pu}/{rel}")
                continue
            if os.path.islink(f):
                tgt = os.path.realpath(f)
                if any(tgt == p or tgt.startswith(p + os.sep) for p in private_real):
                    problems.append(f"leak: public symlink resolves into private grader: {pu}/{rel}")
                continue
            try:
                with open(f, "rb") as fh:
                    data = fh.read()
            except OSError:
                continue
            if len(data) >= MIN_HASH_SIZE and hashlib.sha256(data).hexdigest() in private_hashes:
                problems.append(f"leak: public file is a copy of a private grader file: {pu}/{rel}")
            elif any(s in data for s in forbidden):
                problems.append(f"leak: canary (forbidden) string in public input: {pu}/{rel}")


A_PROGRAM = "/home/rmaeda/.local/share/claude/versions/2.1.303"
A_PROGRAM_SHA = "375988f457756524234695cf2d8f64ace99c5f68682e47ea24862a843c191450"
HEX64 = re.compile(r"^[0-9a-f]{64}$")


def check_lock(layout, lock_dir, env, problems):
    """生成済み lock の内容を、承認 pin・実 file の hash・launcher.env・cap-env と突き合わせる。"""
    el = load_json(os.path.join(lock_dir, "env-lock.json"), problems, "env-lock.json")
    lk = load_json(os.path.join(lock_dir, "lock.json"), problems, "lock.json")
    if el is not None:
        if el.get("schema") != "celeris-env-lock/3":
            problems.append("env-lock.json: schema must be celeris-env-lock/3")
        if el.get("campaign_id") != CAMPAIGN_ID:
            problems.append(f"env-lock.json: campaign_id must be {CAMPAIGN_ID}")
        if el.get("campaign_sha256") != CAMPAIGN_SHA:
            problems.append("env-lock.json: campaign_sha256 differs from the approved pin")
        bc = el.get("bc") or {}
        if bc.get("release") != RELEASE:
            problems.append(f"env-lock.json: bc.release must be {RELEASE}")
        prog = (el.get("a") or {}).get("program") or {}
        if prog.get("path") != A_PROGRAM or prog.get("sha256") != A_PROGRAM_SHA:
            problems.append("env-lock.json: a.program path/sha256 differs from the approved A binary pin")
        elif os.path.isfile(prog["path"]) and sha256_file(prog["path"]) != prog["sha256"]:
            problems.append("env-lock.json: a.program sha256 differs from the binary on disk")
        if "worker_programs" in bc and bc["worker_programs"] not in ([],):
            for w in bc["worker_programs"]:
                path, sha = (w or {}).get("path"), (w or {}).get("sha256")
                if not path or not sha or (os.path.isfile(path) and sha256_file(path) != sha):
                    problems.append("env-lock.json: worker program path/sha256 does not match the binary on disk")
        arms = {a.get("id"): a for a in bc.get("arms", [])}
        if set(arms) != {"B", "C"}:
            problems.append("env-lock.json: bc.arms must be exactly B and C")
        for aid, a in arms.items():
            if a.get("models") != [MODEL] or a.get("run_harnesses") != [HARNESS] or a.get("online_update"):
                problems.append(f"env-lock.json: arm {aid} must be Sonnet-only, claude-code, online_update false")
            if a.get("harness_version") != RELEASE:
                problems.append(f"env-lock.json: arm {aid} harness_version must be {RELEASE}")
            if a.get("evidence_table_id"):
                problems.append(f"env-lock.json: arm {aid} must have no evidence table")
            if aid == "C" and not a.get("no_evidence_table"):
                problems.append("env-lock.json: arm C must declare no_evidence_table (Jev disabled, C calls 0)")
            if not any(r.get("model_id") == MODEL for r in a.get("catalog", [])):
                problems.append(f"env-lock.json: arm {aid} catalog lacks {MODEL}")
        capenv = load_json(os.path.join(layout["pinned"], "cap-env.json"), [], "cap-env")
        if capenv is not None and el.get("cap_env") is not None:
            tc = el["cap_env"].get("toolchain") or {}
            if any(tc.get(k) != capenv.get("a", {}).get(k) for k in ("ro_mounts", "sockets", "env")):
                problems.append("env-lock.json: cap_env toolchain differs from pinned/cap-env.json")
        elif el.get("cap_env") is None:
            problems.append("env-lock.json: cap_env missing")
        if env and env.get("CELERIS_BIN") and RELEASE not in env["CELERIS_BIN"]:
            problems.append("launcher.env: CELERIS_BIN is not the release pinned in env-lock.json")
    if lk is not None:
        for k, want in (("campaign_sha256", CAMPAIGN_SHA), ("measurement_id", CAMPAIGN_ID),
                        ("materials_sha256", MATERIALS_SHA), ("kb_snapshot", KB_SHA)):
            if lk.get(k) != want:
                problems.append(f"lock.json: {k} differs from the approved pin")
        if not HEX64.match(str(lk.get("sha256", ""))):
            problems.append("lock.json: seal sha256 missing or malformed")
        cases = {c.get("id"): c for c in lk.get("cases", [])}
        if set(cases) < set(PROBLEMS):
            problems.append("lock.json: cases must include K2 and K4")
        init = (cases.get(PROBLEMS[0]) or {}).get("initial") or {}
        if cases.get(PROBLEMS[0]) is not None and init.get("commit") != K2_COMMIT:
            problems.append("lock.json: K2 initial commit differs from the approved K2 tree")
        for c in lk.get("configs", (lk.get("execution") or {}).get("configs", [])):
            if c.get("model") != MODEL or c.get("model_version") != MODEL:
                problems.append(f"lock.json: config {c.get('id')} is not Sonnet only")
            if str(c.get("policy_version", "")).startswith("jev") and c.get("id") != "C":
                problems.append("lock.json: Jev policy outside the secondary arm C")
        if el is not None and el.get("campaign_sha256") != lk.get("campaign_sha256"):
            problems.append("env-lock.json / lock.json: campaign_sha256 disagree")


def check_evidence(layout, problems):
    for name, case, arm in (("cap-a-k2.json", PROBLEMS[0], "A"), ("cap-a-k4.json", PROBLEMS[1], "A"),
                            ("cap-b.json", PROBLEMS[0], "B")):
        d = load_json(os.path.join(layout["evidence"], name), problems, name)
        if d is None:
            continue
        if d.get("schema") != "celeris.campaign-cap-probe/1" or d.get("ok") is not True \
                or d.get("case") != case or d.get("arm") != arm or d.get("paid_calls_made") != 0:
            problems.append(f"{name}: must be an ok cap-probe of case {case} arm {arm} with paid_calls_made 0")
    pf = load_json(os.path.join(layout["main"], "evidence", "k2-preflight.json"), problems,
                   "real K2 preflight evidence (pinned/main/evidence/k2-preflight.json)")
    if pf is not None:
        done = set(pf.get("checked", []))
        if pf.get("schema") != "celeris.campaign-preflight/1" or pf.get("ok") is not True \
                or pf.get("missing") or pf.get("paid_calls_made") != 0 \
                or pf.get("campaign_sha256") != CAMPAIGN_SHA or not PREFLIGHT_STAGES <= done:
            problems.append("real K2 preflight evidence: must be ok, missing=[], paid_calls_made 0, "
                            "approved campaign_sha256 and cover " + ",".join(sorted(PREFLIGHT_STAGES)))


def check_preflight_argv(layout, env, problems):
    """実 K2 preflight が launcher.env の CAMPAIGN_BIN（承認済み修正後 binary）で走ったこと。"""
    path = os.path.join(layout["evidence"], "main-k2", "run", "argv.txt")
    try:
        with open(path, encoding="utf-8") as f:
            lines = [ln for ln in f.read().splitlines() if not ln.startswith("#")]
    except OSError:
        problems.append(f"real K2 preflight argv: missing: {path}")
        return
    if len(lines) < 2 or lines[1] != "preflight":
        problems.append("real K2 preflight argv: must be a `preflight` invocation")
    elif env and lines[0] != env.get("CAMPAIGN_BIN"):
        problems.append("real K2 preflight argv: binary differs from launcher.env CAMPAIGN_BIN")


def run_checks(root, lock_dir=None):
    root = os.path.abspath(root)
    pinned = os.path.join(root, "pinned")
    main_dir = os.path.join(pinned, "main")
    fixed_dir = os.path.join(main_dir, "fixed")
    layout = {
        "root": root, "pinned": pinned, "main": main_dir, "fixed_dir": fixed_dir,
        "evidence": os.path.join(pinned, "evidence"),
        "fixed": {"CASES_ROOT": os.path.join(fixed_dir, "cases"),
                  "GRADER_ROOT": os.path.join(fixed_dir, "grader-private"),
                  "GRADE_CASES_ROOT": os.path.join(fixed_dir, "grade-cases")},
        "src": {k: os.path.join(root, "measure-src", "main", k) for k in ("materials", "kb", "repo", "inputs")},
    }
    if not os.path.isdir(main_dir):
        return [f"missing pinned main dir: {main_dir}"]
    problems = []
    env = parse_env(os.path.join(main_dir, "launcher.env"), problems)
    if env is not None:
        check_env(layout, env, problems)
    check_manifest(layout, problems)
    canaries = check_task_and_canaries(layout, problems)
    check_cap_env(layout, problems)
    check_daemon(layout, env, problems)
    check_fixed(layout, canaries, problems)
    # 寿命の短い work tree への参照が pinned/main に残っていないこと（fixed 配下は資産の内容なので除く）
    for f in files_under(main_dir):
        if f.startswith(fixed_dir + os.sep) or os.path.islink(f):
            continue
        try:
            with open(f, encoding="utf-8") as fh:
                text = fh.read()
        except (OSError, UnicodeDecodeError):
            continue
        if EPHEMERAL.search(text):
            problems.append(f"stale reference to an ephemeral work tree: {os.path.relpath(f, root)}")
    check_lock(layout, lock_dir or main_dir, env, problems)
    check_evidence(layout, problems)
    check_preflight_argv(layout, env, problems)
    return problems


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--root", required=True, help="成果物 root（pinned/ と measure-src/ を持つ dir）")
    ap.add_argument("--lock-dir", help="env-lock.json / lock.json の dir（既定 <root>/pinned/main）")
    args = ap.parse_args(argv)
    problems = run_checks(args.root, args.lock_dir)
    if problems:
        for p in problems:
            print(f"NG {p}", file=sys.stderr)
        print(f"check-main-pins: {len(problems)} problem(s)", file=sys.stderr)
        return 1
    print("check-main-pins: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
