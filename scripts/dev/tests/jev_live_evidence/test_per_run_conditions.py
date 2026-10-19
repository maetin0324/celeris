"""root/子/孫/retry/B の各 trial を凍結 manifest の共通候補・実効 pin に独立照合する試験。

読取元 DB（原記録）を改変し、export はその改変後の原記録から投影し直す（原記録と export の双方が整合した改変）。
よって出自・照合エラーでは落ちず、候補/条件の検証だけが落とす（[candidates]/[pin]/[condition-mismatch] 以外の
コードが出ないことも確かめる）。builder は test double（合成）で、実 daemon の証明ではない。
"""
import json
import os
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import builder as b  # noqa: E402
import contextlib  # noqa: E402
import importlib.util  # noqa: E402
import io  # noqa: E402
from test_jev_live_evidence import Base  # noqa: E402

R = b.RUNS
SONNET, OPUS = b.CANDS
ALLOWED = ("FAIL [candidates]", "FAIL [pin]", "FAIL [condition-mismatch]", "FAIL [exec-model]")
FAIL_CODES = ("candidates", "pin", "condition-mismatch")


_spec = importlib.util.spec_from_file_location("jev_live_evidence_cli", os.path.join(HERE, "..", "..", "jev-live-evidence.py"))
cli = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(cli)


def set_cands(s, run, cands, exec_model="claude-sonnet-5-5"):
    first = cands[0] if cands else {}
    s.c.execute(
        "UPDATE decision_log_runs SET proposed_candidates_json = ?, proposed_first_source_id = ?, proposed_first_model = ?, "
        "proposed_first_harness = ?, exec_model = ? WHERE run_id = ?",
        (json.dumps(cands) if cands else None, first.get("source_id"), first.get("model_id"), first.get("harness_id"),
         exec_model, run))
    s.c.execute("UPDATE runs SET model = ? WHERE run_id = ?", (exec_model, run))


class InProcess(Base):
    """パラメータ化の負例が多いので CLI の main を同一 process で呼ぶ（引数・環境は run_cli と同じ）。"""

    def run_cli(self, env=None, live=True, quiet=True):
        args = ["--evidence-dir", self.ev] + (["--quiet"] if quiet else []) + (["--require-live"] if live else [])
        old = dict(os.environ)
        for k in (b.reader.ENV_SOURCE, "CELERIS_DB", "CELERIS_RUN_DB"):
            os.environ.pop(k, None)
        os.environ[b.reader.ENV_SOURCE] = self.db
        out, err = io.StringIO(), io.StringIO()
        try:
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                code = cli.main(args)
        finally:
            os.environ.clear()
            os.environ.update(old)
        return code, out.getvalue(), err.getvalue()


class PerRunCandidates(InProcess):
    def reject(self, mutate, code="candidates"):
        s = self.source()
        mutate(s)
        s.close()
        err = self.check(1, f"[{code}]")
        for line in err.splitlines():
            if line.startswith("FAIL ["):
                self.assertTrue(line.startswith(ALLOWED), f"出自/照合エラーが混ざった（候補検証の証拠にならない）: {line}")
        return err

    def test_reviewer_repro_c1_r2_single_sonnet_exec_sonnet(self):
        """reviewer 再現: C1 子 r2 だけ Sonnet 1 件かつ exec_model=Sonnet。arm 和集合では Opus が残り通っていた。"""
        err = self.reject(lambda s: set_cands(s, R["r2"], [SONNET]))
        self.assertIn(R["r2"], err)
        self.assertNotIn(f"trial {R['r1']}", err)  # root/他 run は正しい。落ちた理由は r2 の候補不一致

    def test_each_depth_retry_and_b_side_single_sonnet(self):
        for key in ("r1", "r2", "r3", "r4", "rb"):
            with self.subTest(run=key):
                self.reject(lambda s, k=key: set_cands(s, R[k], [SONNET]))

    def test_candidate_variants_on_every_run(self):
        variants = {
            "empty": [],
            "single-opus": [OPUS],
            "other-harness": [SONNET, dict(OPUS, harness_id="codex")],
            "other-source": [SONNET, dict(OPUS, source_id="openrouter")],
            "unknown-id": [SONNET, dict(OPUS, model_id="claude-opus-9-9")],
            "duplicate": [SONNET, SONNET],
            "extra": [SONNET, OPUS, dict(OPUS, model_id="claude-haiku-5-5")],
            "name-substring-only": [dict(SONNET, model_id="my-sonnet-clone"), dict(OPUS, model_id="x-opus-y")],
        }
        for key in ("r1", "r2", "r3", "r4", "rb"):
            for name, cands in variants.items():
                with self.subTest(run=key, variant=name):
                    self.reject(lambda s, k=key, c=cands: set_cands(s, R[k], c))

    def test_only_root_correct_descendant_retry_wrong(self):
        s = self.source()
        set_cands(s, R["r4"], [SONNET])
        s.close()
        err = self.check(1, "[candidates]")
        self.assertIn(R["r4"], err)

    def test_empty_everywhere_is_not_silently_accepted(self):
        def m(s):
            for k in R:
                set_cands(s, R[k], [])
        self.reject(m)

    def test_export_only_tamper_still_fails_by_reconcile(self):
        s = self.source()
        s.close()
        ex = b.export_of(self.db)
        for r in ex["records"]:
            if r.get("record_type") == "trial" and r["run_id"] == R["r2"]:
                r["proposed_candidates"] = [SONNET]
        self.check(1, "records.jsonl", ex=ex)

    def test_baseline_still_passes(self):
        self.source().close()
        self.check(0)


class PerRunPin(InProcess):
    def edit_event(self, key, arm, **over):
        res = dict(b.RESOLVED if arm == "C" else b.RESOLVED_B, **over)
        s = self.source()
        s.c.execute("UPDATE events SET json = ? WHERE json LIKE ?",
                    (json.dumps(b.event_body(R[key], arm, resolved=res)), f'%"{R[key]}"%'))
        s.close()

    def test_effective_pin_fields_on_each_run(self):
        cases = {"source": {"source": "config"}, "online": {"online_update": True},
                 "evidence-mode": {"evidence_mode": "pinned"}, "table": {"evidence_table_id": "t1"},
                 "lower": {"lower_policy": "other"}}
        for key, arm in (("r1", "C"), ("r2", "C"), ("r3", "C"), ("r4", "C"), ("rb", "B")):
            for name, over in cases.items():
                with self.subTest(run=key, case=name):
                    self.tmp.cleanup()
                    self.setUp()
                    self.edit_event(key, arm, **over)
                    err = self.check(1, "[condition-mismatch]" if (key, name) == ("rb", "lower") else "[pin]")
                    for line in err.splitlines():
                        if line.startswith("FAIL ["):
                            self.assertTrue(line.startswith(ALLOWED), line)

    def test_b_and_c_common_conditions_differ_beyond_mode(self):
        """B 側だけ下段方策が違うと、root 内の整合を保っても B/C 共通条件の不一致で落ちる。"""
        self.edit_event("rb", "B", lower_policy="other")
        self.check(1, "[condition-mismatch]")

    def test_missing_resolution_is_not_tolerated(self):
        s = self.source()
        body = b.event_body(R["r3"], "C")
        del body["record"]["features"]["decision"]["routing_resolved"]
        s.c.execute("UPDATE events SET json = ? WHERE json LIKE ?", (json.dumps(body), f'%"{R["r3"]}"%'))
        s.close()
        self.check(1, "[pin]")


if __name__ == "__main__":
    unittest.main()
