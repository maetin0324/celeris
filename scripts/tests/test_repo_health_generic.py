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

    def evidence_in_head(self, repo, finding):
        changed = git(repo, 'diff', '--name-only', 'main', 'candidate').splitlines()
        self.assertTrue(finding['rationale'])
        self.assertTrue(any(e['path'] in changed for e in finding['evidence']))
        for e in finding['evidence']:
            self.assertGreater(e['line'], 0)
            self.assertLessEqual(e['line'], len((repo / e['path']).read_text().splitlines()))

    def candidate_found(self, variant, rid):
        """dup.* are review candidates: never fail-grade, never change the verdict."""
        for configured, repo, code, report in self.reports(variant):
            with self.subTest(profile=configured, repo=repo.name, variant=variant):
                findings = [f for f in report['findings'] if f['rule_id'] == rid]
                self.assertEqual(len(findings), 1, report['summary'])
                language = 'rust' if (repo / 'Cargo.toml').exists() else 'typescript'
                key = f'shape:{language}:id,owner,state' if rid == 'dup.contract' else 'table:sessions'
                expected = rid + ':' + hashlib.sha256((rid + '\n' + key).encode()).hexdigest()[:16]
                f = findings[0]
                self.assertEqual(f['fingerprint'], expected)
                self.assertIs(f['candidate'], True)
                self.assertEqual(f['severity'], 'warning')
                self.assertEqual(f['delta'], 'new')
                self.assertIsNone(f['exception'])
                self.assertFalse(fail_grade(f))
                self.evidence_in_head(repo, f)
                if rid == 'dup.contract':
                    self.assertEqual({e['symbol'] for e in f['evidence']}, {'Session', 'SessionView'})
                self.assertEqual(code, 0, report['summary'])
                self.assertEqual(report['summary']['verdict'], 'pass')
                self.assertEqual(report['summary']['counts']['fail_grade'], 0)

    def test_generic_dup_contract_renamed_detected(self):
        self.candidate_found('dup-contract', 'dup.contract')

    def test_generic_dup_state_detected(self):
        self.candidate_found('dup-state', 'dup.state')

    def test_generic_dep_inversion_detected(self):
        """With layers declared in the base profile the inversion is fail-grade."""
        for configured, repo, code, report in self.reports('dep-inversion'):
            if not configured:
                continue
            with self.subTest(repo=repo.name):
                findings = [f for f in report['findings'] if f['rule_id'] == 'deps.violation']
                self.assertTrue(findings, report['summary'])
                for f in findings:
                    self.assertEqual(f['delta'], 'new')
                    self.assertTrue(fail_grade(f), f)
                    self.evidence_in_head(repo, f)
                self.assertEqual(code, 1, report['summary'])
                self.assertEqual(report['summary']['verdict'], 'fail')

    def test_generic_dep_inversion_without_layers_not_applicable(self):
        """No layer declaration: deps.violation is not_applicable (no_layers_declared).

        Other rules are reported as measured: a Rust path dependency from core to
        app is a cycle (deps.cycle, fail-grade); the TypeScript import is not.
        """
        for configured, repo, code, report in self.reports('dep-inversion'):
            if configured:
                continue
            with self.subTest(repo=repo.name):
                rules = {r['id']: r for r in report['rules']}
                self.assertEqual(rules['deps.violation']['status'], 'not_applicable')
                self.assertTrue(rules['deps.violation']['reason'].startswith('no_layers_declared'))
                self.assertFalse([f for f in report['findings'] if f['rule_id'] == 'deps.violation'])
                if (repo / 'Cargo.toml').exists():
                    cycles = [f for f in report['findings'] if f['rule_id'] == 'deps.cycle']
                    self.assertEqual(len(cycles), 1)
                    self.assertTrue(fail_grade(cycles[0]))
                    self.assertEqual(code, 1, report['summary'])
                else:
                    self.assertEqual(code, 0, report['summary'])
                    self.assertEqual(report['summary']['verdict'], 'pass')

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
