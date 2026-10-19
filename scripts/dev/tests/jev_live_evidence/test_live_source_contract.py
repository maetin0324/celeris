"""jev-live-evidence.py の外部出自契約の攻撃試験（CLI を subprocess で起動する）。

実行: python3 -m unittest discover -s scripts/dev/tests/jev_live_evidence -p test_live_source_contract.py

契約: 信頼の根は運用者が入力とは独立に設定する CELERIS_JEV_SOURCE_DB だけ。evidence dir の中身（provenance の
source / export_command / files_sha256 / manifest_hash、同梱 DB）はどれも証明にならず、照合されない限り非 0。

test double の明記: 読取元は builder が crates/task-core/migrations から作る一時 SQLite（運用者設定を模擬した独立
DB）で、中身は合成。ここで exit 0 になる正例は「変換・照合アルゴリズムの正例」であり、実 daemon の原記録の
証明ではない。実データでの検証は実コマンド（--evidence-dir <実 dir> --require-live）の結果だけが示す。
"""
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "..", "..", "jev-live-evidence.py")
sys.path.insert(0, HERE)
import builder as b  # noqa: E402

R = b.RUNS
ENV = b.reader.ENV_SOURCE
DAEMON_VARS = (ENV, "CELERIS_DB", "CELERIS_RUN_DB")


def sha256(path):
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = self.tmp.name
        os.makedirs(os.path.join(self.root, "measure"))
        # 運用者設定を模擬した独立一時 DB（test double）。evidence dir の外に置く。
        self.db = os.path.join(self.root, "measure", "celeris.db")
        self.ev = os.path.join(self.root, "ev")
        self.marker = os.path.join(self.root, "executed.marker")

    def cli(self, env, live=True):
        full = {k: v for k, v in os.environ.items() if k not in DAEMON_VARS}
        full.update(env)
        args = [sys.executable, SCRIPT, "--evidence-dir", self.ev, "--quiet"] + (["--require-live"] if live else [])
        p = subprocess.run(args, capture_output=True, text=True, env=full, cwd=self.root)
        return p.returncode, p.stderr

    def trusted(self):
        return {ENV: self.db}

    def assert_nonzero(self, code, err, *needles):
        self.assertNotEqual(code, 0, err)
        self.assertNotIn("OK:", err)
        for n in needles:
            self.assertIn(n, err)

    def write(self, ex, **kw):
        return b.write(ex, self.ev, **kw)

    def rehash(self, extra=None):
        """自己申告の provenance を「完全」にする: 偽 source・files_sha256 を書き直す（証明にならないことを見る）。"""
        p = os.path.join(self.ev, "provenance.json")
        with open(p, encoding="utf-8") as f:
            prov = json.load(f)
        prov["files_sha256"] = {n: sha256(os.path.join(self.ev, n))
                                for n in ("records.jsonl", "summary.json", "routing_events.jsonl")}
        prov["source"] = "celerisctl campaign export"
        prov.update(extra or {})
        with open(p, "w", encoding="utf-8") as f:
            json.dump(prov, f, indent=2)


class AlgorithmPositive(Base):
    """test double の正例（変換・照合アルゴリズムの正例。実 daemon の証明ではない）。"""

    def test_independent_temp_source_reconciles(self):
        b.default_source(self.db).close()
        self.write(b.export_of(self.db))
        code, err = self.cli(self.trusted())
        self.assertEqual(code, 0, err)
        self.assertIn("OK:", err)


class NoTrustedSource(Base):
    def setUp(self):
        super().setUp()
        b.default_source(self.db).close()
        self.write(b.export_of(self.db))

    def test_unset_is_nonzero_even_with_daemon_db_vars(self):
        code, err = self.cli({"CELERIS_DB": self.db, "CELERIS_RUN_DB": self.db})
        self.assert_nonzero(code, err, "no_trusted_source")
        self.assertEqual(code, 2)

    def test_unreachable_is_nonzero(self):
        code, err = self.cli({ENV: os.path.join(self.root, "measure", "nope.db")})
        self.assert_nonzero(code, err, "source_unreachable")
        with open(os.path.join(self.root, "measure", "junk.db"), "w") as f:
            f.write("not sqlite")
        code, err = self.cli({ENV: os.path.join(self.root, "measure", "junk.db")})
        self.assert_nonzero(code, err)

    def test_relative_and_daemon_db_source_is_refused(self):
        code, err = self.cli({ENV: "measure/celeris.db"})
        self.assert_nonzero(code, err, "source_not_absolute")
        code, err = self.cli({ENV: self.db, "CELERIS_RUN_DB": self.db})
        self.assert_nonzero(code, err, "source_is_daemon_db")


