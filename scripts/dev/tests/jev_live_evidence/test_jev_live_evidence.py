"""jev-live-evidence.py の限定試験（原記録との全件照合 → 段階判定）。

読取元 DB と evidence は builder の test double（合成）。正例は「実 export 形式への変換と照合アルゴリズムの正例」で
あり、実 daemon の原記録を証明するものではない。
"""
import json
import os
import sqlite3
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "..", "..", "jev-live-evidence.py")
sys.path.insert(0, HERE)
import builder as b  # noqa: E402

R = b.RUNS


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        os.makedirs(os.path.join(self.tmp.name, "measure"))
        self.db = os.path.join(self.tmp.name, "measure", "celeris.db")
        self.ev = os.path.join(self.tmp.name, "ev")

    def source(self, **kw):
        return b.default_source(self.db, **kw)

    def run_cli(self, env=None, live=True, quiet=True):
        full = {k: v for k, v in os.environ.items() if k not in (b.reader.ENV_SOURCE, "CELERIS_DB", "CELERIS_RUN_DB")}
        full.update({b.reader.ENV_SOURCE: self.db} if env is None else env)
        args = [sys.executable, SCRIPT, "--evidence-dir", self.ev] + (["--quiet"] if quiet else []) + (
            ["--require-live"] if live else [])
        p = subprocess.run(args, capture_output=True, text=True, env=full)
        return p.returncode, p.stdout, p.stderr

    def check(self, want_code, *needles, ex=None, live=True, **kw):
        """source の現状から export を作り（ex で差し替え可）、CLI の終了コードと stderr の語を確かめる。"""
        b.write(ex if ex is not None else (getattr(self, "ex", None) or b.export_of(self.db)), self.ev, **kw)
        code, _, err = self.run_cli(live=live)
        self.assertEqual(code, want_code, err)
        for n in needles:
            self.assertIn(n, err)
        return err

    def report(self):
        b.write(b.export_of(self.db), self.ev)
        code, out, err = self.run_cli(quiet=False)
        return code, json.loads(out), err


