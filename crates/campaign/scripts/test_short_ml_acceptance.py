"""verify-short-ml-case.py --acceptance の決定的 test（標準ライブラリのみ・LLM / network / 実 job なし）。
実行: python3 -m unittest crates/campaign/scripts/test_short_ml_acceptance.py

ここで組む fixture はすべて synthetic（origin=synthetic）で、本物の外部証拠（ml-case-reassessment.json）を
adopted / deferred に書き換えない。時計注入と synthetic 証拠は test 専用で、CLI は実時計と実証拠だけを受ける。
通常 grader は celeris-campaign の実 binary（CELERIS_CAMPAIGN_BIN / $CARGO_TARGET_DIR/debug）を子 process で実行する。
"""
import datetime
import hashlib
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCRIPT = HERE / "verify-short-ml-case.py"
spec = importlib.util.spec_from_file_location("vsmc", SCRIPT)
vsmc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(vsmc)

CAMPAIGN = vsmc.CAMPAIGN
CASE = vsmc.CASE_ID
GRADER = vsmc.find_grader(None)
UTC = datetime.timezone.utc
CP = datetime.datetime(2026, 10, 22, 0, 0, 0, tzinfo=UTC)
SEC = datetime.timedelta(seconds=1)
REAL_EVIDENCE = "/local/celeris/data/workspaces/01M59KY5FTRJTJ80PACNW3K533/artifacts/ml-case-reassessment.json"


def sha(b):
    return hashlib.sha256(b).hexdigest()


def iso(t):
    return t.strftime("%Y-%m-%dT%H:%M:%SZ")


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = Path(self.tmp.name)
        self.campaign = self.dir / "campaign"
        for rel in (f"cases/dev/{CASE}", f"fixtures/inputs/{CASE}", f"fixtures/grade-cases/dev/{CASE}",
                    f"fixtures/grader-private/dev/{CASE}"):
            shutil.copytree(CAMPAIGN / rel, self.campaign / rel)
        self.status_path = self.campaign / "optional" / CASE / "status.json"
        self.status_path.parent.mkdir(parents=True)
        (self.dir / "re.json").write_text(json.dumps({"withdrawn_decisions": [{"saved": {"status_json": {k: ["x"] for k in vsmc.SAVED_KEYS}}}]}))
        (self.dir / "rv.md").write_text("## 撤回した判断\n")

    def write_status(self, status, **over):
        s = {
            "schema": vsmc.SCHEMA, "task_id": vsmc.TASK_ID, "status": status, "label": "dev",
            "claims": {"final_generalization": False, "real_pilot_confirmed": False, "adopted": status == "adopted"},
            "external_refs": {"reassessment": str(self.dir / "re.json"), "review": str(self.dir / "rv.md")},
        }
        if status == "pending":
            s["confirm_at"] = "2026-10-22T00:00:00Z"
        else:
            s["decided_at"] = "2026-10-22"
        if status == "adopted":
            s["case_dir"] = CASE
        s.update(over)
        self.status_path.write_text(json.dumps(s))

    def acc(self, ev, **kw):
        p = self.dir / "evidence.json"
        p.write_text(json.dumps(ev))
        return vsmc.acceptance(p, self.status_path, self.campaign, **kw)