class SelfDeclaredInput(Base):
    """偽 source・有効 ULID・再 hash した旧 default_case 相当の自己申告だけでは必ず非 0。"""

    def setUp(self):
        super().setUp()
        b.default_source(self.db).close()
        self.ex = b.export_of(self.db)
        for tid in self.ex["tasks"]:
            self.assertRegex(tid, r"^[0-9A-HJKMNP-TV-Z]{26}$")  # task は有効な ULID

    def test_rehashed_default_case_without_source_is_nonzero(self):
        self.write(self.ex)
        self.rehash()
        os.remove(self.db)  # 照合できる原記録は世界のどこにも無い
        for env in ({}, {ENV: self.db}):
            code, err = self.cli(env)
            self.assert_nonzero(code, err)
            self.assertEqual(code, 2, err)

    def test_rehashed_default_case_against_an_unrelated_source_is_nonzero(self):
        self.write(self.ex)
        self.rehash()
        os.remove(self.db)
        # 運用者の読取元は実在するが、この campaign の原記録を持たない。
        s = b.Source(self.db)
        s.task(b.ROOT)
        s.campaign_root(b.ROOT, "C", campaign="another-campaign")
        s.close()
        code, err = self.cli(self.trusted())
        self.assert_nonzero(code, err, "campaign_not_found")

    def test_rehashed_default_case_with_forged_values_is_nonzero(self):
        """運用者の読取元と食い違う値を自己申告 hash で固めても照合で落ちる。"""
        for r in self.ex["records"]:
            if r["record_type"] == "trial":
                r["classifier_snapshot"] = "forged-version"
        self.write(self.ex)
        self.rehash({"manifest_hash": b.MHASH})
        code, err = self.cli(self.trusted())
        self.assert_nonzero(code, err, "[altered]", "classifier_snapshot")
        self.assertEqual(code, 1)

    def test_self_declared_flags_in_provenance_are_rejected(self):
        self.write(self.ex, prov_over={"fixture": False, "real_daemon": True, "verified_by": "operator"})
        self.rehash()
        code, err = self.cli(self.trusted())
        self.assert_nonzero(code, err, "[provenance]", "未知欄")


class BundledSource(Base):
    """完全な合成入力 + 同梱の自称原記録。外部照合を迂回できず、同梱 DB を自動選択しない。"""

    def setUp(self):
        super().setUp()
        b.default_source(self.db).close()
        self.write(b.export_of(self.db))
        self.bundled = os.path.join(self.ev, "celeris.db")
        os.replace(self.db, self.bundled)  # evidence と完全に一致する自称原記録を同梱
        self.rehash({"source_db": self.bundled})

    def test_bundled_db_is_not_auto_selected(self):
        before = sha256(self.bundled)
        code, err = self.cli({})
        self.assert_nonzero(code, err, "no_trusted_source")
        self.assertEqual(code, 2)
        self.assertNotIn("source_in_evidence_dir", err)  # 同梱 DB を探しにも行かない
        self.assertEqual(sha256(self.bundled), before)

    def test_operator_pointing_into_evidence_is_refused(self):
        code, err = self.cli({ENV: self.bundled})
        self.assert_nonzero(code, err, "source_in_evidence_dir")
        sub = os.path.join(self.ev, "nested")
        os.makedirs(sub)
        link = os.path.join(self.root, "measure", "link.db")
        os.symlink(self.bundled, link)  # 外を装った symlink も実 path で判定
        code, err = self.cli({ENV: link})
        self.assert_nonzero(code, err, "source_in_evidence_dir")


class InputPathsAndCommands(Base):
    """入力中の path / command は読取元にも実行にも使わない。"""

    def setUp(self):
        super().setUp()
        b.default_source(self.db).close()
        self.ex = b.export_of(self.db)
        self.cmd = f"touch {self.marker}"

    def test_command_and_path_in_provenance_are_not_used(self):
        self.write(self.ex, prov_over={"export_command": f"{self.cmd}; sh -c '{self.cmd}'"})
        self.rehash({"source_db": self.db, "source_command": self.cmd})
        # 入力に正しい読取元の path が書いてあっても、運用者の設定が無ければ使わない。
        code, err = self.cli({})
        self.assert_nonzero(code, err, "no_trusted_source")
        # 運用者の設定があっても、未知欄（自称の path/command）は自己申告として拒否する。
        code, err = self.cli(self.trusted())
        self.assert_nonzero(code, err, "[provenance]")
        self.assertFalse(os.path.exists(self.marker), "入力のコマンドが実行された")

    def test_command_inside_known_field_is_not_executed(self):
        self.write(self.ex, prov_over={"export_command": f"$({self.cmd})", "generated_at": f"`{self.cmd}`"})
        code, err = self.cli(self.trusted())
        self.assertEqual(code, 0, err)  # export_command は照合対象外の記録欄（証明にも実行にも使わない）
        self.assertFalse(os.path.exists(self.marker), "入力のコマンドが実行された")

    def test_command_in_records_is_not_executed(self):
        for r in self.ex["records"]:
            if r["record_type"] == "run" and r["run_id"] == R["r1"]:
                r["fallback_reason"] = f"$({self.cmd})"
        self.write(self.ex)
        code, err = self.cli(self.trusted())
        self.assert_nonzero(code, err, "[altered]", "fallback_reason")
        self.assertFalse(os.path.exists(self.marker), "入力のコマンドが実行された")