class ConversionPositive(Base):
    """test double の正例: 変換・照合アルゴリズムが全件一致を合格にする（実 daemon の証明ではない）。"""

    def test_reconciled_export_passes_and_reports_stages(self):
        self.source().close()
        code, rep, err = self.report()
        self.assertEqual(code, 0, err)
        self.assertTrue(rep["reconciled"])
        c = rep["stages"]["by_arm"]["C"]
        self.assertEqual((c["trials"], c["jev_ok"], c["chain_ok"], c["final_lane_changed"], c["retries"]), (4, 4, 4, 4, 1))
        self.assertEqual(rep["stages"]["by_arm"]["B"]["jev_calls"], 0)
        # 全役割・retry・共有オーバーヘッドの費用は原記録の投影から（共有は by_role に入れない）。
        self.assertAlmostEqual(rep["costs"]["worker"]["sum"], 2.5)
        self.assertAlmostEqual(rep["costs"]["jev"]["sum"], 0.04)
        self.assertAlmostEqual(rep["costs"]["planner"]["sum"], 0.3)
        self.assertIsNone(rep["costs"]["cos"]["sum"])
        self.assertAlmostEqual(rep["shared_overhead"]["sum"], 0.2)
        # retry 区分は消費行の is_retry（run の retry の worker/reviewer）。Jev は call_no>1 だけが retry。
        self.assertAlmostEqual(rep["retry_cost"]["sum"], 0.6)
        self.assertEqual(rep["costs_cash"]["worker"]["n_missing"], 5)  # 実支出は欠測のまま（定価で埋めない）
        self.assertIn("cash_cost_usd", rep["missing"]["incomplete_fields"])
        self.assertTrue(any("LaneLimits" in u for u in rep["unverified"]))

    def test_projection_follows_the_export_contract(self):
        """Rust の build_campaign_export の規則のうち取り違えやすい点を手で書いた期待値で固定する。"""
        self.source().close()
        p = b.project(self.db)
        kinds = [r["record_type"] for r in p["records"]]
        self.assertEqual(kinds[:12], ["usage"] * 12)  # usage が先頭（dedup_key 順）
        self.assertEqual(kinds[-5:], ["trial"] * 5)  # trial は末尾
        keys = [r["dedup_key"] for r in p["records"] if r["record_type"] == "usage"]
        self.assertEqual(keys, sorted(keys))
        jev = next(r for r in p["records"] if r["record_type"] == "usage" and r["role"] == "jev")
        self.assertEqual(jev["usage_key"]["kind"], "call")
        self.assertTrue(jev["list_cost_usd_estimated"])
        r4 = next(r for r in p["records"] if r["record_type"] == "run" and r["run_id"] == R["r4"])
        self.assertEqual((r4["is_retry"], r4["parent_run_id"], r4["attempt_no"]), (True, R["r3"], 2))
        s = p["summary"]
        self.assertEqual(s["manifest_hash"], b.MHASH)
        self.assertEqual(s["by_attribution_rule"], {"cos_turn_shared_overhead": 1, "jev_classification_subject": 4, "none": 11})
        self.assertEqual(s["exec_vs_proposed"], {"exec_differs": 5})
        self.assertEqual(s["incomplete"][-1], "quota")

    def test_interventions_split_by_actor_like_the_rust_export(self):
        """人・案件 CoS・統括 CoS・operator・MCP が 1 件ずつ答え、主体不明（A2 以前）が 1 件。人の介入は human だけ。"""
        s = self.source()
        s.intervention("iv-1", "research_judgment", "human", "human:web", wall_ms=1000)
        s.intervention("iv-2", "research_judgment", "cos_project", "cos:tp")
        s.intervention("iv-3", "research_judgment", "cos_general", "cos:tg")
        s.intervention("iv-4", "recovery", "external_operator", "operator:claude-code")
        s.intervention("iv-5", "manual_handoff", "mcp_client", "mcp:c1")
        s.intervention("iv-6", "recovery", "unknown")
        s.close()
        p = b.project(self.db)
        sm = p["summary"]
        self.assertNotIn("interventions_by_kind", sm)
        self.assertEqual(sm["interventions_by_actor_kind"], {
            "human": 1, "cos_project": 1, "cos_general": 1, "external_operator": 1, "mcp_client": 1, "system": 0,
            "unknown": 1})
        self.assertEqual({k: v["count"] for k, v in sm["human_interventions_by_kind"].items()},
                         {"research_judgment": 1, "manual_handoff": 0, "recovery": 0, "unclassified": 0})
        ivs = {r["intervention_id"]: r for r in p["records"] if r["record_type"] == "intervention"}
        self.assertEqual((ivs["iv-2"]["actor_kind"], ivs["iv-2"]["actor_id"]), ("cos_project", "cos:tp"))
        self.assertEqual((ivs["iv-6"]["actor_kind"], ivs["iv-6"]["actor_id"]), ("unknown", None))
        ex = b.export_of(self.db)
        self.check(0, ex=ex)
        # 旧形式の export（interventions_by_kind・主体の欄なし）は改変として落ちる。
        old = json.loads(json.dumps(ex))
        old["summary"]["interventions_by_kind"] = old["summary"].pop("human_interventions_by_kind")
        del old["summary"]["interventions_by_actor_kind"]
        for r in old["records"]:
            if r["record_type"] == "intervention":
                del r["actor_kind"], r["actor_id"]
        self.check(1, "altered", ex=old)

    def test_fallback_is_suppression_not_success(self):
        s = self.source()
        s.c.execute("UPDATE events SET json = ? WHERE json LIKE ?", (json.dumps(b.event_body(R["r2"], "C", jev_ok=False)), f'%"{R["r2"]}"%'))
        s.c.execute("DELETE FROM decision_log_usage WHERE role = 'jev' AND run_id = ?", (R["r2"],))
        s.jev("r2", b.CHILD, ok=False, cost=0.0)
        s.close()
        code, rep, err = self.report()
        self.assertEqual(code, 0, err)
        self.assertEqual(rep["suppressed"]["classifier_fallback"], 1)
        self.assertEqual(rep["stages"]["by_arm"]["C"]["chain_ok"], 3)

    def test_redaction_is_projected_like_celerisctl(self):
        s = self.source()
        s.c.execute("UPDATE decision_log_runs SET fallback_reason = 'rate api_key=abc123' WHERE run_id = ?", (R["r1"],))
        s.close()
        ex = b.export_of(self.db)
        run = next(r for r in ex["records"] if r["record_type"] == "run" and r["run_id"] == R["r1"])
        self.assertEqual(run["fallback_reason"], "rate api_key=[redacted]")
        self.check(0, ex=ex)
        run["fallback_reason"] = "rate api_key=abc123"
        self.check(1, "altered", ex=ex)