class Deferred(Base):
    def setUp(self):
        super().setUp()
        shutil.rmtree(self.campaign / "cases" / "dev" / CASE)  # deferred は case を置かない
        self.write_status("deferred", decided_at="2026-10-23")
        self.out = self.dir / "recheck.out"
        self.out.write_text("verify exit 1: no live job record\n")

    def ev(self, **rc):
        r = {"at": iso(CP + 3600 * SEC), "command": "python3 verify-short-ml-case.py --acceptance", "exit": 1,
             "result": "not_established", "output_path": str(self.out), "output_sha256": sha(self.out.read_bytes())}
        r.update(rc)
        return {"origin": "synthetic", "status": "deferred", "deadline": {"checkpoint": vsmc.CHECKPOINT},
                "deferral": {"recheck": r}}

    def run_at(self, now, **rc):
        return self.acc(self.ev(**rc), now=now, allow_synthetic=True)[0]

    def test_boundary_just_before_rejected(self):
        self.assertTrue(any("before the checkpoint" in e for e in self.run_at(CP - SEC)))

    def test_boundary_exact_accepted(self):
        self.assertEqual(self.run_at(CP, at=iso(CP)), [])

    def test_boundary_just_after_accepted(self):
        self.assertEqual(self.run_at(CP + SEC, at=iso(CP + SEC)), [])

    def test_future_injected_clock_ok_but_never_production(self):
        # 未来時刻の注入は synthetic でだけ通る。実証拠・CLI では使えない。
        self.assertEqual(self.run_at(CP + 400 * 24 * 3600 * SEC), [])
        real = self.ev()
        del real["origin"]
        self.assertTrue(self.acc(real, now=CP + 3600 * SEC, allow_synthetic=True)[0])
        self.assertTrue(self.acc(real, now=CP + 3600 * SEC)[0])
        self.assertTrue(any("synthetic" in e for e in self.acc(self.ev())[0]))

    def test_recorded_before_deadline_rejected(self):
        self.assertTrue(any("before the checkpoint" in e for e in self.run_at(CP + 9 * 3600 * SEC, at=iso(CP - SEC))))

    def test_recheck_in_the_future_rejected(self):
        self.assertTrue(any("future" in e for e in self.run_at(CP + 60 * SEC, at=iso(CP + 120 * SEC))))

    def test_exit_zero_or_wrong_result_rejected(self):
        self.assertTrue(self.run_at(CP + 3600 * SEC, exit=0))
        self.assertTrue(self.run_at(CP + 3600 * SEC, result="established"))

    def test_output_tamper_and_missing_rejected(self):
        ev = self.ev()
        self.out.write_text("edited\n")
        self.assertTrue(any("hash" in e for e in self.acc(ev, now=CP + 3600 * SEC, allow_synthetic=True)[0]))
        self.out.unlink()
        self.assertTrue(self.acc(ev, now=CP + 3600 * SEC, allow_synthetic=True)[0])

    def test_checkpoint_and_status_must_agree(self):
        ev = self.ev()
        ev["deadline"]["checkpoint"] = "2026-10-21T00:00:00Z"
        self.assertTrue(self.acc(ev, now=CP + 3600 * SEC, allow_synthetic=True)[0])
        self.write_status("pending")
        self.assertTrue(self.acc(self.ev(), now=CP + 3600 * SEC, allow_synthetic=True)[0])


def build_artifact(root, job="4242.sirius"):
    """synthetic な正例 artifact と評価側 observed-jobs（固定応答）。"""
    art = root / "artifact"
    for d in ("inputs", "results", "evidence"):
        (art / d).mkdir(parents=True)
    shutil.copy(CAMPAIGN / "fixtures/inputs" / CASE / "train.csv", art / "inputs/train.csv")
    ref = json.loads((CAMPAIGN / "fixtures/grader-private/dev" / CASE / "reference.json").read_text())
    rows = (art / "inputs/train.csv").read_text().splitlines()[1:]
    ymean = sum(float(r.split(",")[-1]) for r in rows) / len(rows)
    coefs = "name,value\n" + "".join(f"{k},{v}\n" for k, v in ref["coef"].items()) + f"intercept,{ref['intercept']}\n"
    (art / "results/coefficients.csv").write_text(coefs)
    (art / "results/metrics.csv").write_text(f"name,value\ntrain_rmse,{ref['train_rmse']}\nn_train_rows,{len(rows)}\n")
    (art / "results/predictions.csv").write_text("id,pred\n0,1.0\n")
    m = {"n_train_rows": len(rows), "train_y_mean": ymean, "intercept": ref["intercept"], "train_rmse": ref["train_rmse"]}
    m.update({f"coef_{k}": v for k, v in ref["coef"].items()})
    (art / "report.md").write_text("# synthetic report\n" + "".join(f"metric {k} = {v}\n" for k, v in m.items()))
    raw = [{"path": p, "sha256": sha((art / p).read_bytes())} for p in
           ("results/coefficients.csv", "results/metrics.csv", "results/predictions.csv")]
    (art / f"evidence/{job}.qstat").write_text(f"Job Id: {job}\n Exit_status = 0\n")
    (art / f"evidence/{job}.log").write_text("done\n")
    (art / "evidence/jobs.json").write_text(json.dumps({"jobs": [{
        "job_id": job, "evidence_kind": "real", "scheduler_record": f"evidence/{job}.qstat",
        "log": f"evidence/{job}.log", "raw": raw, "conditions": {"queue": "debug", "ncpus": 1}}]}))
    obs = {"schema": "celeris.observed-jobs/1", "origin": "synthetic", "jobs": [{
        "job_id": job, "scheduler": "pbs", "cluster": "sirius", "source": "sirius", "exit_status": 0,
        "conditions": {"queue": "debug", "ncpus": 1}, "raw": raw, "scheduler_record_sha256": sha(b"qstat")}]}
    (root / "observed.json").write_text(json.dumps(obs))
    return art, root / "observed.json"


