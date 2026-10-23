"""check-main-pins.py の offline 試験。合成 fixture だけを使い、実 artifacts・ネットワーク・LLM には触れない。"""
import copy
import hashlib
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "check-main-pins.py")
spec = importlib.util.spec_from_file_location("check_main_pins", SCRIPT)
cmp_ = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cmp_)

K2, K4 = cmp_.PROBLEMS
PRIVATE_BODY = '{"answer": "hidden-grader-answer-0123456789abcdef"}'


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)


def write_json(path, obj):
    write(path, json.dumps(obj, indent=1))


MANIFEST = {
    "campaign_id": cmp_.CAMPAIGN_ID,
    "input": {"path": "final-input-manifest.json", "sha256": cmp_.INPUT_MANIFEST_SHA},
    "problems": [K2, K4],
    "configs": [
        {"id": "A", "harness": "claude-code", "harness_version": cmp_.A_VERSION,
         "model": cmp_.MODEL, "model_version": cmp_.MODEL},
        {"id": "B", "harness": "celeris", "harness_version": cmp_.RELEASE,
         "model": cmp_.MODEL, "model_version": cmp_.MODEL,
         "routing": {"lower_policy": "mlsys-min-1", "online_update": False, "models": [cmp_.MODEL],
                     "run_harnesses": ["claude-code"]}},
        {"id": "C", "harness": "celeris", "harness_version": cmp_.RELEASE,
         "model": cmp_.MODEL, "model_version": cmp_.MODEL,
         "routing": {"lower_policy": "mlsys-min-1", "online_update": False, "models": [cmp_.MODEL],
                     "run_harnesses": ["claude-code"], "no_evidence_table": True}},
    ],
    "primary": ["A", "B"],
    "secondary": {"pair": ["B", "C"], "subset": [K2, K4]},
    "budget": {"max_tokens": 3000000, "per_arm": {"max_tokens": 1300000}},
    "snapshot": {"repo_commit": "final-cases-1: k2=" + cmp_.K2_COMMIT + ", k4=none",
                 "materials_sha256": cmp_.MATERIALS_SHA, "kb_snapshot": cmp_.KB_SHA},
    "repeats": 1,
}

DAEMON = """
db = "/local/celeris/data/measure/main/state/celeris.db"
workspace_root = "/local/celeris/data/measure/main/state/ws"
[api]
listen = "127.0.0.1:7731"
[knowledge]
root = "/local/celeris/data/measure/main/state/kb"
[knowledge.langmem]
enabled = false
[knowledge.gc]
enabled = false
[llm_sources.claude_oauth]
kind = "claude_oauth"
models = ["claude-sonnet-5-5"]
[llm_sources.cap-probe]
kind = "fake"
models = ["fake"]
[harnesses.claude-code]
kind = "claude_code"
[route_defaults.claude-code]
candidates = [{ source = "claude_oauth", model = "claude-sonnet-5-5" }]
[harnesses.cap-probe-claude-code]
kind = "fake"
command = "@CAMPAIGN_BIN@"
env_of = "claude-code"
[route_defaults.cap-probe-claude-code]
candidates = [{ source = "cap-probe", model = "fake" }]
[harness_selection]
rules = [
  { genre = "cap-probe-claude-code", harness = "cap-probe-claude-code" },
  { harness = "claude-code" },
]
[selfdeploy]
releases_dir = "/local/celeris/state/releases"
"""


def good_env_lock(side):
    arm = lambda i: {"id": i, "harness_version": cmp_.RELEASE, "lower_policy": "mlsys-min-1",
                     "evidence_table_id": None, "no_evidence_table": i == "C", "online_update": False,
                     "models": [cmp_.MODEL], "run_harnesses": ["claude-code"],
                     "catalog": [{"model_id": cmp_.MODEL, "tier": "unset", "source_id": "claude-oauth"}]}
    return {"schema": "celeris-env-lock/3", "campaign_id": cmp_.CAMPAIGN_ID,
            "campaign_sha256": cmp_.CAMPAIGN_SHA,
            "a": {"program": {"path": cmp_.A_PROGRAM, "sha256": cmp_.A_PROGRAM_SHA, "version": "2.1.303 (Claude Code)"}},
            "bc": {"release": cmp_.RELEASE, "worker_programs": [], "arms": [arm("B"), arm("C")]},
            "cap_env": {"schema": "celeris.campaign-cap-env/1", "toolchain": side, "mounts": []}}