class TrustBoundary(Base):
    def test_no_operator_source_is_nonzero(self):
        self.source().close()
        b.write(b.export_of(self.db), self.ev)
        code, _, err = self.run_cli(env={"CELERIS_DB": self.db})
        self.assertEqual(code, 2, err)
        self.assertIn("no_trusted_source", err)

    def test_bundled_self_declared_source_cannot_bypass(self):
        """完全な合成入力と、それに合う自称原記録を evidence に同梱しても外部照合を迂回できない。"""
        self.source().close()
        b.write(b.export_of(self.db), self.ev)
        bundled = os.path.join(self.ev, "source.db")
        os.replace(self.db, bundled)
        code, _, err = self.run_cli(env={b.reader.ENV_SOURCE: bundled})
        self.assertEqual(code, 2, err)
        self.assertIn("source_in_evidence_dir", err)
        code, _, err = self.run_cli(env={})
        self.assertEqual(code, 2, err)

    def test_unreachable_source_is_nonzero(self):
        self.source().close()
        b.write(b.export_of(self.db), self.ev)
        os.remove(self.db)
        code, _, err = self.run_cli()
        self.assertEqual(code, 2, err)
        self.assertIn("source_unreachable", err)

    def test_source_without_required_column_is_not_live(self):
        self.source().close()
        b.write(b.export_of(self.db), self.ev)
        c = sqlite3.connect(self.db)
        c.execute("ALTER TABLE decision_log_usage DROP COLUMN shared_overhead")
        c.commit()
        c.close()
        code, _, err = self.run_cli()
        self.assertEqual(code, 3, err)
        self.assertIn("decision_log_usage.shared_overhead", err)

    def test_missing_dir_or_file_is_no_evidence(self):
        code, _, err = self.run_cli()
        self.assertEqual(code, 2)
        self.assertIn("証拠 dir なし", err)
        self.source().close()
        b.write(b.export_of(self.db), self.ev)
        os.remove(os.path.join(self.ev, "provenance.json"))
        code, _, err = self.run_cli()
        self.assertEqual(code, 2)
        self.assertIn("provenance.json", err)