@unittest.skipUnless(GRADER, "celeris-campaign binary not built (cargo build -p celeris-campaign)")
class Adopted(Base):
    def setUp(self):
        super().setUp()
        self.write_status("adopted")
        self.art, self.obs = build_artifact(self.dir)
        self.ap = {"approved": True, "remaining": {"tokens": 2512372, "as_of": "2026-10-22T00:00:00Z", "source": "ledger"}}
        self.em = {"command": "ssh sirius python3 -c ...", "exit": 0, "python": "3.9.25", "numpy": "1.23.5"}

    def ev(self, **ad):
        a = {"artifact": str(self.art), "observed_jobs": str(self.obs), "env_measured": self.em, "approval": self.ap}
        a.update(ad)
        return {"origin": "synthetic", "status": "adopted", "adoption": a}

    def run_acc(self, ev=None):
        return self.acc(ev or self.ev(), grader_bin=GRADER, now=CP, allow_synthetic=True)

    def test_adopted_passes_with_four_real_grader_runs(self):
        errs, runs = self.run_acc()
        self.assertEqual(errs, [])
        self.assertEqual([(r["name"], r["exit"] == 0) for r in runs],
                         [("positive", True), ("miscalc", False), ("wrong-condition", False), ("unexecuted", False)])
        for r in runs:
            self.assertEqual(r["cmd"][1], "grade")
            self.assertEqual(len(r["output_sha256"]), 64)

    def test_negatives_are_rejected_for_their_own_intended_reason(self):
        trials = vsmc.build_trials(self.art, self.obs, self.dir / "direct")
        gcase = self.campaign / "fixtures/grade-cases/dev" / CASE / f"{CASE}.json"
        outs = set()
        for name, bad, good, why in vsmc.NEGATIVES:
            t = trials[name]
            out = vsmc.run_grader(GRADER, gcase, t / "artifact", t / "observed.json", t / "work")
            self.assertNotEqual(out["exit"], 0, name)
            st, reasons, _ = vsmc.check_status(out["report"], bad)
            self.assertEqual(st, "fail", name)
            self.assertTrue(any(why in r for r in reasons), (name, reasons))
            self.assertEqual(vsmc.check_status(out["report"], good)[0], "pass", name)
            outs.add(out["output_sha256"])
        self.assertEqual(len(outs), 3)

    def test_negatives_are_independent_single_point_changes(self):
        trials = vsmc.build_trials(self.art, self.obs, self.dir / "ind")
        pos = vsmc.tree_files(trials["positive"] / "artifact")
        self.assertEqual(vsmc.tree_files(trials["wrong-condition"] / "artifact"), pos)
        self.assertEqual(vsmc.tree_files(trials["unexecuted"] / "artifact"), pos)
        mc = vsmc.tree_files(trials["miscalc"] / "artifact")
        self.assertEqual({k for k in pos if pos[k] != mc[k]}, {"report.md"})
        self.assertNotEqual(len({str(trials[n]) for n in trials}), 1)

    def test_synthetic_observation_is_not_production(self):
        errs, _ = self.acc(self.ev(), grader_bin=GRADER)
        self.assertTrue(any("synthetic" in e for e in errs))
        ev = self.ev()
        del ev["origin"]  # 実証拠を名乗るのに観測が synthetic
        self.assertTrue(any("origin" in e for e in self.acc(ev, grader_bin=GRADER)[0]))

    def tamper_rejected(self, rel, fn):
        p = self.campaign / rel
        orig = p.read_bytes()
        p.write_bytes(fn(orig))
        errs, _ = self.run_acc()
        p.write_bytes(orig)
        self.assertTrue(errs, rel)
        return errs

    def test_tamper_case_input_lock_grader_rejected(self):
        for rel in (f"cases/dev/{CASE}/case.json", f"cases/dev/{CASE}/env.lock.json", f"cases/dev/{CASE}/materials/job.py",
                    f"cases/dev/{CASE}/kb/snapshot.md", f"fixtures/inputs/{CASE}/train.csv",
                    f"fixtures/inputs/{CASE}/eval-features.csv",
                    f"fixtures/grade-cases/dev/{CASE}/{CASE}.json",
                    f"fixtures/grader-private/dev/{CASE}/private.json",
                    f"fixtures/grader-private/dev/{CASE}/eval-targets.csv"):
            self.tamper_rejected(rel, lambda b: b + b" ")

    def test_tamper_contract_pin_rejected(self):
        self.tamper_rejected(f"cases/dev/{CASE}/contract.json", lambda b: b.replace(b"123c9b3f", b"123c9b3e", 1))

    def test_tamper_raw_and_evidence_rejected(self):
        p = self.art / "results/coefficients.csv"
        b = p.read_bytes()
        p.write_bytes(b.replace(b"x1,1", b"x1,2"))
        self.assertTrue(any("raw" in e for e in self.run_acc()[0]))
        p.write_bytes(b)
        o = json.loads(self.obs.read_text())
        o["jobs"][0]["raw"][0]["sha256"] = "0" * 64
        self.obs.write_text(json.dumps(o))
        self.assertTrue(self.run_acc()[0])

    def test_unobserved_or_unexecuted_runner_rejected(self):
        o = json.loads(self.obs.read_text())
        o["jobs"] = []
        self.obs.write_text(json.dumps(o))
        self.assertTrue(any("no job" in e or "not executed" in e for e in self.run_acc()[0]))
        o["jobs"] = [{"job_id": "1.sirius", "exit_status": 1, "raw": []}]
        self.obs.write_text(json.dumps(o))
        self.assertTrue(any("exit_status" in e for e in self.run_acc()[0]))
        self.obs.unlink()
        self.assertTrue(self.run_acc()[0])

    def test_runner_that_accepts_everything_is_rejected(self):
        fake = self.dir / "fake-grader"
        fake.write_text('#!/bin/sh\necho \'{"final_pass": true}\'\n')
        fake.chmod(0o755)
        errs, _ = self.acc(self.ev(), grader_bin=str(fake), now=CP, allow_synthetic=True)
        self.assertTrue(any("must be rejected" in e for e in errs))

    def test_runner_that_rejects_everything_is_rejected(self):
        fake = self.dir / "fake-grader"
        fake.write_text("#!/bin/sh\nexit 1\n")
        fake.chmod(0o755)
        errs, _ = self.acc(self.ev(), grader_bin=str(fake), now=CP, allow_synthetic=True)
        self.assertTrue(any("positive run was not accepted" in e for e in errs))
        self.assertTrue(any("intended reason" in e for e in errs))

    def test_claimed_runs_must_match_reexecution(self):
        bad = [{"name": "positive", "exit": 0}, {"name": "miscalc", "exit": 0}]
        self.assertTrue(any("claimed runs" in e for e in self.run_acc(self.ev(runs=bad))[0]))

    def test_missing_approval_env_or_stale_balance_rejected(self):
        for bad in ({"approval": None}, {"approval": {"approved": False, "remaining": self.ap["remaining"]}},
                    {"approval": {"approved": True, "remaining": {"tokens": 2512372, "source": "survey", "as_of": "2026-10-17T00:00:00Z"}}},
                    {"approval": {"approved": True}},
                    {"env_measured": None}, {"env_measured": {**self.em, "numpy": "1.26.0"}},
                    {"env_measured": {**self.em, "exit": 1}}):
            self.assertTrue(self.run_acc(self.ev(**bad))[0], bad)

    def test_status_and_evidence_must_agree(self):
        self.write_status("deferred")
        self.assertTrue(self.run_acc()[0])
        self.write_status("pending")
        self.assertTrue(self.run_acc()[0])