def good_lock():
    cfg = lambda i, pol: {"id": i, "model": cmp_.MODEL, "model_version": cmp_.MODEL, "policy_version": pol}
    return {"schema": "celeris.campaign-lock/1", "version": "1", "measurement_id": cmp_.CAMPAIGN_ID,
            "campaign_sha256": cmp_.CAMPAIGN_SHA, "materials_sha256": cmp_.MATERIALS_SHA,
            "kb_snapshot": cmp_.KB_SHA,
            "cases": [{"id": K2, "initial": {"commit": cmp_.K2_COMMIT}}, {"id": K4}],
            "execution": {"configs": [cfg("A", "ops-baseline"), cfg("B", "fixed"), cfg("C", "jev-1")]},
            "sha256": "a" * 64}


def build(root):
    """全部の検査に通る最小の artifacts root を作る。"""
    pinned = os.path.join(root, "pinned")
    main = os.path.join(pinned, "main")
    fixed = os.path.join(main, "fixed")
    campaign_bin = os.path.join(root, cmp_.CAMPAIGN_BIN_REL)
    celeris_bin = os.path.join(root, "releases", cmp_.RELEASE, "bin", "celeris")
    for p in (campaign_bin, celeris_bin):
        write(p, "#!/bin/sh\n")
    cmp_.CAMPAIGN_BIN_SHA = cmp_.sha256_file(campaign_bin)  # fixture の偽 binary を承認済みとみなす
    write(os.path.join(pinned, "evidence", "main-k2", "run", "argv.txt"),
          f"# 2026-10-19T00:00:00Z pid=1\n{campaign_bin}\npreflight\n--manifest\nx\n# end\n")
    normal = os.path.join(root, "normal-config.toml")
    write(normal, "# normal\n")
    src = {k: os.path.join(root, "measure-src", "main", k) for k in ("materials", "kb", "repo", "inputs")}
    for k, d in src.items():
        write(os.path.join(d, "readme.txt"), f"public {k} content for the measurement run\n")
    for p in (K2, K4):
        write_json(os.path.join(fixed, "cases", "final", p, "case.json"), {"id": p})
        write(os.path.join(fixed, "grade-cases", "final", p, "hidden", "run.sh"), f"echo grade {p} secret script\n")
        write(os.path.join(fixed, "grader-private", "final", p, "private.json"), PRIVATE_BODY + p)
    write_json(os.path.join(fixed, "cases", "final", K2, "initial.lock.json"), {"commit": cmp_.K2_COMMIT})
    env = {
        "MANIFEST": "final.json", "DAEMON_CONFIG": "/local/celeris/data/measure/main/daemon.toml",
        "NORMAL_CONFIG": normal, "CANARIES": "canaries.json", "TASK_TEMPLATE": "task-template.json",
        "A_ACCOUNT": "claude_max_lab", "CELERIS_BIN": celeris_bin, "RELEASE": cmp_.RELEASE,
        "OUT": "/local/celeris/data/measure/main/out", "STATE_DIR": "/local/celeris/data/measure/main/state",
        "CAP_ENV": os.path.join(pinned, "cap-env.json"),
        "MATERIALS_SRC": src["materials"], "KB_SRC": src["kb"], "REPO_SRC": src["repo"],
        "INPUTS_SRC": src["inputs"], "CAMPAIGN_BIN": campaign_bin,
        "CASES_ROOT": os.path.join(fixed, "cases"), "GRADER_ROOT": os.path.join(fixed, "grader-private"),
        "GRADE_CASES_ROOT": os.path.join(fixed, "grade-cases"), "MEASUREMENT_ID": cmp_.CAMPAIGN_ID,
    }
    write(os.path.join(main, "launcher.env"), "".join(f"{k}={v}\n" for k, v in env.items()))
    write_json(os.path.join(main, "final.json"), MANIFEST)
    write_json(os.path.join(main, "final-input-manifest.json"),
               {"sha256": cmp_.INPUT_MANIFEST_SHA, "problems": [{"id": K2}, {"id": K4}]})
    write_json(os.path.join(main, "task-template.json"), {"project_id": "P1"})
    write_json(os.path.join(main, "canaries.json"), {"forbidden": ["forbidden-canary-xyz"], "required": ["req"]})
    write(os.path.join(main, "daemon.toml"), DAEMON.replace("@CAMPAIGN_BIN@", campaign_bin))
    side = {"ro_mounts": ["/tc"], "sockets": [], "env": [["PATH", "/bin"]]}
    write_json(os.path.join(pinned, "cap-env.json"),
               {"schema": "celeris.campaign-cap-env/1", "a": side, "worker": side})
    for name, case, arm in (("cap-a-k2.json", K2, "A"), ("cap-a-k4.json", K4, "A"), ("cap-b.json", K2, "B")):
        write_json(os.path.join(pinned, "evidence", name),
                   {"schema": "celeris.campaign-cap-probe/1", "ok": True, "case": case, "arm": arm,
                    "paid_calls_made": 0})
    write_json(os.path.join(main, "env-lock.json"), good_env_lock(side))
    write_json(os.path.join(main, "lock.json"), good_lock())
    write_json(os.path.join(main, "evidence", "k2-preflight.json"),
               {"schema": "celeris.campaign-preflight/1", "ok": True, "paid_calls_made": 0, "missing": [],
                "campaign_sha256": cmp_.CAMPAIGN_SHA, "checked": sorted(cmp_.PREFLIGHT_STAGES)})
    return root


