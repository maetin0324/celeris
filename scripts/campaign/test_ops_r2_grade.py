#!/usr/bin/env python3
"""ops-r2-grade.py（採点 wrapper）と ops-r2-set-private-sha.py の試験。

別の uid は作れないので、EVAL_ROOT はこの試験の uid が持つ 0700 の dir にし、uid の不一致は
CELERIS_EVAL_TEST_EUID で注入する。実 uid（celeris-eval）での確認は docs/ops/ops-r2-isolation.md §4.7 の手順で人が行う。
"""
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest

HERE = Path(__file__).parent
GRADE = HERE / 'ops-r2-grade.py'
SETSHA = HERE / 'ops-r2-set-private-sha.py'
LEAK = HERE / 'check-ops-r1-leak.py'
CASE = 'k2-bf-xattr-restart-campaign'
SECRET = 'hidden-acceptance-command-zq81-bbf0-private-only-answer'

FAKE_CC = r'''#!/usr/bin/env python3
import json, os, sys
a = sys.argv[1:]
assert a[0] == 'grade', a
case = a[a.index('--case') + 1]
art = a[a.index('--artifact') + 1]
work = a[a.index('--work') + 1]
root = open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'eval_root')).read()
assert case == os.path.join(root, 'grade-cases', 'final', '%s', '%s.json'), case
assert work.startswith(os.path.join(root, 'work') + '/') and os.path.isdir(work), work
assert '--grader-root' not in a and '--grade-case-root' not in a, a
env = dict(os.environ)
home = env['HOME']
assert env['PATH'].split(':')[0] == os.path.join(home, '.cargo', 'bin'), env['PATH']
assert env['CARGO_HOME'] == os.path.join(home, '.cargo') and env['RUSTUP_HOME'] == os.path.join(home, '.rustup')
assert 'CALLER_ONLY' not in env
priv = json.load(open(os.path.join(root, 'private', 'final', '%s', 'private.json')))
print('debug private:', json.dumps(priv), file=sys.stderr)
if os.path.exists(os.path.join(art, 'error.txt')):
    # 新しい契約: 判定を作れないエラーは stdout 空・stderr に理由・exit 2
    print('error: cannot read grade-case', file=sys.stderr)
    sys.exit(2)
ok = os.path.exists(os.path.join(art, 'result.txt'))
v = {'pass': ok}
if os.path.exists(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'leak')):
    v['note'] = priv['answer']
print(json.dumps(v))
sys.exit(0 if ok else 1)
''' % (CASE, CASE, CASE)


class GradeTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(dir=os.environ.get('TMPDIR'))
        r = Path(self.tmp.name)
        self.eval_root = r / 'eval'
        (self.eval_root / 'private' / 'final' / CASE).mkdir(parents=True)
        os.chmod(self.eval_root, 0o700)
        (self.eval_root / 'private' / 'final' / CASE / 'private.json').write_text(
            json.dumps({'answer': SECRET}))
        self.bin = r / 'bin'
        self.bin.mkdir()
        (self.bin / 'celeris-campaign').write_text(FAKE_CC)
        os.chmod(self.bin / 'celeris-campaign', 0o755)
        (self.bin / 'eval_root').write_text(str(self.eval_root))
        gc = self.eval_root / 'grade-cases' / 'final' / CASE
        gc.mkdir(parents=True)
        (gc / f'{CASE}.json').write_text('{}')
        self.home = r / 'home'
        self.home.mkdir()
        shutil.copy(LEAK, self.bin / 'check-ops-r1-leak.py')
        self.outbox = r / 'outbox'
        self.outbox.mkdir()
        self.tgz = r / 'drop.tgz'
        self.make_tgz({'result.txt': b'ok'})
        self.env = dict(os.environ, CELERIS_EVAL_ROOT=str(self.eval_root),
                        CELERIS_EVAL_BIN=str(self.bin), CELERIS_EVAL_OUTBOX=str(self.outbox),
                        CELERIS_EVAL_TEST_HOME=str(self.home), CALLER_ONLY='1',
                        PATH='/nonexistent-caller-path')
        self.env.pop('CELERIS_EVAL_TEST_EUID', None)

    def tearDown(self):
        self.tmp.cleanup()

    def make_tgz(self, files):
        with tarfile.open(self.tgz, 'w:gz') as t:
            for n, b in files.items():
                ti = tarfile.TarInfo(n)
                ti.size = len(b)
                t.addfile(ti, io.BytesIO(b))

    def sha(self):
        return hashlib.sha256(self.tgz.read_bytes()).hexdigest()

    def grade(self, sha=None, extra=None, env=None):
        return subprocess.run([sys.executable, '-I', str(GRADE), '--tgz', str(self.tgz),
                               '--sha256', sha or self.sha(), '--case', CASE, *(extra or [])],
                              capture_output=True, text=True, env=dict(self.env, **(env or {})))

    def assert_no_private(self, r):
        for s in (r.stdout, r.stderr):
            self.assertNotIn(SECRET, s)
            self.assertNotIn('private.json', s)
            self.assertNotIn(str(self.eval_root / 'private'), s)

    def test_publishes_verdict_only(self):
        r = self.grade()
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assert_no_private(r)
        files = list(self.outbox.iterdir())
        self.assertEqual(len(files), 1)
        self.assertEqual(r.stdout.strip(), str(files[0]))
        self.assertEqual(files[0].stat().st_mode & 0o777, 0o644)
        self.assertEqual(json.loads(files[0].read_text()), {'pass': True})

    def test_leaking_verdict_is_not_published(self):
        (self.bin / 'leak').write_text('1')
        r = self.grade()
        self.assertEqual(r.returncode, 71, r.stderr)
        self.assert_no_private(r)
        self.assertEqual(list(self.outbox.iterdir()), [])

    def test_sha_mismatch(self):
        r = self.grade(sha='0' * 64)
        self.assertEqual(r.returncode, 65)
        self.assertIn('sha256 mismatch', r.stderr)
        self.assertEqual(list(self.outbox.iterdir()), [])
        self.assertFalse((self.eval_root / 'work').exists())

    def test_other_uid_refused(self):
        r = self.grade(env={'CELERIS_EVAL_TEST_EUID': str(os.geteuid() + 1)})
        self.assertEqual(r.returncode, 77)
        self.assert_no_private(r)
        self.assertEqual(list(self.outbox.iterdir()), [])

    def test_open_eval_root_refused(self):
        os.chmod(self.eval_root, 0o755)
        r = self.grade()
        self.assertEqual(r.returncode, 77)

    def test_unsafe_tgz_member(self):
        self.make_tgz({'../escape.txt': b'x'})
        r = self.grade()
        self.assertEqual(r.returncode, 65)
        self.assertEqual(list(self.outbox.iterdir()), [])

    def test_failing_artifact_verdict_is_published(self):
        # grade の exit 1（不合格）も採点の成功。判定 JSON を outbox に置く。
        self.make_tgz({'other.txt': b'x'})
        r = self.grade()
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(json.loads(Path(r.stdout.strip()).read_text()), {'pass': False})

    def test_failing_verdict_goes_through_leak_check(self):
        # 不合格の判定も漏洩検査を通す。漏れていれば outbox に置かない。
        self.make_tgz({'other.txt': b'x'})
        (self.bin / 'leak').write_text('')
        r = self.grade()
        self.assertEqual(r.returncode, 71)
        self.assertEqual(list(self.outbox.iterdir()), [])
        self.assert_no_private(r)

    def test_campaign_error_publishes_nothing(self):
        # exit 2（判定 JSON なし）は採点の失敗。outbox に何も置かない。
        self.make_tgz({'error.txt': b'x'})
        r = self.grade()
        self.assertEqual(r.returncode, 70)
        self.assertEqual(r.stdout, '')
        self.assertIn('exit 2', r.stderr)
        self.assertEqual(list(self.outbox.iterdir()), [])
        self.assert_no_private(r)

    def test_missing_grade_case_refused(self):
        shutil.rmtree(self.eval_root / 'grade-cases')
        r = self.grade()
        self.assertEqual(r.returncode, 66)
        self.assertEqual(list(self.outbox.iterdir()), [])

    def test_caller_cannot_pass_campaign_args(self):
        # grade-case・private・作業 dir の場所を差し替える引数は受けない（argv は wrapper が組む）。
        for extra in (['--', 'grade', '--case', 'crates/campaign/fixtures/grade-cases/final/x.json'],
                      ['--grader-root', '/x'],
                      ['--grade-case-root', 'crates/campaign/fixtures/grade-cases'],
                      ['--work', '/tmp'],
                      ['--case', 'k2-other']):
            r = self.grade(extra=extra)
            self.assertEqual(r.returncode, 64, (extra, r.stderr))
        self.assertEqual(list(self.outbox.iterdir()), [])


class SetShaTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(dir=os.environ.get('TMPDIR'))
        self.root = Path(self.tmp.name) / 'cases'
        src = HERE.parent.parent / 'crates/campaign/cases/final' / CASE / 'case.json'
        (self.root / 'final' / CASE).mkdir(parents=True)
        self.case = self.root / 'final' / CASE / 'case.json'
        shutil.copy(src, self.case)

    def tearDown(self):
        self.tmp.cleanup()

    def run_set(self, sha, *extra):
        return subprocess.run([sys.executable, '-I', str(SETSHA), '--case', CASE, '--sha256', sha,
                               '--cases-root', str(self.root), *extra], capture_output=True, text=True)

    def test_replaces_only_private_sha(self):
        before = json.loads(self.case.read_text())
        new = 'ab' * 32
        r = self.run_set(new.upper())
        self.assertEqual(r.returncode, 0, r.stderr)
        rep = json.loads(r.stdout)
        self.assertFalse(rep['manifest_refreeze'])
        after = json.loads(self.case.read_text())
        self.assertEqual(after['private_ref']['sha256'], new)
        after['private_ref']['sha256'] = before['private_ref']['sha256']
        self.assertEqual(after, before)

    def test_dry_run_keeps_file(self):
        text = self.case.read_text()
        self.assertEqual(self.run_set('cd' * 32, '--dry-run').returncode, 0)
        self.assertEqual(self.case.read_text(), text)

    def test_refuses_leaked_and_bad_sha(self):
        text = self.case.read_text()
        old = json.loads(text)['private_ref']['sha256']
        self.assertEqual(self.run_set(old).returncode, 65)
        self.assertEqual(self.run_set('xyz').returncode, 64)
        self.assertEqual(self.case.read_text(), text)

    def test_sets_grade_case_sha(self):
        before = json.loads(self.case.read_text())
        r = self.run_set('ab' * 32, '--grade-case-sha256', 'cd' * 32)
        self.assertEqual(r.returncode, 0, r.stderr)
        after = json.loads(self.case.read_text())
        self.assertEqual(after['private_ref']['sha256'], 'ab' * 32)
        self.assertEqual(after['grade_case_ref']['sha256'], 'cd' * 32)
        for k in ('private_ref', 'grade_case_ref'):
            after[k]['sha256'] = before[k]['sha256']
        self.assertEqual(after, before)

    def test_refuses_repo_grade_case_sha(self):
        text = self.case.read_text()
        old = json.loads(text)['grade_case_ref']['sha256']
        r = self.run_set('ab' * 32, '--grade-case-sha256', old)
        self.assertEqual(r.returncode, 65)
        self.assertIn('leaked', r.stderr)
        self.assertEqual(self.case.read_text(), text)

    def test_does_not_read_private(self):
        # private の置き場所が無くても動く（中身も path も読まない）。
        self.assertFalse((self.root.parent / 'private').exists())
        self.assertEqual(self.run_set('ef' * 32).returncode, 0)


if __name__ == '__main__':
    unittest.main()
