"""Exercise the real differential CLI on isolated Git histories."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SOURCE = Path(__file__).resolve().parents[1] / 'dev/check-source-size.py'
spec = importlib.util.spec_from_file_location('size_gate', SOURCE)
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


class GateTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.git('init', '-q')
        self.git('config', 'user.email', 'test@example.com')
        self.git('config', 'user.name', 'test')
        self.write('scripts/dev/source-size-report.toml',
                   '[thresholds]\nproduction_lines=10\ninline_test_lines=5\n')
        self.write('src/lib.rs', '// production\n' * 12)
        self.write('src/inline.rs', '#[cfg(test)]\nmod tests {\n' + '    // test\n' * 6 + '}\n')
        self.git('add', '.')
        self.git('commit', '-qm', 'baseline')
        self.sha = self.git('rev-parse', 'HEAD').strip()
        report = gate.ssr.run_report(str(self.root), 10, 5, [])
        self.baseline = {'version': 1, 'classification': 'source-size-report/1',
                         'sha': self.sha, 'thresholds': report['thresholds'], 'debt': report['warnings']}
        self.save_baseline()

    def git(self, *args):
        return subprocess.run(['git', '-C', str(self.root), *args], check=True,
                              text=True, capture_output=True).stdout

    def write(self, path, content):
        p = self.root / path
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(content)

    def save_baseline(self):
        self.write('scripts/dev/source-size-baseline.json', json.dumps(self.baseline))

    def run_gate(self, expected, base=None):
        result = subprocess.run([sys.executable, str(SOURCE), '--root', str(self.root),
                                 '--base', base or self.sha, '--json'], text=True, capture_output=True)
        self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
        return result.stdout + result.stderr

    def test_unchanged_and_improved_debt_pass(self):
        self.run_gate(0)
        self.write('src/lib.rs', '// production\n' * 11)
        self.run_gate(0)

    def test_worsening_reports_path_rule_threshold_and_both_bases(self):
        self.write('src/lib.rs', '// production\n' * 13)
        out = self.run_gate(1)
        for value in ['src/lib.rs', 'production_size', 'threshold=10', 'baseline=12', 'comparison=12']:
            self.assertIn(value, out)

    def test_new_untracked_violation_fails(self):
        self.write('src/new.rs', '// production\n' * 11)
        self.assertIn('src/new.rs', self.run_gate(1))

    def test_inline_growth_and_new_module_fail(self):
        self.write('src/inline.rs', '#[cfg(test)]\nmod tests {\n' + '    // test\n' * 7 + '}\n')
        self.assertIn('inline_test_size', self.run_gate(1))
        self.write('src/inline.rs', '#[cfg(test)]\nmod other {\n' + '    // test\n' * 6 + '}\n')
        self.assertIn('other', self.run_gate(1))

    def test_regrowth_after_landed_improvement_fails(self):
        self.write('src/lib.rs', '// production\n' * 11)
        self.git('add', '.')
        self.git('commit', '-qm', 'improvement')
        base = self.git('rev-parse', 'HEAD').strip()
        self.write('src/lib.rs', '// production\n' * 12)
        self.assertIn('comparison=11', self.run_gate(1, base))

    def test_reasoned_exception_passes_empty_reason_fails(self):
        path = self.root / 'scripts/dev/source-size-report.toml'
        config = path.read_text()
        self.write('src/lib.rs', '// production\n' * 13)
        exception = '\n[[exceptions]]\npath="src/lib.rs"\nchecks=["production_size"]\nreason="cohesive wire schema"\n'
        path.write_text(config + exception)
        self.run_gate(0)
        path.write_text(config + exception.replace('cohesive wire schema', ''))
        self.assertIn('reason', self.run_gate(2))

    def test_generated_and_external_tests_are_not_production(self):
        for path in ['src/generated/types.rs', 'src/unit_tests.rs', 'tests/example.rs']:
            self.write(path, '// test or generated\n' * 50)
        self.run_gate(0)

    def test_invalid_baseline_or_revision_fails_closed(self):
        self.baseline['debt'][0]['value'] += 10
        self.save_baseline()
        self.assertIn('baseline does not match', self.run_gate(2))
        self.run_gate(2, 'missing-revision')

    def test_deleted_debt_passes(self):
        (self.root / 'src/lib.rs').unlink()
        self.run_gate(0)

    def test_threshold_change_requires_review(self):
        self.write('scripts/dev/source-size-report.toml', '[thresholds]\nproduction_lines=100\n')
        self.assertIn('threshold change', self.run_gate(2))


if __name__ == '__main__':
    unittest.main()
