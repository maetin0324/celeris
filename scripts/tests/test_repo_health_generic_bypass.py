"""Judgement and bypass resistance of the real checker on isolated, non-project histories.

ADR 2026-11-10-health-generic-fixtures (3), unit py-bypass: (d) docs-only, non-Git, unsupported and
tool faults; (f) a candidate that weakens the policy, rewrites the baseline or hides the change in an
earlier commit; and the binding of a report to candidate, base and policy.

Invariants guarded here and why the existing tests do not catch them: the rule unit tests
(test_repo_health_core.py and friends) run fake rules or a single rule on inline repos.  They do not
show that the shipped rule set, run end to end on an ordinary TypeScript/Rust/multi-repo history,
keeps a layer violation blocking while the candidate edits the policy that would hide it.  Every
weakening below is first shown to be effective (it hides the violation once it is in the base), so a
test cannot pass because the edit did nothing.
"""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

from repo_health_fixtures import BASELINE, KINDS, PROFILE, WEAKENINGS, build, commit, git, write

ROOT = Path(__file__).resolve().parents[2]
TOOL_DIR = ROOT / 'scripts/dev/repo-health'
TOOL = TOOL_DIR / 'repo_health.py'
VALIDATOR = TOOL_DIR / 'validate_report.py'
STRUCTURE = ('deps.violation', 'deps.cycle', 'dup.contract', 'dup.state', 'dup.public-surface')


def fail_grade(f):
    return f['severity'] == 'error' and f['delta'] in ('new', 'worsened') and f['exception'] is None


def definition_sha256(files):
    """--print-review-policy digest: <path>\\0<bytes>\\0<content> per profile file (health/profile.py)."""
    h = hashlib.sha256()
    for path, data in files:
        h.update(path.encode() + b'\0' + str(len(data)).encode() + b'\0' + data)
    return h.hexdigest()