class PendingAndCli(Base):
    def cli(self, *a):
        return subprocess.run([sys.executable, str(SCRIPT), *a], capture_output=True, text=True)

    def test_pending_status_is_never_accepted(self):
        self.write_status("pending")
        for st in ("pending", "unverified", None):
            errs, _ = self.acc({"status": st})
            self.assertTrue(errs, st)

    def test_current_repo_state_is_pending_and_nonzero_while_check_passes(self):
        r = self.cli("--check")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(self.cli("--placement-check").returncode, 0)
        if json.loads(vsmc.STATUS_PATH.read_text())["status"] == "pending":
            r = self.cli("--acceptance", "--evidence", REAL_EVIDENCE)
            self.assertEqual(r.returncode, 1, r.stderr)

    def test_cli_rejects_bad_arguments(self):
        for a in ([], ["--acceptance"], ["--acceptance", "--evidence"], ["--acceptance", "--evidence", "x", "--now", "2030-01-01"],
                  ["--acceptance", "--evidence", "a", "--evidence", "b"], ["--acceptance", "--check"],
                  ["--acceptance", "--evidence", "--grader-bin", "g"], ["--check", "--placement-check"],
                  ["--now", "2030-01-01T00:00:00Z"], ["--acceptance", "--evidence", "x", "extra"]):
            self.assertEqual(self.cli(*a).returncode, 2, a)

    def test_cli_unreadable_evidence_is_one_not_two(self):
        self.assertEqual(self.cli("--acceptance", "--evidence", str(self.dir / "none.json")).returncode, 1)

    def test_clock_injection_is_not_reachable_from_the_cli(self):
        self.assertNotIn("--now", SCRIPT.read_text().split("def main")[1])
        errs, _ = vsmc.acceptance(self.dir / "x.json", now=CP)
        self.assertTrue(any("test-only" in e for e in errs))

    def test_fixture_never_rewrites_the_real_evidence(self):
        if Path(REAL_EVIDENCE).is_file():
            self.assertNotIn(json.loads(Path(REAL_EVIDENCE).read_text()).get("status"), ("adopted_synthetic",))


if __name__ == "__main__":
    unittest.main()