class ExportReconcile(Base):
    """原記録は正しいまま、evidence 側を崩す。全件照合で拒否し、段階判定へ渡さない。"""

    def setUp(self):
        super().setUp()
        self.source().close()
        self.ex = b.export_of(self.db)

    def recs(self, kind, run=None):
        return [r for r in self.ex["records"] if r["record_type"] == kind and (run is None or r.get("run_id") == run)]

    def test_identical_duplicate_line(self):
        text = "".join(json.dumps(r, separators=(",", ":")) + "\n" for r in self.ex["records"])
        first = text.splitlines(True)[0]
        err = self.check(1, "duplicate", "同一内容", raw_records=text + first)
        self.assertNotIn("chain_ok", err)

    def test_missing_record(self):
        self.ex["records"].remove(self.recs("usage", R["r4"])[0])
        self.check(1, "[missing]")

    def test_other_campaign_row_mixed(self):
        row = dict(self.recs("usage", R["r1"])[0], dedup_key="run:other:worker", campaign_id="other-campaign")
        self.ex["records"].insert(0, row)
        self.check(1, "[mixed]", "other-campaign")

    def test_orphan_row_in_same_campaign(self):
        self.ex["records"].insert(0, dict(self.recs("usage", R["r1"])[0], dedup_key="run:01M5AORPHAN:worker"))
        self.check(1, "[extra]")

    def test_altered_value_and_unknown_field(self):
        self.recs("usage", R["r1"])[0]["list_cost_usd"] = 0.77
        self.recs("trial", R["r2"])[0]["real_api_verified"] = True
        err = self.check(1, "[altered]", "list_cost_usd", "原記録に無い未知欄")
        self.assertIn("real_api_verified", err)

    def test_summary_cost_and_cli_manifest_hash_are_checked(self):
        self.ex["summary"]["by_role"]["jev"]["list_cost_usd"]["sum"] = 99.0
        self.check(1, "[summary]", "by_role.jev.list_cost_usd.sum")
        ex = b.export_of(self.db)
        ex["summary"]["manifest_hash"] = "b" * 64  # export の --manifest-hash 引数は証明にならない
        self.check(1, "[summary]", "manifest_hash", ex=ex)

    def test_order_differs(self):
        self.ex["records"][0], self.ex["records"][1] = self.ex["records"][1], self.ex["records"][0]
        self.check(1, "[order]")

    def test_task_tree_and_routing_events_are_reconciled(self):
        self.ex["tasks"][b.GRAND] = {"parent": b.ROOT}
        self.check(1, "[task-link]", b.GRAND)
        ex = b.export_of(self.db)
        ex["events"] = [e for e in ex["events"] if e["run_id"] != R["r4"]]
        self.check(1, "[routing-events]", "欠落", ex=ex)

    def test_retry_event_copied_from_the_parent_is_rejected(self):
        ev3 = next(e for e in self.ex["events"] if e["run_id"] == R["r3"])
        self.ex["events"] = [e for e in self.ex["events"] if e["run_id"] != R["r4"]] + [dict(ev3, run_id=R["r4"])]
        self.check(1, "[routing-events]")

    def test_self_declared_fixture_flags_and_hashes_are_not_proof(self):
        self.check(1, "[provenance]", "未知欄", prov_over={"fixture": True, "files_sha256": {}, "synthetic": False})
        self.check(1, "[provenance]", prov_over={"campaign_id": "other"})