class GenericBypassTests(unittest.TestCase):
    def tmp(self):
        temporary = tempfile.TemporaryDirectory(prefix='orchard-', dir=os.environ.get('TMPDIR'))
        self.addCleanup(temporary.cleanup)
        return Path(temporary.name)

    def fixtures(self, variant, kinds=KINDS, with_profile=True):
        """(kind, repo) for every repo of every kind; multi yields its ts and rust repos."""
        for kind in kinds:
            root = self.tmp()
            fixture = build(kind, variant, root / 'project', with_profile=with_profile)
            for rel in fixture.repos:
                repo = root / 'project' / rel
                self.assertNotIn('celeris', str(repo.relative_to(root)).lower())
                yield kind, repo

    def check(self, repo, base='main', head='candidate', tool=TOOL, valid=True, env=None):
        out = repo.parent / f'{repo.name}-{base.replace("~", "_")}-report.json'
        out.unlink(missing_ok=True)
        proc = subprocess.run([sys.executable, '-I', str(tool), '--repo', str(repo), '--base', base,
                               '--head', head, '--policy-version', '1', '--out', str(out)],
                              text=True, capture_output=True, env=env)
        report = json.loads(out.read_text()) if out.exists() else None
        if report is not None and valid:
            check = subprocess.run([sys.executable, '-I', str(VALIDATOR), str(out)], text=True, capture_output=True)
            self.assertEqual(check.returncode, 0, check.stdout + check.stderr)
        return proc, report

    def review_policy(self, repo, base='main'):
        proc = subprocess.run([sys.executable, '-I', str(TOOL), '--print-review-policy', '--repo', str(repo),
                               '--base', base], text=True, capture_output=True)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        return json.loads(proc.stdout)

    @staticmethod
    def rules(report):
        return {r['id']: r for r in report['rules']}

    def checker_without(self, module):
        """A copy of the checker in $TMPDIR that lacks health/<module> (the shipped checker is not touched)."""
        partial = self.tmp() / 'tool'
        shutil.copytree(TOOL_DIR, partial)
        (partial / 'health' / module).unlink()
        return partial / 'repo_health.py'

    def env_with_adr_tool(self):
        """Environment whose PATH has a passing `orchard-adr-lint`, so only the absent script of doc.links is left."""
        bin_dir = self.tmp() / 'bin'
        bin_dir.mkdir()
        stub = bin_dir / 'orchard-adr-lint'
        stub.write_text('#!/bin/sh\nexit 0\n')
        stub.chmod(0o755)
        return dict(os.environ, PATH=str(bin_dir) + os.pathsep + os.environ.get('PATH', ''))

    # ------------------------------------------------------------------ (d)

    def test_generic_docs_only_not_applicable(self):
        for configured in (True, False):
            for kind, repo in self.fixtures('docs-only', with_profile=configured):
                with self.subTest(kind=kind, repo=repo.name, profile=configured):
                    changed = git(repo, 'diff', '--name-only', 'main', 'candidate').splitlines()
                    self.assertEqual(sorted(changed), ['README.md', str(Path('docs') / 'guide.md')])
                    proc, report = self.check(repo)
                    self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
                    self.assertEqual(report['inputs']['mode'], 'doc-only')
                    rules = self.rules(report)
                    for rid in STRUCTURE:
                        self.assertEqual(rules[rid]['status'], 'not_applicable', rules[rid])
                        self.assertTrue(rules[rid]['reason'].startswith('doc_only'), rules[rid])
                    self.assertFalse([f for f in report['findings'] if f['rule_id'] in STRUCTURE])
                    self.assertEqual(report['summary']['verdict'], 'pass')

    def test_generic_non_git_error(self):
        for configured in (True, False):
            for kind, repo in self.fixtures('non-git', with_profile=configured):
                with self.subTest(kind=kind, repo=repo.name, profile=configured):
                    self.assertFalse((repo / '.git').exists())
                    self.assertTrue((repo / 'README.md').exists())
                    probe = subprocess.run(['git', '-C', str(repo), 'rev-parse', '--show-toplevel'],
                                           capture_output=True, text=True)
                    self.assertNotEqual(probe.returncode, 0, 'the fixture must not sit inside another repo')
                    out = repo.parent / f'{repo.name}-report.json'
                    proc = subprocess.run([sys.executable, '-I', str(TOOL), '--repo', str(repo), '--base', 'main',
                                           '--head', 'candidate', '--policy-version', '1', '--out', str(out)],
                                          text=True, capture_output=True)
                    self.assertEqual(proc.returncode, 2, proc.stdout + proc.stderr)
                    self.assertIn('not_a_git_repo', proc.stderr)
                    self.assertFalse(out.exists())

    def test_generic_unsupported_required_not_pass(self):
        """A good change cannot pass while a required wrapped check cannot run (ADR (2) unsupported-tool).

        The tool that is not on PATH (adr.numbers) is unsupported and makes the run an error (exit 2).  The
        absent script of doc.links is then judged alone (orchard-adr-lint stubbed on PATH): it must not pass
        either.  Its exact status (unsupported, not fail) is the checker gap G2, kept in
        test_generic_missing_script_unsupported.
        """
        env = self.env_with_adr_tool()
        for kind, repo in self.fixtures('unsupported-tool'):
            with self.subTest(kind=kind, repo=repo.name, missing='tool'):
                proc, report = self.check(repo)
                self.assertEqual(proc.returncode, 2, proc.stdout + proc.stderr)
                self.assertEqual(report['summary']['verdict'], 'error')
                tool = self.rules(report)['adr.numbers']
                self.assertTrue(tool['required'])
                self.assertEqual((tool['status'], tool['reason']), ('unsupported', 'tool_missing:orchard-adr-lint'))
            with self.subTest(kind=kind, repo=repo.name, missing='script'):
                proc, report = self.check(repo, env=env)
                self.assertNotEqual(proc.returncode, 0, proc.stdout + proc.stderr)
                self.assertNotEqual(report['summary']['verdict'], 'pass')
                rules = self.rules(report)
                # The stub really removes the missing tool, so only the absent script is left to judge.
                self.assertEqual(rules['adr.numbers']['status'], 'pass', rules['adr.numbers'])
                script = rules['doc.links']
                self.assertTrue(script['required'])
                self.assertNotIn(script['status'], ('pass', 'not_applicable'), script)

    @unittest.expectedFailure
    def test_generic_missing_script_unsupported(self):
        """G2: a required wrapped check whose script is absent is unsupported (command_missing), not a finding.

        Expected by ADR 2026-11-10-health-generic-fixtures (2) (unsupported-tool) and the docstring of
        rules/existing_checks.py (a script file that is absent, exit 126/127, is unsupported).  Actual: /bin/sh
        is dash, which exits 2 for a script it cannot open, so doc.links becomes fail with one fail-grade
        finding, and Celeris would hand the absent script to repair as a finding to fix.  The run does not
        pass either way (test_generic_unsupported_required_not_pass).  Unresolved G2 in
        agent-docs/progress/2026-11-10-health-v2-g4/py-bypass.md; owner rules/existing_checks.py, a policy
        file this unit does not change.  Remove expectedFailure when the policy-only task fixes G2; until
        then an unexpected success reports that the decorator is stale.
        """
        env = self.env_with_adr_tool()
        for kind, repo in self.fixtures('unsupported-tool'):
            for stubbed in (False, True):
                with self.subTest(kind=kind, repo=repo.name, adr_tool_stubbed=stubbed):
                    proc, report = self.check(repo, env=env if stubbed else None)
                    self.assertEqual(proc.returncode, 2, proc.stdout + proc.stderr)
                    self.assertEqual(report['summary']['verdict'], 'error')
                    script = self.rules(report)['doc.links']
                    self.assertTrue(script['required'])
                    self.assertEqual(script['status'], 'unsupported', script)
                    self.assertTrue(script['reason'].startswith('command_missing'), script)
                    self.assertEqual(report['summary']['counts']['fail_grade'], 0, report['findings'])

    def test_generic_tool_failure_is_error(self):
        # A wrapped check that breaks (output is not UTF-8) is an error, not a pass or a finding.
        for kind, repo in self.fixtures('tool-failure'):
            with self.subTest(kind=kind, repo=repo.name, fault='wrapped-check'):
                proc, report = self.check(repo)
                self.assertEqual(proc.returncode, 2, proc.stdout + proc.stderr)
                self.assertEqual(report['summary']['verdict'], 'error')
                rule = self.rules(report)['doc.links']
                self.assertEqual(rule['status'], 'error', rule)
                self.assertTrue(rule['reason'].startswith('crashed:doc.links'), rule)
        # An adapter that raises (copy of the checker whose dependency adapter is broken) is an error.
        broken = self.tmp() / 'tool'
        shutil.copytree(TOOL_DIR, broken)
        with (broken / 'health/lang_deps.py').open('a', encoding='utf-8') as fh:
            fh.write('\n\ndef build(ctx, tree):\n    raise RuntimeError("adapter fault injected by the test")\n')
        for kind, repo in self.fixtures('good', kinds=('ts', 'rust')):
            with self.subTest(kind=kind, fault='adapter'):
                proc, report = self.check(repo, tool=broken / 'repo_health.py')
                self.assertEqual(proc.returncode, 2, proc.stdout + proc.stderr)
                self.assertEqual(report['summary']['verdict'], 'error')
                rules = self.rules(report)
                for rid in ('deps.violation', 'deps.cycle'):
                    self.assertEqual(rules[rid]['status'], 'error', rules[rid])
                    self.assertEqual(rules[rid]['reason'], f'crashed:{rid}:RuntimeError')
        # A checker copy that lacks a health/ module writes no report and does not exit 0, so it cannot
        # pass.  The exact exit code (2, an error) is the checker gap G1, kept in
        # test_generic_checker_import_failure_is_error.
        partial = self.checker_without('scan.py')
        for kind, repo in self.fixtures('good', kinds=('ts',)):
            with self.subTest(kind=kind, fault='missing-module'):
                proc, report = self.check(repo, tool=partial)
                self.assertIsNone(report)
                self.assertNotEqual(proc.returncode, 0, proc.stderr[-400:])

    @unittest.expectedFailure
    def test_generic_checker_import_failure_is_error(self):
        """G1: a checker that cannot import its own module is an error: exit 2 and no report.

        Expected by ADR 2026-11-10-health-generic-fixtures (3) table (d) and the report contract (exit 1 is
        only a required rule that fails; a fault of the checker is an error, exit 2).  Actual: the copy
        without health/scan.py dies with a ModuleNotFoundError traceback, which Python turns into exit 1, so
        the CLI and check-health.sh show a fail instead of an error.  It does not pass either way (no report,
        non-zero exit: test_generic_tool_failure_is_error).  Unresolved G1 in
        agent-docs/progress/2026-11-10-health-v2-g4/py-bypass.md; owner repo_health.py (its imports and
        main), a policy file this unit does not change.  Remove expectedFailure when the policy-only task
        fixes G1; until then an unexpected success reports that the decorator is stale.
        """
        partial = self.checker_without('scan.py')
        for kind, repo in self.fixtures('good', kinds=('ts',)):
            with self.subTest(kind=kind, fault='missing-module'):
                proc, report = self.check(repo, tool=partial)
                self.assertIsNone(report)
                self.assertEqual(proc.returncode, 2, proc.stderr[-400:])

    # ------------------------------------------------------------------ (f)

    def approve_head_policy(self, repo, paths):
        """Branch `approved` = main + the candidate's policy files (the weakening, as if it had been approved)."""
        git(repo, 'switch', '-q', '-c', 'approved', 'main')
        git(repo, 'checkout', 'candidate', '--', *paths)
        commit(repo, 'approve the candidate policy')
        git(repo, 'switch', '-q', 'candidate')

    def test_generic_policy_weakening_fails(self):
        for variant in WEAKENINGS:
            policy_file = BASELINE if variant == 'baseline-rewrite' else PROFILE
            for kind, repo in self.fixtures(variant):
                with self.subTest(variant=variant, kind=kind, repo=repo.name):
                    changed = set(git(repo, 'diff', '--name-only', 'main', 'candidate').splitlines())
                    self.assertIn(policy_file, changed)
                    base_profile = git(repo, 'show', f'main:{PROFILE}').encode() + b'\n'
                    head_profile = (repo / PROFILE).read_bytes()
                    self.assertEqual(head_profile != base_profile, variant != 'baseline-rewrite')
                    proc, report = self.check(repo)
                    self.assertEqual(proc.returncode, 1, proc.stdout + proc.stderr)
                    self.assertEqual(report['summary']['verdict'], 'fail')
                    self.assertEqual(report['policy']['source'], 'base')
                    self.assertTrue(report['policy']['changed'])
                    pc = [f for f in report['findings'] if f['rule_id'] == 'policy.changed']
                    self.assertEqual(len(pc), 1)
                    self.assertTrue(fail_grade(pc[0]), pc[0])
                    self.assertEqual([e['path'] for e in pc[0]['evidence']], [policy_file])
                    rule = self.rules(report)['deps.violation']
                    self.assertTrue(rule['required'], 'required comes from the base profile')
                    self.assertEqual(rule['status'], 'fail', rule)
                    dv = [f for f in report['findings'] if f['rule_id'] == 'deps.violation']
                    self.assertEqual(len(dv), 1, dv)
                    self.assertTrue(fail_grade(dv[0]), dv[0])
                    self.assertTrue(dv[0]['evidence'][0]['path'].split('/')[1] == 'core', dv[0])
                    # The review policy comes from the base commit, never from the checked-out head.
                    self.assertEqual(git(repo, 'rev-parse', '--abbrev-ref', 'HEAD'), 'candidate')
                    got = self.review_policy(repo)['definition_sha256']
                    self.assertEqual(got, definition_sha256([(PROFILE, base_profile)]))
                    if variant != 'baseline-rewrite':
                        self.assertNotEqual(got, definition_sha256([(PROFILE, head_profile)]))
                    # Control: once the weakening is in the base it hides the violation, so the
                    # failure above comes from evaluating the base policy, not from a no-op edit.
                    self.approve_head_policy(repo, [policy_file])
                    proc, approved = self.check(repo, base='approved')
                    self.assertFalse(approved['policy']['changed'])
                    if variant != 'baseline-rewrite':
                        rule = self.rules(approved)['deps.violation']
                        blocking = rule['required'] and any(
                            fail_grade(f) for f in approved['findings'] if f['rule_id'] == 'deps.violation')
                        self.assertFalse(blocking, (variant, rule))

    def test_generic_history_smuggle_fails(self):
        for kind, repo in self.fixtures('history-smuggle'):
            with self.subTest(kind=kind, repo=repo.name):
                base = git(repo, 'rev-parse', 'main')
                self.assertEqual(git(repo, 'rev-list', '--count', 'main..candidate'), '2')
                self.assertEqual(git(repo, 'diff', '--name-only', 'candidate~1', 'candidate'), 'README.md')
                printed = subprocess.run([sys.executable, '-I', str(TOOL), '--repo', str(repo), '--print-base'],
                                         text=True, capture_output=True)
                self.assertEqual(printed.stdout.strip(), base, printed.stderr)
                for given in ('main', 'auto'):
                    proc, report = self.check(repo, base=given)
                    self.assertEqual(proc.returncode, 1, (given, proc.stdout + proc.stderr))
                    self.assertEqual(report['snapshot']['base']['sha'], base)
                    dv = [f for f in report['findings'] if f['rule_id'] == 'deps.violation']
                    self.assertEqual(len(dv), 1, dv)
                    self.assertEqual(dv[0]['delta'], 'new')
                    self.assertTrue(fail_grade(dv[0]))
                # Control: the intermediate commit as base would have let the inversion through.
                proc, smuggled = self.check(repo, base='candidate~1')
                self.assertEqual(proc.returncode, 0, proc.stdout)
                self.assertFalse(any(fail_grade(f) for f in smuggled['findings']))

    # ------------------------------------------------------------------ binding

    def test_generic_binding_tracks_candidate_base_policy(self):
        for kind, repo in self.fixtures('good'):
            with self.subTest(kind=kind, repo=repo.name):
                def snap():
                    proc, report = self.check(repo)
                    blobs = {f['path']: (f['base_blob'], f['head_blob']) for f in report['policy']['files']}
                    return (report['snapshot']['base']['sha'], report['snapshot']['head']['sha'], blobs[PROFILE],
                            self.review_policy(repo)['definition_sha256'], report)
                b0, h0, (pb0, ph0), d0, _ = snap()
                self.assertEqual((b0, pb0), (git(repo, 'rev-parse', 'main'), ph0))
                # candidate changes: head sha moves; base and policy stay.
                write(repo, str(Path('docs') / 'notes.md'), '# Notes\n')
                commit(repo, 'candidate: notes')
                b1, h1, (pb1, _), d1, _ = snap()
                self.assertNotEqual(h1, h0)
                self.assertEqual((b1, pb1, d1), (b0, pb0, d0))
                # base advances (no policy change): base sha moves; policy stays.
                git(repo, 'switch', '-q', 'main')
                write(repo, str(Path('docs') / 'changelog.md'), '# Changelog\n')
                commit(repo, 'main: changelog')
                git(repo, 'switch', '-q', 'candidate')
                b2, h2, (pb2, _), d2, _ = snap()
                self.assertNotEqual(b2, b1)
                self.assertEqual((h2, pb2, d2), (h1, pb1, d1))
                # the candidate edits the policy: only the head blob moves; the base digest stays.
                (repo / PROFILE).write_text((repo / PROFILE).read_text() + '# candidate edit\n')
                commit(repo, 'candidate: policy edit')
                b3, h3, (pb3, ph3), d3, r3 = snap()
                self.assertNotEqual(h3, h2)
                self.assertEqual((b3, pb3, d3), (b2, pb2, d2))
                self.assertNotEqual(ph3, pb3)
                self.assertTrue(r3['policy']['changed'])
                # the base policy changes: base sha, base blob and review digest all move.
                git(repo, 'switch', '-q', 'main')
                (repo / PROFILE).write_text((repo / PROFILE).read_text() + '# approved edit\n')
                commit(repo, 'main: policy edit')
                git(repo, 'switch', '-q', 'candidate')
                b4, h4, (pb4, _), d4, _ = snap()
                self.assertEqual(h4, h3)
                self.assertNotEqual(b4, b3)
                self.assertNotEqual(pb4, pb3)
                self.assertNotEqual(d4, d3)


if __name__ == '__main__':
    unittest.main()
