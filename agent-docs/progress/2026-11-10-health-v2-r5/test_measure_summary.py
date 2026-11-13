#!/usr/bin/env python3
"""Fixture tests: reject incomparable/failed stages, preserve retries and exact tables.
Existing inventory/case tests do not exercise measurement admission or rendering.
"""
import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('measure_summary', Path(__file__).with_name('measure_summary.py'))
original_read = Path.read_text
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)


def fixture():
    # JSON round-trip ensures tests use the persisted input representation.
    stage = dict(exit=0, secs=10, loadavg_before='10 0 0', loadavg_after='12 0 0', load_wait=dict(ready=True, waited_secs=0))
    edit = dict(stage, settle_exit=0, list_exit=0, rebuild_exit=0, doc_test_exit=0, edit_to_verify_with_doc_secs=11)
    del edit['exit']
    return json.loads(json.dumps(dict(schema='health-v2-r5-build-compare/2', attempts=[dict(attempt=1, crates=['a'], pairs=dict(a=dict(build=[[stage, stage]], edit_verify=[[edit, edit]])))])))


class Decisions(unittest.TestCase):
    def test_complete(self):
        self.assertTrue(m.require(fixture(), ['a']))
        self.assertFalse(m.require(fixture(), ['b']))

    def test_ratio_boundary_both_directions(self):
        for side in (0, 1):
            d = fixture()
            v = d['attempts'][0]['pairs']['a']['build'][0][side]
            v['loadavg_after'] = '24 0 0'
            self.assertFalse(m.require(d, ['a']))
            self.assertEqual(m.comparisons(d)[0]['ratio'], 2)
            v['loadavg_after'] = '23.999 0 0'
            self.assertTrue(m.require(d, ['a']))

    def test_failure_wait_and_missing(self):
        for change in ({'exit': 1}, {'load_wait': {'ready': False}}, {'loadavg_before': 'nan'}, {'loadavg_after': '0 0 0', 'loadavg_before': '0 0 0'}):
            d = fixture()
            d['attempts'][0]['pairs']['a']['build'][0][0].update(change)
            self.assertFalse(m.require(d, ['a']))
        d = fixture()
        del d['attempts'][0]['pairs']['a']['edit_verify']
        self.assertFalse(m.require(d, ['a']))

    def test_retry_preserves_excluded(self):
        d = fixture()
        good = copy.deepcopy(d['attempts'][0])
        good['attempt'] = 2
        d['attempts'][0]['pairs']['a']['build'][0][0]['exit'] = 1
        d['attempts'].append(good)
        self.assertTrue(m.require(d, ['a']))
        self.assertEqual(len(m.comparisons(d)), 4)
        self.assertIn('excluded', m.summary([d]))

    def test_doc_failure(self):
        d = fixture()
        d['attempts'][0]['pairs']['a']['edit_verify'][0][0]['doc_test_exit'] = 1
        self.assertFalse(m.require(d, ['a']))

    def test_run_counts_names_and_skip(self):
        r = m.parse_run(' PASS [ 0.1s] bin test_a\n FAIL [ 0.2s] bin test_b\n SKIP [ 0.0s] bin test_c\n Summary [1s] 2 tests run: 1 passed, 1 failed, 1 skipped')
        self.assertEqual((r['tests_run'], r['skipped_tests']), (2, 1))
        self.assertEqual(r['failed_tests'], ['bin test_b'])

    def test_flaky_and_bundle_identity(self):
        self.assertEqual(m.flaky([{'outcomes': {'old a': 'PASS'}}, {'outcomes': {'new a': 'PASS'}}, {'outcomes': {'new a': 'PASS'}}, {'outcomes': {'old a': 'PASS'}}]), [])
        self.assertEqual(m.flaky([{'outcomes': {'old a': 'PASS'}}, {'outcomes': {'new old::a': 'FAIL'}}, {'outcomes': {'new old::a': 'FAIL'}}, {'outcomes': {'old a': 'PASS'}}]), ['old a'])
        self.assertEqual(m.flaky([{'outcomes': {'bin a': v}} for v in ('PASS', 'FAIL', 'PASS', 'PASS')]), ['bin a'])

    def test_run_requires_run(self):
        d = fixture()
        d['schema'] = 'health-v2-r5-run-compare/2'
        self.assertFalse(m.require(d, ['a']))
        p = d['attempts'][0]['pairs']['a']
        p['run'] = p['build']
        self.assertTrue(m.require(d, ['a']))

    def test_summary_exact_bytes_and_validation(self):
        with tempfile.TemporaryDirectory() as tmp:
            p, md = Path(tmp) / 'fixture.json', Path(tmp) / 'report.md'
            m.write(p, fixture())
            md.write_text('prefix\n' + m.summary([fixture()]) + 'suffix\n')
            command = ['python3', str(Path(m.__file__)), 'summary', '--check', str(md), str(p)]
            self.assertEqual(subprocess.run(command).returncode, 0)
            md.write_bytes(md.read_bytes().replace(b'10', b'10.0'))
            self.assertEqual(subprocess.run(command).returncode, 1)
            self.assertEqual(subprocess.run(['python3', str(Path(m.__file__)), 'validate', str(p)]).returncode, 0)

    def test_compare_retries_preserve_every_attempt(self):
        calls = []
        def command(argv, **kwargs):
            calls.append(argv)
            pairs = fixture()['attempts'][0]['pairs']['a']
            sides = {side: {'crates': {'a': {k: copy.deepcopy(v[0][i]) for k, v in pairs.items()}}} for i, side in enumerate(('base', 'head'))}
            if len(calls) == 1:
                sides['head']['crates']['a']['build']['loadavg_after'] = '24 0 0'
            Path(argv[3]).write_text(json.dumps({'sides': sides}))
            return subprocess.CompletedProcess(argv, 0)
        with tempfile.TemporaryDirectory() as tmp, patch.object(m.subprocess, 'check_output', return_value='revision\n'), patch.object(m.subprocess, 'run', side_effect=command):
            out = Path(tmp) / 'compare.json'
            self.assertEqual(m.execute(out, ['a']), 0)
            d = json.loads(out.read_text())
            self.assertEqual(len(d['attempts']), 2)
            self.assertTrue(d['comparisons'][0]['excluded'])
            self.assertFalse(d['comparisons'][2]['excluded'])

    def test_shell_routes_without_measurements(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / 'fixture.json'
            m.write(out, fixture())
            shell = ['bash', str(Path(m.__file__).with_name('measure.sh'))]
            self.assertEqual(subprocess.run(shell + ['--validate', str(out)]).returncode, 0)
            self.assertEqual(subprocess.run(shell + ['--require-compare', str(out), 'a']).returncode, 0)
            self.assertEqual(subprocess.run(shell + ['--require-compare', str(out), 'missing']).returncode, 1)

    def test_run_orchestration_without_cargo(self):
        commands = []
        def command(argv, **kwargs):
            commands.append((argv, str(kwargs.get('cwd', ''))))
            if argv[:3] == ['git', 'worktree', 'add']:
                Path(argv[4]).mkdir()
            if argv[:3] == ['cargo', 'nextest', 'run'] and '--no-run' not in argv:
                kwargs['stdout'].write(' PASS [ 0.1s] bin test_a\n Summary [1s] 1 tests run: 1 passed, 0 skipped\n')
            return subprocess.CompletedProcess(argv, 0)
        with tempfile.TemporaryDirectory() as tmp, patch.dict(m.os.environ, {'TMPDIR': tmp}), patch.object(m.subprocess, 'check_output', return_value='revision\n'), patch.object(m.subprocess, 'run', side_effect=command), patch.object(m, 'wait_load', return_value={'ready': True, 'waited_secs': 0, 'loadavg': '12 0 0'}), patch.object(m.Path, 'read_text', autospec=True, side_effect=lambda p, *a, **kw: '12 0 0' if str(p) == '/proc/loadavg' else original_read(p, *a, **kw)):
            out = Path(tmp) / 'run.json'
            self.assertEqual(m.execute(out, ['celeris'], True), 0)
            d = json.loads(out.read_text())
            self.assertEqual(d['attempts'][0]['flaky']['celeris'], [])
        runs = [(a, cwd) for a, cwd in commands if a[:3] == ['cargo', 'nextest', 'run'] and '--no-run' not in a]
        self.assertEqual([cwd.endswith('/base') for _, cwd in runs], [True, False, False, True])
        self.assertEqual(sum(a[:4] == ['cargo', 'build', '-p', 'celeris-campaign'] for a, _ in commands), 4)
        self.assertEqual(sum('--no-run' in a for a, _ in commands), 4)

    def test_wait_sleeps_and_times_out(self):
        with patch.dict(m.os.environ, {'MEASURE_LOAD_WAIT': '5', 'MEASURE_MAX_LOAD': '24'}), patch.object(m.Path, 'read_text', return_value='30 0 0'), patch.object(m.time, 'monotonic', side_effect=[0, 0, 5]), patch.object(m.time, 'sleep') as sleep:
            result = m.wait_load()
            self.assertFalse(result['ready'])
            self.assertEqual(result['waited_secs'], 5)
            sleep.assert_called_once_with(5)


def celerisctl_fixture():
    # Shape of meas-edit-a's record: base test-affected ran `cargo test --doc -p celerisctl`,
    # which exits 101 for a crate without a lib target; HEAD's doc test passed.
    d = fixture()
    d['attempts'][0]['crates'] = ['celerisctl']
    pairs = d['attempts'][0]['pairs'].pop('a')
    for side, secs in enumerate((6.18, 30.55)):
        pairs['edit_verify'][0][side].update(edit_to_verify_secs=secs - 1, edit_to_verify_with_doc_secs=secs)
    pairs['edit_verify'][0][0]['doc_test_exit'] = 101
    d['attempts'][0]['pairs']['celerisctl'] = pairs
    return d


class Items(unittest.TestCase):
    """--require-compare <crate>:<item> and exclusion reasons (rules unchanged)."""

    def shell(self, path, spec):
        return subprocess.run(['bash', str(Path(m.__file__).with_name('measure.sh')), '--require-compare', str(path), spec],
                              capture_output=True).returncode

    def test_celerisctl_doc_test_101_items(self):
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp) / 'edit-a.json'
            m.write(p, celerisctl_fixture())
            expected = {'celerisctl:build': 0, 'celerisctl': 1, 'celerisctl:edit_verify': 1,
                        'celerisctl:build,celerisctl:edit_verify': 1, 'celerisctl:build,other:build': 1,
                        'celerisctl:run': 2, 'celerisctl:unknown': 2, ':build': 2, 'celerisctl:': 2, 'celerisctl,': 2}
            self.assertEqual({spec: self.shell(p, spec) for spec in expected}, expected)
            self.assertEqual(subprocess.run(['python3', str(Path(m.__file__)), 'validate', str(p)]).returncode, 0)
        table = m.summary([celerisctl_fixture()])
        self.assertIn('| celerisctl | build | 1/1 | 10 | 10 | - | - | 1 | included | - |', table)
        self.assertIn('| celerisctl | edit_verify | 1/1 | 6.18 | 30.55 | 5.18 | 29.55 | 1 | excluded | base doc_test_exit 101 |', table)

    def test_require_items_in_schema(self):
        d = celerisctl_fixture()
        self.assertTrue(m.require(d, ['celerisctl:build']))
        self.assertFalse(m.require(d, ['celerisctl:edit_verify']))
        with self.assertRaises(ValueError):
            m.require(d, ['celerisctl:run'])
        d = fixture()
        d['schema'] = 'health-v2-r5-run-compare/2'
        p = d['attempts'][0]['pairs']['a']
        p['run'] = p['build']
        self.assertTrue(m.require(d, ['a:run']))
        for spec in ('a:build', 'a:edit_verify'):
            with self.assertRaises(ValueError):
                m.require(d, [spec])

    def test_reasons(self):
        cases = [
            ({'loadavg_after': '24 0 0'}, {}, 'load ratio >= 2'),
            ({'loadavg_after': '0 0 0', 'loadavg_before': '0 0 0'}, {}, 'load ratio >= 2'),
            ({'load_wait': {'ready': False}}, {}, 'load wait expired'),
            ({}, {'exit': 125}, 'head exit 125'),
            ({'exit': None}, {}, 'missing or invalid measurement'),
            ({'secs': -1}, {}, 'missing or invalid measurement'),
            ({'loadavg_before': 'nan'}, {}, 'missing or invalid measurement'),
            ({'loadavg_after': '30 0 0', 'load_wait': {'ready': False}, 'exit': 1}, {'secs': None},
             'load ratio >= 2; load wait expired; base exit 1; missing or invalid measurement'),
        ]
        for base, head, reason in cases:
            d = fixture()
            b, h = d['attempts'][0]['pairs']['a']['build'][0]
            b.update(base)
            h.update(head)
            row = m.comparisons(d)[0]
            self.assertEqual((row['excluded'], row['reason']), (True, reason), (base, head))
            self.assertIn(f'| excluded | {reason} |', m.summary([d]))
        d = fixture()
        d['attempts'][0]['pairs']['a']['edit_verify'][0][1].update(list_exit=2, doc_test_exit=101)
        self.assertEqual(m.comparisons(d)[1]['reason'], 'head list_exit 2; head doc_test_exit 101')
        self.assertEqual([r['reason'] for r in m.comparisons(fixture())], [None, None])

    def test_compare_takes_crate_names_only(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(m.subprocess, 'run') as run, patch.object(m.subprocess, 'check_output') as out:
            for crates in (['celerisctl:build'], ['']):
                with self.assertRaises(ValueError):
                    m.execute(Path(tmp) / 'x.json', crates)
            run.assert_not_called()
            out.assert_not_called()


if __name__ == '__main__':
    unittest.main()