class SourceBinding(Base):
    """原記録の側に欠けや食い違いがあり、export は原記録どおり（全件一致）でも 整合 OK にしない。"""

    def edit(self, sql, *args):
        s = self.source()
        s.c.execute(sql, args)
        return s

    def test_retry_without_its_own_event_does_not_borrow_the_parent_event(self):
        self.edit("DELETE FROM events WHERE json LIKE ?", f'%"{R["r4"]}"%').close()
        self.check(1, "[no-event]", R["r4"])

    def test_one_event_shared_by_retry_and_parent_is_rejected(self):
        s = self.edit("DELETE FROM events WHERE json LIKE ?", f'%"{R["r4"]}"%')
        s.event(b.GRAND, b.event_body(R["r3"], "C"))
        s.close()
        self.check(1, "[duplicate-event]", "[no-event]")

    def test_retry_parent_and_attempt_are_checked(self):
        self.edit("UPDATE decision_log_runs SET attempt_no = 5 WHERE run_id = ?", R["r4"]).close()
        self.check(1, "[retry]", "attempt_no")
        os.remove(self.db)
        self.edit("UPDATE decision_log_runs SET parent_run_id = ? WHERE run_id = ?", R["r1"], R["r4"]).close()
        self.check(1, "[retry]", "task")

    def test_task_only_jev_usage_is_unverified_not_live(self):
        """本番の dispatcher の形（Jev 消費行に run_id なし）は run に結べず、整合検査合格にしない。"""
        self.source(jev_bound=False).close()
        code, rep, err = self.report()
        self.assertEqual(code, 1, err)
        self.assertIn("[auth]", err)
        self.assertIn("Jev の消費行が無い", err)
        self.assertEqual(rep["stages"]["by_arm"]["C"]["chain_ok"], 0)
        self.assertEqual(sum("task 単位の fallback はしない" in u for u in rep["unverified"]), 4)

    def test_jev_cost_of_another_run_is_not_reused(self):
        self.edit("UPDATE decision_log_usage SET run_id = ? WHERE role = 'jev' AND run_id = ?", R["r3"], R["r4"]).close()
        self.check(1, f"trial {R['r4']}", "Jev の消費行が無い")

    def test_version_of_another_run_is_not_reused(self):
        self.edit("UPDATE decision_log_runs SET classifier_snapshot = NULL WHERE run_id = ?", R["r3"]).close()
        self.check(1, f"trial {R['r3']}", "応答版")

    def test_version_needs_a_jev_classifier_record(self):
        self.edit("UPDATE decision_log_runs SET classifier_kind = 'rule' WHERE run_id = ?", R["r2"]).close()
        self.check(1, f"trial {R['r2']}", "応答版")

    def test_auth_status_must_match_the_call_record(self):
        s = self.edit("DELETE FROM decision_log_usage WHERE role = 'jev' AND run_id = ?", R["r1"])
        s.jev("r1", b.ROOT, ok=False)
        s.close()
        self.check(1, "[auth]", "成功を示さない")

    def test_missing_jev_cost_is_not_success(self):
        self.edit("UPDATE decision_log_usage SET list_cost_usd = NULL WHERE role = 'jev'").close()
        self.check(1, "費用が欠測")

    def test_unexported_usage_row_is_a_missing_cost(self):
        s = self.source()
        s.usage(f"run:{R['r2']}:late", "reviewer", 0.7, run=R["r2"], task=b.CHILD, campaign=None)
        s.close()
        self.check(1, "[usage-not-exported]")

    def test_run_without_execution_record_is_rejected(self):
        self.edit("DELETE FROM runs WHERE run_id = ?", R["r2"]).close()
        self.check(1, "[run-link]")

    def test_fake_success_constant(self):
        s = self.source()
        for k in ("r1", "r2", "r3", "r4"):
            s.c.execute("UPDATE events SET json = ? WHERE json LIKE ?",
                        (json.dumps(b.event_body(R[k], "C", jev_ok=False, real_api_verified=True)), f'%"{R[k]}"%'))
        s.c.execute("DELETE FROM decision_log_usage WHERE role = 'jev'")
        s.close()
        self.check(1, "[fake-success]", "[auth]")

    def test_b1_with_jev_usage_is_mixed(self):
        s = self.source()
        s.jev("rb", b.BROOT)
        s.close()
        self.check(1, "B1")

    def test_primary_arm_a_is_mixed(self):
        self.edit("UPDATE decision_log_runs SET arm = 'A' WHERE run_id = ?", R["rb"]).close()
        self.check(1, "arm が B/C ではない")

    def test_pin_mismatch_in_grandchild(self):
        self.edit("UPDATE events SET json = ? WHERE json LIKE ?",
                  json.dumps(b.event_body(R["r3"], "C", resolved=dict(b.RESOLVED, lower_policy="other"))), f'%"{R["r3"]}"%').close()
        self.check(1, "[pin]", "root")

    def test_explicit_lane_and_evidence_table(self):
        self.edit("UPDATE events SET json = ? WHERE json LIKE ?",
                  json.dumps(b.event_body(R["r1"], "C", overridden_by="explicit",
                                          resolved=dict(b.RESOLVED, evidence_mode="pinned", evidence_table_id="t1"))),
                  f'%"{R["r1"]}"%').close()
        self.check(1, "明示 lane", "根拠表")

    def test_single_model_and_missing_exec_model(self):
        s = self.source()
        s.c.execute("UPDATE decision_log_runs SET proposed_candidates_json = ?", (json.dumps(b.CANDS[:1]),))
        s.c.execute("UPDATE decision_log_runs SET exec_model = NULL WHERE run_id = ?", (R["r3"],))
        s.close()
        self.check(1, "単一モデル", "exec_model が欠測")

    def test_no_grandchild_or_retry_coverage(self):
        s = self.source()
        for k in ("r3", "r4"):
            for t in ("decision_log_runs", "runs"):
                s.c.execute(f"DELETE FROM {t} WHERE run_id = ?", (R[k],))
            s.c.execute("DELETE FROM decision_log_usage WHERE run_id = ?", (R[k],))
            s.c.execute("DELETE FROM events WHERE json LIKE ?", (f'%"{R[k]}"%',))
        s.close()
        self.check(1, "孫", "retry の run が無い")


if __name__ == "__main__":
    unittest.main()