class CheckMainPins(unittest.TestCase):
    def setUp(self):
        self.root = build(tempfile.mkdtemp(prefix="pins-"))
        self.main = os.path.join(self.root, "pinned", "main")
        self.addCleanup(shutil.rmtree, self.root, ignore_errors=True)

    def problems(self):
        return cmp_.run_checks(self.root)

    def assertFlags(self, needle):
        got = self.problems()
        self.assertTrue(any(needle in p for p in got), f"{needle!r} not in {got}")

    def edit_manifest(self, fn):
        path = os.path.join(self.main, "final.json")
        with open(path) as f:
            m = json.load(f)
        fn(m)
        write_json(path, m)

    def test_success(self):
        self.assertEqual(self.problems(), [])

    def grade_case_with_archive(self, stage=True, tamper=False):
        """K4 の grade-case に input_archive を書き、公開 inputs と（必要なら）grade-case の隣へ同じ tgz を置く。"""
        gc_dir = os.path.join(self.main, "fixed", "grade-cases", "final", K4)
        body = b"\x1f\x8b fake tgz bytes for the pinned k4 raw archive 0123456789"
        pub = os.path.join(self.root, "measure-src", "main", "inputs", "main-v3-n2-43706.tgz")
        with open(pub, "wb") as f:
            f.write(body)
        sha = hashlib.sha256(body).hexdigest()
        write_json(os.path.join(gc_dir, f"{K4}.json"),
                   {"schema": "celeris.grade-case/1", "id": K4, "split": "final",
                    "recompute": {"report": "report.md", "metrics": [],
                                  "input_archive": {"path": "main-v3-n2-43706.tgz", "sha256": sha}}})
        if stage:
            with open(os.path.join(gc_dir, "main-v3-n2-43706.tgz"), "wb") as f:
                f.write(body + (b"x" if tamper else b""))

    def test_staged_grader_archive_passes_and_is_not_a_leak(self):
        self.grade_case_with_archive()
        self.assertEqual(self.problems(), [])

    def test_unstaged_or_tampered_grader_archive_is_flagged(self):
        self.grade_case_with_archive(stage=False)
        self.assertFlags("is not staged next to the grade-case")
        self.grade_case_with_archive(stage=True, tamper=True)
        self.assertFlags("sha256 differs from the pinned value")

    def test_pins_file_overrides_the_approved_values_for_a_new_bundle(self):
        # 別 bundle（mlsys-main-4）: pins.json が無ければ main-1 の値で落ち、あれば通る。
        self.edit_manifest(lambda m: m.__setitem__("campaign_id", "mlsys-main-4"))
        self.assertFlags("campaign_id must be mlsys-main-1")
        pins = {k: getattr(cmp_, k) for k in cmp_.PIN_KEYS}
        pins["CAMPAIGN_ID"] = "mlsys-main-4"
        write_json(os.path.join(self.main, "pins.json"), pins)
        got = self.problems()
        self.assertFalse(any("final.json: campaign_id must be" in p for p in got), got)
        # 他の main-1 固定値（MEASUREMENT_ID 等）も同じ値で見る
        self.assertTrue(any("MEASUREMENT_ID must be mlsys-main-4" in p for p in got), got)
        write_json(os.path.join(self.main, "pins.json"), {"BOGUS": 1, "PROBLEMS": "not-a-list"})
        got = self.problems()
        self.assertTrue(any("unknown pin BOGUS" in p for p in got), got)
        self.assertTrue(any("PROBLEMS must be a list" in p for p in got), got)

    def test_explicit_pins_never_fall_back_to_main1_release(self):
        for missing in ("RELEASE", "A_VERSION", "CAMPAIGN_BIN_SHA", "A_PROGRAM"):
            for value in (None, ""):
                with self.subTest(pin=missing, value=value):
                    pins = {k: getattr(cmp_, k) for k in cmp_.PIN_KEYS}
                    if value is None:
                        del pins[missing]
                    else:
                        pins[missing] = value
                    path = os.path.join(self.main, "pins.json")
                    write_json(path, pins)
                    got = cmp_.run_checks(self.root, pins=path)
                    self.assertIn(f"pins.json: {missing}: pending: 人が記入", got)
                    self.assertFalse(any("B must be celeris@" in p for p in got), got)
                    result = subprocess.run([sys.executable, SCRIPT, "--root", self.root,
                                             "--pins", path], capture_output=True, text=True)
                    self.assertEqual(result.returncode, 1)
                    self.assertIn("pending: 人が記入", result.stderr)

    def test_cli_exit_codes(self):
        cmd = [sys.executable, "-I", SCRIPT, "--root", self.root]
        # 承認済み binary の hash は定数なので、成功は同 process（fixture 用に定数を差し替え済み）で見る
        self.assertEqual(cmp_.main(["--root", self.root]), 0)
        os.remove(os.path.join(self.main, "lock.json"))
        r = subprocess.run(cmd, capture_output=True, text=True)
        self.assertEqual(r.returncode, 1)
        self.assertIn("lock.json: missing", r.stderr)

    def test_missing_lock_and_evidence(self):
        os.remove(os.path.join(self.main, "env-lock.json"))
        os.remove(os.path.join(self.main, "lock.json"))
        os.remove(os.path.join(self.main, "evidence", "k2-preflight.json"))
        got = self.problems()
        self.assertEqual(len(got), 3, got)
        self.assertFlags("env-lock.json: missing")
        self.assertFlags("real K2 preflight evidence")

    def test_failed_preflight_is_not_success(self):
        write_json(os.path.join(self.main, "evidence", "k2-preflight.json"),
                   {"schema": "celeris.campaign-preflight/1", "ok": False, "paid_calls_made": 0,
                    "missing": [{"check": "measure-env"}], "campaign_sha256": cmp_.CAMPAIGN_SHA, "checked": []})
        self.assertFlags("must be ok")

    def test_campaign_bin_must_be_approved_binary(self):
        path = os.path.join(self.root, cmp_.CAMPAIGN_BIN_REL)
        write(path, "#!/bin/sh\n# swapped\n")
        self.assertFlags("CAMPAIGN_BIN sha256 differs")

    def test_campaign_bin_pointing_elsewhere(self):
        path = os.path.join(self.main, "launcher.env")
        with open(path) as f:
            text = f.read()
        write(path, text.replace(os.path.join(self.root, cmp_.CAMPAIGN_BIN_REL), "/old/bin/celeris-campaign"))
        self.assertFlags("CAMPAIGN_BIN must be")

    def test_preflight_argv_binary_mismatch(self):
        write(os.path.join(self.root, "pinned", "evidence", "main-k2", "run", "argv.txt"),
              "# t\n/old/bin/celeris-campaign\npreflight\n# end\n")
        self.assertFlags("argv: binary differs")

    def test_missing_paths(self):
        os.remove(os.path.join(self.root, cmp_.CAMPAIGN_BIN_REL))
        self.assertFlags("CAMPAIGN_BIN does not exist")
        shutil.rmtree(os.path.join(self.main, "fixed", "grade-cases"))
        self.assertFlags("GRADE_CASES_ROOT does not exist")

    def test_launcher_env_root_not_migrated(self):
        path = os.path.join(self.main, "launcher.env")
        with open(path) as f:
            text = f.read()
        old = "/x/wu/pin-values/repos/agent-platform/crates/campaign/cases"
        text = text.replace(os.path.join(self.main, "fixed", "cases"), old)
        write(path, text)
        self.assertFlags("CASES_ROOT must be")
        self.assertFlags("ephemeral work tree")

    def test_secret_value_in_env(self):
        with open(os.path.join(self.main, "launcher.env"), "a") as f:
            f.write("API_KEY=sk-abc\n")
        self.assertFlags("API_KEY looks like a secret")

    def test_tampered_pins(self):
        self.edit_manifest(lambda m: m["input"].update(sha256="0" * 64))
        self.edit_manifest(lambda m: m["snapshot"].update(kb_snapshot="1" * 64))
        self.assertFlags("input.sha256")
        self.assertFlags("kb_snapshot")

    def edit_lock(self, name, fn):
        path = os.path.join(self.main, name)
        with open(path) as f:
            d = json.load(f)
        fn(d)
        write_json(path, d)

    def test_tampered_lock_hash(self):
        self.edit_lock("env-lock.json", lambda d: d.update(campaign_sha256="f" * 64))
        self.assertFlags("env-lock.json: campaign_sha256 differs")
        self.assertFlags("campaign_sha256 disagree")

    def test_lock_json_pin_tampered(self):
        for k in ("campaign_sha256", "materials_sha256", "kb_snapshot", "measurement_id"):
            self.setUp()
            self.edit_lock("lock.json", lambda d: d.update({k: "x" * 64}))
            self.assertFlags(f"lock.json: {k} differs")
        self.edit_lock("lock.json", lambda d: d.update(sha256="short"))
        self.assertFlags("seal sha256 missing")

    def test_lock_k2_commit_changed(self):
        self.edit_lock("lock.json", lambda d: d["cases"][0]["initial"].update(commit="deadbeef"))
        self.assertFlags("K2 initial commit")

    def test_lock_non_sonnet_or_jev_outside_c(self):
        self.edit_lock("lock.json", lambda d: d["execution"]["configs"][1].update(model="claude-opus-5-5"))
        self.assertFlags("not Sonnet only")
        self.edit_lock("lock.json", lambda d: d["execution"]["configs"][1].update(policy_version="jev-1"))
        self.assertFlags("Jev policy outside")

    def test_env_lock_binary_path_and_hash_mismatch(self):
        self.edit_lock("env-lock.json", lambda d: d["a"]["program"].update(path="/tmp/other-claude"))
        self.assertFlags("a.program path/sha256 differs")
        self.setUp()
        self.edit_lock("env-lock.json", lambda d: d["a"]["program"].update(sha256="0" * 64))
        self.assertFlags("a.program path/sha256 differs")

    def test_env_lock_binary_content_changed_on_disk(self):
        binary = os.path.join(self.root, "claude-bin")
        write(binary, "binary v1")
        old = (cmp_.A_PROGRAM, cmp_.A_PROGRAM_SHA)
        cmp_.A_PROGRAM, cmp_.A_PROGRAM_SHA = binary, "0" * 64
        self.addCleanup(lambda: setattr(cmp_, "A_PROGRAM", old[0]) or setattr(cmp_, "A_PROGRAM_SHA", old[1]))
        self.edit_lock("env-lock.json", lambda d: d["a"]["program"].update(path=binary, sha256="0" * 64))
        self.assertFlags("differs from the binary on disk")

    def test_env_lock_worker_program_hash_mismatch(self):
        w = os.path.join(self.root, "worker-bin")
        write(w, "worker")
        self.edit_lock("env-lock.json", lambda d: d["bc"].update(worker_programs=[{"path": w, "sha256": "1" * 64}]))
        self.assertFlags("worker program")

    def test_env_lock_arm_equivalence(self):
        self.edit_lock("env-lock.json", lambda d: d["bc"]["arms"][0].update(models=["claude-opus-5-5"]))
        self.assertFlags("arm B must be Sonnet-only")
        self.setUp()
        self.edit_lock("env-lock.json", lambda d: d["bc"]["arms"][1].update(no_evidence_table=False))
        self.assertFlags("arm C must declare no_evidence_table")
        self.setUp()
        self.edit_lock("env-lock.json", lambda d: d["bc"]["arms"][0].update(evidence_table_id="t1"))
        self.assertFlags("no evidence table")

    def test_env_lock_cap_env_and_release_mismatch(self):
        self.edit_lock("env-lock.json", lambda d: d["cap_env"]["toolchain"].update(ro_mounts=["/other"]))
        self.assertFlags("cap_env toolchain differs")
        self.setUp()
        self.edit_lock("env-lock.json", lambda d: d["bc"].update(release="aaaaaaaaaaaa"))
        self.assertFlags("bc.release must be")

    def test_k2_tree_changed(self):
        write_json(os.path.join(self.main, "fixed", "cases", "final", K2, "initial.lock.json"),
                   {"commit": "deadbeef"})
        self.assertFlags("approved K2 tree")

    def test_non_sonnet_and_c_in_primary(self):
        self.edit_manifest(lambda m: m["configs"][1]["routing"].update(models=["claude-opus-5-5"]))
        self.edit_manifest(lambda m: m.update(primary=["A", "B", "C"]))
        self.assertFlags("B.routing.models")
        self.assertFlags("primary must be")

    def test_harness_and_evidence_table_parity(self):
        self.edit_manifest(lambda m: m["configs"][1]["routing"].update(run_harnesses=["codex"],
                                                                       evidence_table="t"))
        self.assertFlags("run_harnesses")
        self.assertFlags("no evidence table")

    def test_double_counted_budget(self):
        self.edit_manifest(lambda m: m["budget"]["per_arm"].update(max_tokens=2000000))
        self.assertFlags("double counting")

    def test_daemon_jev_and_other_model(self):
        path = os.path.join(self.main, "daemon.toml")
        with open(path) as f:
            t = f.read()
        write(path, t.replace('models = ["claude-sonnet-5-5"]', 'models = ["claude-opus-5-5"]') + "\n[jev]\nenabled = true\n")
        self.assertFlags("Jev must be disabled")
        self.assertFlags("llm_sources.claude_oauth.models")

    def test_daemon_extra_real_harness(self):
        with open(os.path.join(self.main, "daemon.toml"), "a") as f:
            f.write('\n[harnesses.codex]\nkind = "codex"\n')
        self.assertFlags("real harnesses must be exactly")

    def test_cap_env_parity(self):
        path = os.path.join(self.root, "pinned", "cap-env.json")
        with open(path) as f:
            d = json.load(f)
        d["worker"] = copy.deepcopy(d["a"])
        d["worker"]["ro_mounts"] = []
        write_json(path, d)
        self.assertFlags("must be identical")

    def test_leak_private_file_copy_in_public(self):
        copy_to = os.path.join(self.root, "measure-src", "main", "materials", "answer-copy.txt")
        write(copy_to, PRIVATE_BODY + K2)
        self.assertFlags("copy of a private grader file")

    def test_leak_private_name_in_cases(self):
        write(os.path.join(self.main, "fixed", "cases", "final", K2, "private.json"), "{}")
        self.assertFlags("grader-looking path")

    def test_leak_canary_string(self):
        write(os.path.join(self.root, "measure-src", "main", "repo", "notes.md"), "x forbidden-canary-xyz y\n")
        self.assertFlags("canary (forbidden) string")

    def test_leak_symlink_into_grader(self):
        link = os.path.join(self.root, "measure-src", "main", "inputs", "peek")
        os.symlink(os.path.join(self.main, "fixed", "grader-private", "final", K2, "private.json"), link)
        self.assertFlags("symlink resolves into private grader")

    def test_private_root_aliased_as_public(self):
        kb = os.path.join(self.root, "measure-src", "main", "kb")
        shutil.rmtree(kb)
        os.symlink(os.path.join(self.main, "fixed", "grader-private"), kb)
        self.assertFlags("overlaps public root")


if __name__ == "__main__":
    unittest.main()
