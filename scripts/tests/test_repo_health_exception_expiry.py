"""repo health の期限付き例外（profile の例外の expires）: 期限切れは免除せず、期限は base 側で判定する。

ADR 2026-11-07-repo-health-contract の付記（2026-11-10、期限付き例外）。一時 dir の小さな git repo に、本物の検査器
（scripts/dev/repo-health/repo_health.py。写さない）と本物の rule size.responsibility-growth（file_lines = 3）を当てる。

make_expiry_repo(root, case) は試験の外（承認の材料を作る unit record）からも使う。返す (base_sha, head_sha) を
`repo_health.py --repo <root> --base <base_sha> --head <head_sha> --policy-version 1 --out <report.json>` に渡すと、
次の report になる（どの日に走らせても同じ。基準日は base の commit の committer date の UTC の日付）:

    case      base の commit の日付            基準日      base の expires  head の expires  exit
    expired   2026-03-02T10:00:00+09:00       2026-03-02  2026-03-01       2026-03-01       1（期限切れ）
    in_term   2026-03-02T01:00:00+09:00       2026-03-01  2026-03-01       2026-03-01       0（当日まで有効）
    extended  2026-03-02T10:00:00+09:00       2026-03-02  2026-03-01       2027-03-01       1（期限切れ + policy.changed）

head は src/big.py を 2 行から 6 行にする（size.responsibility-growth の file_lines 3 を超える new の finding）。例外は
path '^src/big\\.py$' でこの finding に一致する。head の commit の日付は、expired では期限より前、in_term では期限より
後にしてあり、head の日付で判定すれば結果が逆になる。in_term の base の日付は JST では 2026-03-02 で、UTC で判定する
ときだけ期限の当日になる。
"""
from __future__ import annotations

import datetime
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TOOL_SRC = ROOT / "scripts/dev/repo-health"
CHECKER = TOOL_SRC / "repo_health.py"
VALIDATOR = TOOL_SRC / "validate_report.py"
SCHEMA_DIR = TOOL_SRC / "schema"
PROFILE_PATH = "scripts/dev/repo-health/profiles/default.toml"
RULE = "size.responsibility-growth"

CASES = ("expired", "in_term", "extended")
BASE_EXPIRES = "2026-03-01"
EXTENDED_EXPIRES = "2027-03-01"
# case -> (base の commit の日付, head の commit の日付, head の expires)
_CASE_DATES = {
    "expired": ("2026-03-02T10:00:00+09:00", "2026-02-01T00:00:00+00:00", BASE_EXPIRES),
    "in_term": ("2026-03-02T01:00:00+09:00", "2026-12-01T00:00:00+00:00", BASE_EXPIRES),
    "extended": ("2026-03-02T10:00:00+09:00", "2026-03-03T00:00:00+00:00", EXTENDED_EXPIRES),
}
# case -> 基準日（report の policy.exceptions_as_of）
AS_OF = {"expired": "2026-03-02", "in_term": "2026-03-01", "extended": "2026-03-02"}
REASON = "試験: src/big.py の分割は次の段で行う（期限付き）"
# profile の中の例外の表の行（report の source.line）
EXCEPTION_LINE = 6

NO_EXPIRES = object()  # _make_repo の expires に渡すと expires の欄を書かない


def _profile(expires) -> str:
    line = "" if expires is NO_EXPIRES else f"expires = {expires}\n"
    return ('version = "1"\n'
            "\n"
            f'[rules."{RULE}"]\n'
            "file_lines = 3\n"
            "\n"
            f'[[rules."{RULE}".exceptions]]\n'
            "path = '^src/big\\.py$'\n"
            f'reason = "{REASON}"\n'
            f"{line}")


def _git(repo: Path, *args: str, date: str = "2026-01-01T00:00:00+00:00") -> str:
    env = {**os.environ, "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@example.com", "GIT_AUTHOR_DATE": date,
           "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@example.com", "GIT_COMMITTER_DATE": date}
    return subprocess.run(["git", "-C", str(repo), "-c", "commit.gpgsign=false", *args], check=True,
                          capture_output=True, text=True, env=env).stdout.strip()


def _write(repo: Path, rel: str, text: str) -> None:
    path = repo / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def _commit(repo: Path, msg: str, date: str) -> str:
    _git(repo, "add", "-A")
    _git(repo, "commit", "-q", "--allow-empty", "-m", msg, date=date)
    return _git(repo, "rev-parse", "HEAD")


