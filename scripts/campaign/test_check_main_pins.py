"""check-main-pins.py の offline 試験。合成 fixture だけを使い、実 artifacts・ネットワーク・LLM には触れない。"""
import copy
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


def build(root):
    """全部の検査に通る最小の artifacts root を作る。"""
    pinned = os.path.join(root, "pinned")
    main = os.path.join(pinned, "main")
    fixed = os.path.join(main, "fixed")
    campaign_bin = os.path.join(pinned, "bin", "celeris-campaign")
    celeris_bin = os.path.join(root, "releases", cmp_.RELEASE, "bin", "celeris")
    for p in (campaign_bin, celeris_bin):
        write(p, "#!/bin/sh\n")
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
    write_json(os.path.join(main, "lock", "env-lock.json"),
               {"schema": "celeris-env-lock/3", "campaign_id": cmp_.CAMPAIGN_ID,
                "campaign_sha256": cmp_.CAMPAIGN_SHA, "bc": {"release": cmp_.RELEASE}})
    write_json(os.path.join(main, "lock", "lock.json"), {"campaign_sha256": cmp_.CAMPAIGN_SHA})
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

    def test_cli_exit_codes(self):
        cmd = [sys.executable, "-I", SCRIPT, "--root", self.root]
        self.assertEqual(subprocess.run(cmd, capture_output=True).returncode, 0)
        os.remove(os.path.join(self.main, "lock", "lock.json"))
        r = subprocess.run(cmd, capture_output=True, text=True)
        self.assertEqual(r.returncode, 1)
        self.assertIn("lock.json: missing", r.stderr)

    def test_missing_lock_and_evidence(self):
        shutil.rmtree(os.path.join(self.main, "lock"))
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

    def test_missing_paths(self):
        os.remove(os.path.join(self.root, "pinned", "bin", "celeris-campaign"))
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

    def test_tampered_lock_hash(self):
        write_json(os.path.join(self.main, "lock", "env-lock.json"),
                   {"schema": "celeris-env-lock/3", "campaign_id": cmp_.CAMPAIGN_ID,
                    "campaign_sha256": "f" * 64, "bc": {"release": cmp_.RELEASE}})
        self.assertFlags("campaign_sha256 differs")

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