class Reuse(Base):
    """別 retry の event / usage / 応答版（classifier_snapshot）の使い回しを拒否する。"""

    def edit(self, *stmts):
        s = b.default_source(self.db)
        for sql, args in stmts:
            s.c.execute(sql, args)
        return s

    def check(self, s, *needles):
        s.close()
        self.write(b.export_of(self.db))
        code, err = self.cli(self.trusted())
        self.assert_nonzero(code, err, *needles)
        self.assertEqual(code, 1, err)

    def test_retry_reusing_parent_event_in_evidence(self):
        b.default_source(self.db).close()
        ex = b.export_of(self.db)
        ev3 = next(e for e in ex["events"] if e["run_id"] == R["r3"])
        ex["events"] = [e for e in ex["events"] if e["run_id"] != R["r4"]] + [dict(ev3, run_id=R["r4"])]
        self.write(ex)
        code, err = self.cli(self.trusted())
        self.assert_nonzero(code, err, "[routing-events]")

    def test_retry_without_own_event_in_source(self):
        self.check(self.edit(("DELETE FROM events WHERE json LIKE ?", (f'%"{R["r4"]}"%',))), "[no-event]", R["r4"])

    def test_one_event_for_two_runs(self):
        s = self.edit(("DELETE FROM events WHERE json LIKE ?", (f'%"{R["r4"]}"%',)))
        s.event(b.GRAND, b.event_body(R["r3"], "C"))
        self.check(s, "[duplicate-event]")

    def test_jev_usage_of_another_retry(self):
        self.check(self.edit(("UPDATE decision_log_usage SET run_id = ? WHERE role = 'jev' AND run_id = ?", (R["r3"], R["r4"]))),
                   f"trial {R['r4']}", "Jev の消費行が無い")

    def test_task_only_usage_is_not_borrowed(self):
        s = b.default_source(self.db, jev_bound=False)
        self.check(s, "[auth]", "Jev の消費行が無い")

    def test_response_version_of_another_retry(self):
        self.check(self.edit(("UPDATE decision_log_runs SET classifier_snapshot = NULL WHERE run_id = ?", (R["r4"],))),
                   f"trial {R['r4']}", "応答版")


class ProjectionMismatch(Base):
    """重複・孤立・混入・欠落・summary と原欄の改変。"""

    def setUp(self):
        super().setUp()
        b.default_source(self.db).close()
        self.ex = b.export_of(self.db)

    def recs(self, kind, run):
        return [r for r in self.ex["records"] if r["record_type"] == kind and r.get("run_id") == run]

    def run_case(self, *needles, **kw):
        self.write(self.ex, **kw)
        code, err = self.cli(self.trusted())
        self.assert_nonzero(code, err, *needles)
        self.assertEqual(code, 1, err)

    def test_duplicate(self):
        text = "".join(json.dumps(r, separators=(",", ":")) + "\n" for r in self.ex["records"])
        self.run_case("[duplicate]", raw_records=text + text.splitlines(True)[-1])

    def test_orphan(self):
        self.ex["records"].insert(0, dict(self.recs("usage", R["r1"])[0], dedup_key="run:01M5AORPHAN000000000000000:worker"))
        self.run_case("[extra]")

    def test_mixed_campaign(self):
        self.ex["records"].insert(0, dict(self.recs("usage", R["r1"])[0], dedup_key="run:x:worker", campaign_id="other"))
        self.run_case("[mixed]")

    def test_missing(self):
        self.ex["records"].remove(self.recs("trial", R["r4"])[0])
        self.run_case("[missing]")

    def test_summary_altered(self):
        self.ex["summary"]["by_retry"]["retry"]["list_cost_usd"]["sum"] = 0.0
        self.run_case("[summary]", "by_retry.retry.list_cost_usd.sum")

    def test_source_field_altered(self):
        self.recs("run", R["r2"])[0]["exec_model"] = "claude-sonnet-5-5"
        self.recs("usage", R["r3"])[0]["list_cost_usd"] = 0.0
        self.run_case("[altered]", "exec_model", "list_cost_usd")


class MissingVersionOrAuth(Base):
    """原記録に応答版・認証（Jev 呼出成功）記録が欠けたら live にしない（export が原記録どおりでも）。"""

    def check(self, *stmts_needles):
        stmts, needles = stmts_needles[0], stmts_needles[1:]
        s = b.default_source(self.db)
        for sql, args in stmts:
            s.c.execute(sql, args)
        s.close()
        self.write(b.export_of(self.db))
        code, err = self.cli(self.trusted())
        self.assert_nonzero(code, err, *needles)

    def test_all_versions_missing(self):
        self.check([("UPDATE decision_log_runs SET classifier_snapshot = NULL", ())], "応答版", "[auth]")

    def test_all_auth_records_missing(self):
        self.check([("DELETE FROM decision_log_usage WHERE role = 'jev'", ())], "Jev の消費行が無い", "[auth]")

    def test_auth_record_says_fallback(self):
        self.check([("UPDATE decision_log_usage SET dedup_key = replace(dedup_key, ':ok:', ':fallback:') WHERE role = 'jev'", ())],
                   "[auth]", "成功を示さない")


if __name__ == "__main__":
    unittest.main()