def _make_repo(root, base_expires, head_expires, base_date: str, head_date: str) -> tuple[str, str]:
    repo = Path(root)
    repo.mkdir(parents=True, exist_ok=True)
    if any(repo.iterdir()):
        raise ValueError(f"{repo} は空の dir でなければならない")
    _git(repo, "init", "-q", "-b", "main")
    _write(repo, "README.md", "# expiry fixture\n")
    _write(repo, "src/big.py", "a = 1\nb = 2\n")
    _write(repo, PROFILE_PATH, _profile(base_expires))
    base = _commit(repo, "base: 期限付きの例外", base_date)
    _write(repo, "src/big.py", "".join(f"v{i} = {i}\n" for i in range(6)))
    if head_expires != base_expires:
        _write(repo, PROFILE_PATH, _profile(head_expires))
    head = _commit(repo, "head: src/big.py を 6 行にする", head_date)
    return base, head


def make_expiry_repo(root, case: str) -> tuple[str, str]:
    """root（無いか空の dir）に case（expired・in_term・extended）の git repo を作り (base_sha, head_sha) を返す。

    commit の作者・日付は固定なので、同じ case からは同じ sha が出る。各 case の日付と結果は module の docstring。
    """
    if case not in _CASE_DATES:
        raise ValueError(f"case は {CASES} のどれか（{case!r}）")
    base_date, head_date, head_expires = _CASE_DATES[case]
    return _make_repo(root, BASE_EXPIRES, head_expires, base_date, head_date)


def run_checker(repo, base: str, head: str, out) -> tuple[subprocess.CompletedProcess, dict | None]:
    """本物の検査器を全 rule で走らせる。report を書かなければ None。"""
    out = Path(out)
    if out.exists():
        out.unlink()
    proc = subprocess.run([sys.executable, "-I", str(CHECKER), "--repo", str(repo), "--base", base, "--head", head,
                           "--policy-version", "1", "--out", str(out)], capture_output=True, text=True)
    report = json.loads(out.read_text(encoding="utf-8")) if out.exists() else None
    return proc, report


def validate(*paths) -> subprocess.CompletedProcess:
    return subprocess.run([sys.executable, "-I", str(VALIDATOR), *map(str, paths)], capture_output=True, text=True)


def _engine():
    if str(TOOL_SRC) not in sys.path:
        sys.path.insert(0, str(TOOL_SRC))
    from health import engine  # noqa: PLC0415 — 検査器の package は scripts/dev/repo-health の下
    return engine


class RepoHealthExceptionExpiryTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp(prefix="rh-expiry-", dir=os.environ.get("TMPDIR")))
        self.shas: dict[str, tuple[str, str]] = {}

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def run_case(self, case: str, out: str = "report.json"):
        repo = self.tmp / case
        if case not in self.shas:
            self.shas[case] = make_expiry_repo(repo, case)
        base, head = self.shas[case]
        proc, report = run_checker(repo, base, head, self.tmp / out)
        self.assertIsNotNone(report, proc.stdout + proc.stderr)
        check = validate(self.tmp / out)
        self.assertEqual(check.returncode, 0, check.stdout)
        return proc, report

    @staticmethod
    def size_finding(report: dict) -> dict:
        (f,) = [f for f in report["findings"] if f["rule_id"] == RULE]
        return f

    @staticmethod
    def rule(report: dict, rid: str) -> dict:
        return next(r for r in report["rules"] if r["id"] == rid)

    def expected_exception(self, expires: str) -> dict:
        return {"reason": REASON, "source": {"path": PROFILE_PATH, "line": EXCEPTION_LINE}, "expires": expires}

    # ------------------------------------------------------------ tests
    def test_exception_expired_finding_counts_as_fail(self):
        """不変条件: 期限を過ぎた例外は finding を免除しない（exception=null、expired_exception に書き、fail 級）。

        既存の試験の例外（test_repo_health_core の test_existing_new_and_exception）は期限の無い例外だけで、
        期限切れの例外が免除し続けても検出できない。
        """
        proc, rep = self.run_case("expired")
        self.assertEqual(proc.returncode, 1, proc.stdout + proc.stderr)
        f = self.size_finding(rep)
        self.assertEqual((f["severity"], f["delta"]), ("error", "new"))
        self.assertIsNone(f["exception"])
        self.assertEqual(f["expired_exception"], self.expected_exception(BASE_EXPIRES))
        self.assertTrue(_engine().is_fail_grade(f), "fail 級の式（exception==null）は変えずに数える")
        self.assertEqual(rep["policy"]["exceptions_as_of"], AS_OF["expired"])
        self.assertFalse(rep["policy"]["changed"])
        self.assertEqual(self.rule(rep, RULE)["status"], "fail")
        self.assertEqual(self.rule(rep, "policy.changed")["status"], "pass")
        self.assertEqual(rep["summary"]["counts"]["fail_grade"], 1)
        self.assertEqual((rep["summary"]["verdict"], rep["summary"]["exit_code"]), ("fail", 1))
        self.assertIn(f"例外は期限切れ: expires={BASE_EXPIRES} 基準日={AS_OF['expired']}", proc.stdout)
        # head の commit の日付（2026-02-01）は期限より前だが、判定には使わない
        head_date = _git(self.tmp / "expired", "show", "-s", "--format=%cI", self.shas["expired"][1])
        self.assertLess(head_date[:10], BASE_EXPIRES)

    def test_exception_expiry_in_term_exempts_finding(self):
        """不変条件: 期限の当日（基準日 == expires）までは免除し、基準日は base の commit の UTC の日付で決める。

        base の commit は 2026-03-02T01:00:00+09:00（UTC では 2026-03-01）。時差の日付・head の日付（2026-12-01）・
        実行時の時計（期限より後）のどれで判定しても期限切れになり、この試験が落ちる。
        """
        proc, rep = self.run_case("in_term")
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        f = self.size_finding(rep)
        self.assertEqual((f["severity"], f["delta"]), ("error", "new"))
        self.assertEqual(f["exception"], self.expected_exception(BASE_EXPIRES))
        self.assertNotIn("expired_exception", f)
        self.assertFalse(_engine().is_fail_grade(f))
        self.assertEqual(rep["policy"]["exceptions_as_of"], AS_OF["in_term"])
        self.assertEqual(rep["policy"]["exceptions_as_of"], BASE_EXPIRES, "期限の当日は有効")
        self.assertEqual(self.rule(rep, RULE)["status"], "pass")
        self.assertEqual(rep["summary"]["counts"]["fail_grade"], 0)
        repo = self.tmp / "in_term"
        base, head = self.shas["in_term"]
        self.assertEqual(_git(repo, "show", "-s", "--format=%cI", base), "2026-03-02T01:00:00+09:00")
        self.assertGreater(_git(repo, "show", "-s", "--format=%cI", head)[:10], BASE_EXPIRES)
        self.assertGreater(datetime.date.today().isoformat(), BASE_EXPIRES, "実行時の時計は期限より後")

    def test_exception_expiry_extension_by_candidate_does_not_pass(self):
        """不変条件: 候補（head）が期限を延ばしても base の期限で判定し、延長は policy.changed で fail にする。

        head の profile で判定する実装なら、size の finding は免除され、fail は policy.changed だけになる。
        """
        proc, rep = self.run_case("extended")
        self.assertEqual(proc.returncode, 1, proc.stdout + proc.stderr)
        repo = self.tmp / "extended"
        head_profile = _git(repo, "show", f"{self.shas['extended'][1]}:{PROFILE_PATH}")
        self.assertIn(f"expires = {EXTENDED_EXPIRES}", head_profile)
        self.assertEqual(rep["policy"]["source"], "base")
        self.assertTrue(rep["policy"]["changed"])
        self.assertEqual(rep["policy"]["exceptions_as_of"], AS_OF["extended"])
        f = self.size_finding(rep)
        self.assertIsNone(f["exception"])
        self.assertEqual(f["expired_exception"], self.expected_exception(BASE_EXPIRES), "base の期限を使う")
        self.assertEqual(self.rule(rep, RULE)["status"], "fail")
        self.assertEqual(self.rule(rep, "policy.changed")["status"], "fail")
        (pf,) = [x for x in rep["findings"] if x["rule_id"] == "policy.changed"]
        self.assertEqual(pf["severity"], "error")
        self.assertEqual([e["path"] for e in pf["evidence"]], [PROFILE_PATH])
        self.assertEqual(sorted(x["rule_id"] for x in rep["findings"] if _engine().is_fail_grade(x)),
                         ["policy.changed", RULE])
        self.assertEqual(rep["summary"]["counts"]["fail_grade"], 2)

    def test_exception_expiry_rejects_invalid_value(self):
        """不変条件: expires が TOML の日付でなければ profile の読み込みエラー（exit 2、report を書かない）。

        datetime.datetime は datetime.date の subclass なので、isinstance で確かめる実装は日時を通してしまう。
        """
        bad = {"string": '"2026-03-01"', "local-datetime": "2026-03-01T00:00:00",
               "offset-datetime": "2026-03-01T00:00:00Z", "time": "00:00:00", "integer": "20260301",
               "float": "2026.0301"}
        for name, literal in bad.items():
            with self.subTest(name):
                repo = self.tmp / f"bad-{name}"
                base, head = _make_repo(repo, literal, literal, "2026-03-02T10:00:00+09:00",
                                        "2026-03-03T00:00:00+00:00")
                proc, rep = run_checker(repo, base, head, self.tmp / f"bad-{name}.json")
                self.assertEqual(proc.returncode, 2, proc.stdout + proc.stderr)
                self.assertIsNone(rep, "report を書かない")
                self.assertIn("profile_invalid", proc.stderr)
                self.assertIn(f'rules.{RULE}.exceptions[0].expires は TOML の日付', proc.stderr)

    def test_exception_expiry_report_validates(self):
        """不変条件: 3 例の report は契約（schema と S13）に合い、決定的で、期限の無い profile の report は欄が増えない。

        S13 に反する改変（exception と expired_exception の同時記載・基準日の欠け・期限の前後の食い違い）は
        検証器が落とす。既存の契約の試験は期限の欄を持つ report を 1 件も検証しない。
        """
        reports = {}
        for case in CASES:
            with self.subTest(case):
                self.run_case(case, out=f"{case}-1.json")
                self.run_case(case, out=f"{case}-2.json")
                first = (self.tmp / f"{case}-1.json").read_bytes()
                self.assertEqual(first, (self.tmp / f"{case}-2.json").read_bytes(), "同じ入力で byte 単位で同じ")
                reports[case] = json.loads(first)

        def broken(case: str, name: str, mutate) -> None:
            rep = json.loads(json.dumps(reports[case]))
            mutate(rep, next(i for i, f in enumerate(rep["findings"]) if f["rule_id"] == RULE))
            path = self.tmp / f"broken-{name}.json"
            path.write_text(json.dumps(rep, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
            proc = validate(path)
            self.assertEqual(proc.returncode, 1, f"{name} は不正のはず\n{proc.stdout}")
            self.assertIn("S13:", proc.stdout, name)

        def both(rep, i):
            ee = rep["findings"][i]["expired_exception"]
            rep["findings"][i]["exception"] = {k: ee[k] for k in ("reason", "source")}
            rep["summary"]["counts"]["fail_grade"] -= 1  # 免除したと読む集計に合わせ、S13 だけを破る
            rep["rules"] = [dict(r, status="pass", reason="") if r["id"] == RULE else r for r in rep["rules"]]
            rep["summary"].update(verdict="pass", exit_code=0)

        broken("expired", "both", both)
        broken("expired", "no-as-of", lambda rep, i: rep["policy"].pop("exceptions_as_of"))
        broken("expired", "not-yet-expired", lambda rep, i: rep["policy"].update(exceptions_as_of=BASE_EXPIRES))
        broken("in_term", "already-expired", lambda rep, i: rep["policy"].update(exceptions_as_of="2026-03-02"))
        broken("in_term", "as-of-null", lambda rep, i: rep["policy"].update(exceptions_as_of=None))

        # schema の例: 期限切れと期限内を 1 件ずつ持つ例は妥当、S13 の不正な例は 4 件とも S13 で落ちる
        example = SCHEMA_DIR / "examples" / "exception-expiry.json"
        self.assertEqual(validate(example).returncode, 0)
        ex = json.loads(example.read_text(encoding="utf-8"))
        self.assertTrue(any(f.get("expired_exception") for f in ex["findings"]))
        self.assertTrue(any((f["exception"] or {}).get("expires") for f in ex["findings"]))
        for name in ("expired-exception-with-exception", "exception-expires-before-as-of",
                     "expired-exception-not-yet-expired", "exception-expires-without-as-of"):
            proc = validate(SCHEMA_DIR / "bad-examples" / f"{name}.json")
            self.assertEqual(proc.returncode, 1, name)
            self.assertIn("S13:", proc.stdout, name)

        # 期限の無い profile: 例外は今と同じく免除し、report に expires・expired_exception・exceptions_as_of が無い
        repo = self.tmp / "no-expires"
        base, head = _make_repo(repo, NO_EXPIRES, NO_EXPIRES, "2026-03-02T10:00:00+09:00", "2026-03-03T00:00:00+00:00")
        proc, rep = run_checker(repo, base, head, self.tmp / "no-expires.json")
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.assertEqual(validate(self.tmp / "no-expires.json").returncode, 0)
        self.assertNotIn("exceptions_as_of", rep["policy"])
        f = self.size_finding(rep)
        self.assertEqual(f["exception"], {"reason": REASON, "source": {"path": PROFILE_PATH, "line": EXCEPTION_LINE}})
        text = (self.tmp / "no-expires.json").read_text(encoding="utf-8")
        for key in ('"expires"', '"expired_exception"', '"exceptions_as_of"'):
            self.assertNotIn(key, text)


if __name__ == "__main__":
    unittest.main()
