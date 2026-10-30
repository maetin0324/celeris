"""ops-r1-measure-config.sh の offline 試験。合成の release dir と一時 state だけを使う（network・LLM・daemon なし）。
HOME は一時 dir に差し替え、本番 path の拒否も実 HOME に触れずに確かめる。"""
import hashlib
import json
import os
import shutil
import stat
import subprocess
import tempfile
import tomllib
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "ops-r1-measure-config.sh")
SHELL = shutil.which("dash") or "/bin/sh"


def rd(p):
    with open(p, encoding="utf-8") as f:
        return f.read()


def rdb(p):
    with open(p, "rb") as f:
        return f.read()


def wr(p, s):
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "w", encoding="utf-8") as f:
        f.write(s)


class MeasureConfigTest(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = os.path.realpath(self._tmp.name)
        self.home = os.path.join(self.root, "home")
        os.makedirs(self.home)
        # 合成の release dir（releases/<sha12>/{bin/celeris,manifest.json}）。
        self.release = os.path.join(self.root, "releases", "94f86928dc13")
        wr(os.path.join(self.release, "bin", "celeris"), "#!/bin/sh\necho fake celeris\n")
        wr(os.path.join(self.release, "manifest.json"),
           json.dumps({"sha": "94f86928dc13" + "0" * 28, "sha12": "94f86928dc13", "schema_version": 102}, indent=2))
        # 合成の skills（cos-operator・cos-inbox-triage と、その他 1 つ。SKILL.md の無い dir は写さない）。
        self.skills = os.path.join(self.root, "skills")
        for name in ("cos-operator", "cos-inbox-triage", "yomiyasu"):
            wr(os.path.join(self.skills, name, "SKILL.md"), f"---\nname: {name}\n---\n# {name}\n")
        wr(os.path.join(self.skills, "cos-operator", "references", "ops.md"), "ref\n")
        os.makedirs(os.path.join(self.skills, "not-a-skill"))
        wr(os.path.join(self.skills, "README.md"), "readme\n")
        self.stub = os.path.join(self.root, "stub.sh")
        wr(self.stub, "#!/bin/sh\ncat >/dev/null\n")
        os.chmod(self.stub, 0o755)

    def tearDown(self):
        self._tmp.cleanup()

    def run_script(self, *args, state=None, out=None, models="cos-opus", base=True):
        argv = [SHELL, SCRIPT]
        if base:
            argv += ["--state", state or os.path.join(self.root, "state"),
                     "--release-dir", self.release,
                     "--models", models,
                     "--out", out or os.path.join(self.root, "cfg", "daemon.toml"),
                     "--skills-src", self.skills]
        argv += list(args)
        env = {"HOME": self.home, "PATH": os.environ.get("PATH", "/usr/bin:/bin")}
        return subprocess.run(argv, capture_output=True, text=True, env=env)

    def gen_stub(self, *extra, state=None, out=None, models="cos-opus"):
        r = self.run_script("--port", "17861", "--harness", "stub", "--stub-command", self.stub, *extra,
                            state=state, out=out, models=models)
        self.assertEqual(r.returncode, 0, r.stderr)
        return out or os.path.join(self.root, "cfg", "daemon.toml")

    def test_stub_config_fields(self):
        out = self.gen_stub()
        text = rd(out)
        cfg = tomllib.loads(text)
        state = os.path.join(self.root, "state")
        # db・token・knowledge・workspace・scratch は全部 --state の下。
        self.assertEqual(cfg["db"], os.path.join(state, "celeris.db"))
        self.assertEqual(cfg["workspace_root"], os.path.join(state, "ws"))
        self.assertEqual(cfg["api"]["listen"], "127.0.0.1:17861")
        self.assertEqual(cfg["api"]["tokens"], [{"principal": "operator:ops-r1", "token_file": os.path.join(state, "token")}])
        self.assertNotIn("token_file", cfg["api"])
        self.assertEqual(cfg["knowledge"]["root"], os.path.join(state, "kb"))
        self.assertEqual(cfg["knowledge"]["default_mounts"], ["kb:trial"])
        self.assertFalse(cfg["knowledge"]["langmem"]["enabled"])
        self.assertFalse(cfg["knowledge"]["gc"]["enabled"])
        self.assertEqual(cfg["scratch"]["dir"], os.path.join(state, "scratch"))
        self.assertEqual(cfg["selfdeploy"]["releases_dir"], os.path.join(self.root, "releases"))
        for absent in ("memory", "llm_proxy", "mcp", "cron", "accounts", "clusters"):
            self.assertNotIn(absent, cfg)
        # 並列 1・試行 1。
        self.assertEqual(cfg["max_concurrency"], 1)
        self.assertEqual(cfg["max_requeues"], 0)
        self.assertEqual(cfg["execution"]["max_parallel_work_units"], 1)
        self.assertEqual(cfg["execution"]["tree"]["max_parallel_child_tasks"], 1)
        # CoS: enabled・policy 6・route（cos-opus = CoS opus、他 sonnet）。
        self.assertTrue(cfg["cos"]["enabled"])
        self.assertEqual(cfg["cos"]["triage"], {"policy_skill": "cos-inbox-triage", "policy_version": "6"})
        self.assertEqual(cfg["cos"]["route_candidates"], [{"source": "stub", "model": "claude-opus-5-5"}])
        self.assertEqual(cfg["route_defaults"], {"claude_code": {"candidates": [{"source": "stub", "model": "claude-sonnet-5-5"}]}})
        # 全 task を claude_code へ（無いと CoS が起票した task が route されない）。
        self.assertEqual(cfg["harness_selection"], {
            "rules": [{"required_capabilities": [], "harness": "claude_code"}],
            "execution_class_defaults": {"coding": ["claude_code"], "planner": ["claude_code"],
                                         "review": ["claude_code"]}})
        # stub: harness id は claude_code のまま kind = fake、adapter は --stub-command。
        self.assertEqual(cfg["harnesses"], {"claude_code": {"kind": "fake", "api_formats": ["fake"], "source_kinds": ["fake"]}})
        self.assertEqual(cfg["adapters"], {"fake": {"command": [self.stub]}})
        self.assertEqual(cfg["llm_sources"], {"stub": {"kind": "fake", "accounts": ["stub"],
                                                       "models": ["claude-opus-5-5", "claude-sonnet-5-5"]}})
        # 見出しの release と binary の sha256。
        sha = hashlib.sha256(rdb(os.path.join(self.release, "bin", "celeris"))).hexdigest()
        self.assertIn(f"release 94f86928dc13（bin/celeris sha256 {sha}）", text)
        # state: token（600、64 hex）・skills の配送・案件の body。
        token = os.path.join(state, "token")
        self.assertEqual(stat.S_IMODE(os.stat(token).st_mode), 0o600)
        self.assertRegex(rd(token), r"^[0-9a-f]{64}\n$")
        kb_skills = os.path.join(state, "kb", "skills")
        self.assertEqual(sorted(os.listdir(kb_skills)), ["cos-inbox-triage", "cos-operator", "yomiyasu"])
        for rel in ("cos-operator/SKILL.md", "cos-operator/references/ops.md", "cos-inbox-triage/SKILL.md"):
            self.assertEqual(rd(os.path.join(kb_skills, rel)), rd(os.path.join(self.skills, rel)))
        body = json.loads(rd(os.path.join(state, "setup", "project.json")))
        self.assertEqual(sorted(body), ["request", "title"])
        for d in ("ws", "scratch"):
            self.assertTrue(os.path.isdir(os.path.join(state, d)))

    def test_trial_state_and_home(self):
        for trial in ("B-t001", "B-t002"):
            state = os.path.join(self.root, trial)
            r = self.run_script("--port", "17861", "--harness", "stub",
                                "--stub-command", self.stub, state=state)
            self.assertEqual(r.returncode, 0, r.stderr)
            self.assertIn(f"state={state}\nHOME={state}/home\n", r.stdout)
            self.assertTrue(os.path.isdir(os.path.join(state, "home")))
            cfg = tomllib.loads(rd(os.path.join(self.root, "cfg", "daemon.toml")))
            self.assertEqual(cfg["api"]["tokens"], [
                {"principal": "operator:ops-r1", "token_file": state + "/token"}])

    def test_real_config_all_sonnet_with_accounts_and_clusters(self):
        normal = os.path.join(self.root, "normal.toml")
        wr(normal, 'db = "x"\n\n[[clusters]]\nid = "pegasus"\nhost = "pegasus"\n\n[[clusters.forwards]]\n'
                   'listen = "127.0.0.1:18000"\ntarget = "10.0.0.1:18000"\n\n[api]\nlisten = "127.0.0.1:7721"\n\n'
                   '[[clusters]]\nid = "fern03"\nhost = "fern03"\n')
        accounts = os.path.join(self.root, "accounts")
        r = self.run_script("--port", "17862", "--claude-bin", "/opt/claude", "--claude-accounts", accounts,
                            "--clusters-from", normal, models="all-sonnet")
        self.assertEqual(r.returncode, 0, r.stderr)
        cfg = tomllib.loads(rd(os.path.join(self.root, "cfg", "daemon.toml")))
        self.assertEqual(cfg["cos"]["route_candidates"], [{"source": "claude_oauth", "model": "claude-sonnet-5-5"}])
        self.assertEqual(cfg["route_defaults"]["claude_code"]["candidates"],
                         [{"source": "claude_oauth", "model": "claude-sonnet-5-5"}])
        self.assertEqual(cfg["harnesses"]["claude_code"]["kind"], "claude_code")
        self.assertEqual(cfg["harnesses"]["claude_code"]["source_kinds"], ["claude_oauth"])
        self.assertEqual(cfg["adapters"], {"claude_code": {"command": "/opt/claude"}})
        self.assertEqual(cfg["llm_sources"]["claude_oauth"]["models"], ["claude-sonnet-5-5"])
        self.assertEqual(cfg["accounts"], {"claude_dir": accounts, "max_runs_per_account": 1})
        self.assertEqual(cfg["clusters"], tomllib.loads(rd(normal))["clusters"])
        self.assertEqual(cfg["api"]["listen"], "127.0.0.1:17862")

    def test_free_port_when_omitted(self):
        r = self.run_script("--harness", "stub", "--stub-command", self.stub)
        self.assertEqual(r.returncode, 0, r.stderr)
        cfg = tomllib.loads(rd(os.path.join(self.root, "cfg", "daemon.toml")))
        host, port = cfg["api"]["listen"].rsplit(":", 1)
        self.assertEqual(host, "127.0.0.1")
        self.assertTrue(1024 <= int(port) <= 65535)

    def test_same_input_same_bytes(self):
        out = self.gen_stub()
        token = rd(os.path.join(self.root, "state", "token"))
        first = rdb(out)
        self.gen_stub()
        self.assertEqual(rdb(out), first)
        self.assertEqual(rd(os.path.join(self.root, "state", "token")), token)  # 既存の token は保つ

    def assert_refused(self, r, needle):
        self.assertEqual(r.returncode, 2, r.stderr)
        self.assertIn(needle, r.stderr)

    def test_refuse_state_under_config_celeris(self):
        state = os.path.join(self.home, ".config", "celeris", "measure")
        r = self.run_script("--harness", "stub", "--stub-command", self.stub, state=state)
        self.assert_refused(r, "production path")
        self.assertFalse(os.path.exists(os.path.join(self.home, ".config")))
        self.assertFalse(os.path.exists(os.path.join(self.root, "cfg")))

    def test_refuse_out_under_local_celeris_via_symlink(self):
        link = os.path.join(self.root, "link")
        os.symlink(os.path.join(self.home, ".local", "celeris"), link)
        r = self.run_script("--harness", "stub", "--stub-command", self.stub, out=os.path.join(link, "daemon.toml"))
        self.assert_refused(r, "production path")
        self.assertFalse(os.path.exists(os.path.join(self.root, "state")))

    def test_refuse_out_under_production_state(self):
        r = self.run_script("--harness", "stub", "--stub-command", self.stub,
                            out="/local/celeris/state/ops-r1-test-never-written/daemon.toml")
        self.assert_refused(r, "production path")
        self.assertFalse(os.path.exists("/local/celeris/state/ops-r1-test-never-written"))
        self.assertFalse(os.path.exists(os.path.join(self.root, "state")))

    def test_refuse_unknown_models(self):
        r = self.run_script("--harness", "stub", "--stub-command", self.stub, models="opus-everywhere")
        self.assert_refused(r, "unknown --models")
        self.assertFalse(os.path.exists(os.path.join(self.root, "state")))

    def test_usage_and_input_errors(self):
        self.assertEqual(self.run_script(base=False).returncode, 2)
        self.assertEqual(self.run_script("--harness", "stub").returncode, 2)  # --stub-command が無い
        os.remove(os.path.join(self.release, "bin", "celeris"))
        r = self.run_script("--harness", "stub", "--stub-command", self.stub)
        self.assertEqual(r.returncode, 1, r.stderr)
        self.assertIn("bin/celeris", r.stderr)


if __name__ == "__main__":
    unittest.main()
