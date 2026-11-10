"""Non-project-specific histories, evaluated by the real checker and validator.

These histories complement rule unit tests: exercise base policy resolution,
full report aggregation, and independent repos without mocks or rule filters.
"""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from repo_health_fixtures import build, git, KINDS

ROOT = Path(__file__).resolve().parents[2]
TOOL = ROOT / 'scripts/dev/repo-health/repo_health.py'
VALIDATOR = TOOL.with_name('validate_report.py')


def fail_grade(f):
    return f['severity'] == 'error' and f['delta'] in ('new', 'worsened') and f['exception'] is None


class GenericHealthTests(unittest.TestCase):
    def reports(self, variant):
        for kind in KINDS:
            for configured in (False, True):
                temporary = tempfile.TemporaryDirectory(prefix='orchard-', dir=os.environ.get('TMPDIR'))
                self.addCleanup(temporary.cleanup)
                root = Path(temporary.name)
                fixture = build(kind, variant, root / 'project', with_profile=configured)
                for index, rel in enumerate(fixture.repos):
                    repo = root / 'project' / rel
                    self.assertNotIn('celeris', repo.name.lower())
                    out = root / f'report-{index}.json'
                    proc = subprocess.run([sys.executable, '-I', str(TOOL), '--repo', str(repo),
                                           '--base', 'main', '--head', 'candidate',
                                           '--policy-version', '1', '--out', str(out)],
                                          text=True, capture_output=True)
                    self.assertTrue(out.exists(), proc.stderr + proc.stdout)
                    check = subprocess.run([sys.executable, '-I', str(VALIDATOR), str(out)],
                                           text=True, capture_output=True)
                    self.assertEqual(check.returncode, 0, check.stdout + check.stderr)
                    report = json.loads(out.read_text())
                    self.assertEqual(report['policy']['source'], 'base' if configured else 'builtin')
                    self.assertEqual(git(repo, 'status', '--porcelain'), '')
                    self.assertEqual(git(repo, 'rev-parse', 'HEAD'), git(repo, 'rev-parse', 'candidate'))
                    yield configured, repo, proc.returncode, report

    def test_generic_good_change_passes(self):
        for _, _, code, report in self.reports('good'):
            self.assertEqual(code, 0, report['summary'])
            self.assertEqual(report['summary']['verdict'], 'pass')
            self.assertEqual(report['summary']['counts']['fail_grade'], 0)

    def detected(self, variant, rid):
        for configured, repo, code, report in self.reports(variant):
            with self.subTest(profile=configured, repo=repo.name, variant=variant):
                findings = [f for f in report['findings'] if f['rule_id'] == rid]
                self.assertTrue(findings, report['summary'])
                if rid in ('dup.contract', 'dup.state'):
                    self.assertEqual(len(findings), 1)
                    language = 'rust' if (repo / 'Cargo.toml').exists() else 'typescript'
                    key = f'shape:{language}:id,owner,state' if rid == 'dup.contract' else 'table:sessions'
                    expected = rid + ':' + hashlib.sha256((rid + '\n' + key).encode()).hexdigest()[:16]
                    self.assertEqual(findings[0]['fingerprint'], expected)
                    if rid == 'dup.contract':
                        self.assertEqual({e['symbol'] for e in findings[0]['evidence']}, {'Session', 'SessionView'})
                changed = git(repo, 'diff', '--name-only', 'main', 'candidate').splitlines()
                for f in findings:
                    self.assertEqual(f['delta'], 'new')
                    self.assertTrue(f['rationale'])
                    self.assertTrue(any(e['path'] in changed for e in f['evidence']))
                    for e in f['evidence']:
                        self.assertGreater(e['line'], 0)
                        self.assertLessEqual(e['line'], len((repo / e['path']).read_text().splitlines()))
                if configured or rid == 'deps.violation':
                    self.assertEqual(code, 1, report['summary'])
                    self.assertTrue(any(fail_grade(f) for f in findings), findings)
                else:
                    self.assertEqual(code, 0, report['summary'])

    def test_generic_dup_contract_renamed_detected(self):
        self.detected('dup-contract', 'dup.contract')

    def test_generic_dup_state_detected(self):
        self.detected('dup-state', 'dup.state')

    def test_generic_dep_inversion_detected(self):
        self.detected('dep-inversion', 'deps.violation')

    def test_generic_security_double_check_kept(self):
        for configured, repo, code, report in self.reports('security-double'):
            self.assertEqual(code, 0, report['summary'])
            findings = [f for f in report['findings'] if f['rule_id'] == 'dup.contract']
            self.assertTrue(findings)
            self.assertFalse(any(fail_grade(f) for f in findings))
            if configured:
                self.assertTrue(all(f['exception'] is not None for f in findings))
                self.assertTrue(all('security' in f['exception']['reason'] for f in findings))
            self.assertTrue(any('broker' in e['path'] and (repo / e['path']).exists()
                                for f in findings for e in f['evidence']))

    def test_generic_vendor_generated_kept(self):
        for configured, repo, code, report in self.reports('vendor-generated'):
            self.assertEqual(code, 0, report['summary'])
            self.assertFalse(any(fail_grade(f) for f in report['findings']))
            if configured:
                self.assertFalse([f for f in report['findings'] if f['rule_id'] == 'dup.contract'])
            self.assertTrue(list(repo.glob('vendor/lib/*')))
            self.assertTrue(list(repo.glob('**/api.*')))


if __name__ == '__main__':
    unittest.main()
