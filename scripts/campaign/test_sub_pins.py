"""check-sub-pins.py の offline 試験。実 artifacts の pinned/ を一時 root へ写して書き換える（network・LLM・daemon なし）。
実 artifacts が無い環境では skip。_SRC は小さい stub に差し替え、公開側 tree の走査を軽くする。"""
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "check-sub-pins.py")
REAL = os.environ.get("SUB_PINS_ARTIFACTS", "/local/celeris/data/workspaces/01M58VFRBA6HG0SSF3CG5BKM91/artifacts")
spec = importlib.util.spec_from_file_location("check_sub_pins", SCRIPT)
csp = importlib.util.module_from_spec(spec)
spec.loader.exec_module(csp)
BIN_REL = csp.CAMPAIGN_BIN_REL


def rd(p):
    with open(p, encoding="utf-8") as f:
        return f.read()


def wr(p, s):
    with open(p, "w", encoding="utf-8") as f:
        f.write(s)


def have_real():
    return os.path.isfile(os.path.join(REAL, BIN_REL)) and os.path.isdir(os.path.join(REAL, "pinned", "sub", "fixed"))


def run(root):
    r = subprocess.run([sys.executable, SCRIPT, "--root", root], capture_output=True, text=True)
    return r.returncode, r.stderr


@unittest.skipUnless(have_real(), "real artifacts not present")
class SubPins(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.root = os.path.join(self.tmp, "artifacts")
        os.makedirs(os.path.join(self.root, "pinned"))
        for d in ("sub", "main"):
            shutil.copytree(os.path.join(REAL, "pinned", d), os.path.join(self.root, "pinned", d), symlinks=True)
        shutil.copy(os.path.join(REAL, "pinned", "cap-env.json"), os.path.join(self.root, "pinned"))
        os.makedirs(os.path.dirname(os.path.join(self.root, BIN_REL)))
        shutil.copy2(os.path.join(REAL, BIN_REL), os.path.join(self.root, BIN_REL))
        shutil.copy(os.path.join(REAL, "measure-pinned-sub-report.md"), self.root)
        for k in ("materials", "kb", "repo", "inputs"):
            os.makedirs(os.path.join(self.root, "measure-src", "main", k))
            wr(os.path.join(self.root, "measure-src", "main", k, "f.txt"), "public stub " + k)
        for d in ("sub", "main"):  # 実 root の絶対 path を一時 root へ
            for name in os.listdir(os.path.join(self.root, "pinned", d)):
                p = os.path.join(self.root, "pinned", d, name)
                if os.path.isfile(p) and name != "env-lock.rehearsal.json" or name == "daemon.toml":
                    try:
                        t = rd(p)
                    except UnicodeDecodeError:
                        continue
                    wr(p, t.replace(REAL, self.root))
        # 書き換えた file の hash 表（報告書）を揃える
        self.refresh_report()
        self.sub = os.path.join(self.root, "pinned", "sub")

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def refresh_report(self):
        import re
        p = os.path.join(self.root, "measure-pinned-sub-report.md")
        t = rd(p)
        for n in ("final.json", "daemon.toml", "launcher.env", "canaries.json", "task-template.json", "env-lock.rehearsal.json"):
            h = csp.sha256(csp.Path(self.root, "pinned", "sub", n))
            t = re.sub(rf"(\|\s*`?(?:pinned/sub/)?{re.escape(n)}`?\s*\|\s*)[0-9a-f]{{64}}", rf"\g<1>{h}", t)
        wr(p, t)

    def edit(self, name, fn):
        p = os.path.join(self.sub, name)
        t = rd(p)
        n = fn(t)
        self.assertNotEqual(t, n)
        wr(p, n)
        self.refresh_report()

    def problems(self):
        code, err = run(self.root)
        return code, err

    def only_gaps(self, err):
        return [l for l in err.splitlines() if l.startswith("FAIL:") and "GAP[" not in l]

    def test_baseline_has_only_handover_gaps(self):
        code, err = self.problems()
        self.assertEqual(code, 1)
        self.assertEqual(self.only_gaps(err), [], err)
        for k in ("sub-project-id", "sub-preflight", "sub-campaign-lock"):
            self.assertIn(f"GAP[{k}]", err)

    def test_project_id_only_difference_passes_common_comparison(self):
        def setid(t):
            d = json.loads(t)
            d["project_id"] = "01MXXXXXXXXXXXXXXXXXXXXXXX"
            return json.dumps(d)
        self.edit("task-template.json", setid)
        code, err = self.problems()
        self.assertNotIn("GAP[sub-project-id]", err)
        self.assertNotIn("differs from main in fields other than project_id", err)

    def test_stale_or_main_project_id_is_not_valid(self):
        for pid in (csp.OLD_SUB_PROJECT_ID, json.loads(rd(os.path.join(self.root, "pinned", "main", "task-template.json")))["project_id"]):
            def setid(t, pid=pid):
                d = json.loads(t)
                d["project_id"] = pid
                return json.dumps(d)
            self.edit("task-template.json", setid)
            self.assertIn("GAP[sub-project-id]", self.problems()[1])

    def test_other_task_template_field_rejected(self):
        def f(t):
            d = json.loads(t)
            d["repos"] = ["other"]
            return json.dumps(d)
        self.edit("task-template.json", f)
        self.assertIn("fields other than project_id", self.problems()[1])

    def test_evidence_and_lock_present_clear_their_gaps(self):
        os.makedirs(os.path.join(self.sub, "evidence"))
        wr(os.path.join(self.sub, "evidence", "k2-preflight.json"), json.dumps({"ok": True, "paid_calls_made": 0}))
        wr(os.path.join(self.sub, "lock.json"), json.dumps({"campaign_id": "mlsys-sub-1"}))
        err = self.problems()[1]
        self.assertNotIn("GAP[sub-preflight]", err)
        self.assertNotIn("GAP[sub-campaign-lock]", err)

    def test_failed_evidence_rejected(self):
        os.makedirs(os.path.join(self.sub, "evidence"))
        wr(os.path.join(self.sub, "evidence", "k2-preflight.json"), json.dumps({"ok": False, "paid_calls_made": 0}))
        self.assertIn("ok:true", self.problems()[1])

    def test_old_binary_rejected(self):
        self.edit("launcher.env", lambda t: t.replace(BIN_REL, "pinned/bin/celeris-campaign"))
        self.assertIn("common fixed binary", self.problems()[1])

    def test_binary_hash_regression_rejected(self):
        with open(os.path.join(self.root, BIN_REL), "ab") as f:
            f.write(b"x")
        self.assertIn("sha256", self.problems()[1])

    def test_wu_path_rejected(self):
        self.edit("launcher.env", lambda t: t.replace(self.root + "/pinned/sub/fixed/cases", self.root + "/wu/pin-values/cases"))
        self.assertIn("/wu/", self.problems()[1])

    def test_fixed_content_drift_rejected(self):
        p = os.path.join(self.sub, "fixed", "cases", "final")
        k = os.path.join(p, sorted(os.listdir(p))[0], "case.json")
        with open(k, "a") as f:
            f.write(" ")
        self.assertIn("content differs from pinned/main/fixed/cases", self.problems()[1])

    def test_private_grader_leak_into_public_rejected(self):
        gp = os.path.join(self.sub, "fixed", "grader-private", "final")
        priv = os.path.join(gp, sorted(os.listdir(gp))[0], "private.json")
        shutil.copy(priv, os.path.join(self.root, "measure-src", "main", "kb", "leak.txt"))
        self.assertIn("isolation: private grader file leaked", self.problems()[1])

    def test_public_symlink_into_private_rejected(self):
        gp = os.path.join(self.sub, "fixed", "grader-private")
        os.symlink(gp, os.path.join(self.root, "measure-src", "main", "kb", "link"))
        self.assertIn("symlink resolves into private grader", self.problems()[1])

    def test_missing_fixed_dir_rejected(self):
        shutil.rmtree(os.path.join(self.sub, "fixed", "grade-cases"))
        self.assertIn("fixed dir missing", self.problems()[1])


if __name__ == "__main__":
    unittest.main()
