"""Which argument of a wrapped check is its script (ADR 2026-11-11-health-checker-gaps G2).

rules/existing_checks.py makes a wrapped check unsupported (command_missing) before it starts when the
command's script is not a file in the head tree.  Invariants guarded here and why the existing tests do not
catch them:

- Only the first operand of a known interpreter (sh, bash, dash, python, python3) is a script.  An argument of
  another kind (`-s scripts/tests`, a `test_*.py` pattern, `-m module`, `-c string`) is not.  The fixture tests
  (test_repo_health_generic_bypass.py) only run `sh <script>` and a tool that is not on PATH, so a detector that
  takes any path-like argument passes them while it turns Celeris' own source-size rule
  (`python3 -m unittest discover -s scripts/tests ...`) into unsupported, which fails every Celeris task.
- Every command of the Celeris profile passes the pre-launch check against this tree, so the fix of G2 does not
  make a required rule of this repo unsupported.  No other test reads profiles/celeris.toml's commands.
"""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib
import unittest

from repo_health_fixtures import PROFILE, commit, git, write

ROOT = Path(__file__).resolve().parents[2]
TOOL_DIR = ROOT / 'scripts/dev/repo-health'
TOOL = TOOL_DIR / 'repo_health.py'
CELERIS_PROFILE = TOOL_DIR / 'profiles/celeris.toml'


def existing_checks():
    """rules/existing_checks.py as the checker loads it (its `health` package comes from the checker dir)."""
    if str(TOOL_DIR) not in sys.path:
        sys.path.insert(0, str(TOOL_DIR))
    spec = importlib.util.spec_from_file_location('repo_health_rules_existing_checks_test',
                                                  TOOL_DIR / 'rules/existing_checks.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class ScriptOfTests(unittest.TestCase):
    def test_script_is_the_first_operand_of_an_interpreter(self):
        checks = existing_checks()
        cases = [
            (['sh', 'scripts/dev/check-doc-links.sh', '--root', '/r'], 'scripts/dev/check-doc-links.sh'),
            (['sh', '-e', 'tools/check.sh'], 'tools/check.sh'),
            (['sh', '-eo', 'pipefail', 'tools/check.sh'], 'tools/check.sh'),
            (['bash', '--', 'tools/check.sh'], 'tools/check.sh'),
            (['dash', 'check.sh'], 'check.sh'),
            (['python3', 'scripts/dev/check-human-text.py', '--root', '/r'], 'scripts/dev/check-human-text.py'),
            (['python3', '-I', 'scripts/dev/check.py'], 'scripts/dev/check.py'),
            (['python3', '-W', 'error', 'check.py'], 'check.py'),
            (['python3', '-Werror', 'check.py'], 'check.py'),
            (['/usr/bin/python3', 'check.py'], 'check.py'),
            (['python3.13', 'check.py'], 'check.py'),
            (['python', 'check.py'], 'check.py'),
            # No script: a module, a command string, stdin, nothing, or a tool that is not an interpreter.
            (['python3', '-m', 'unittest', 'discover', '-s', 'scripts/tests', '-p', 'test_source_size*.py'], None),
            (['python3', '-Im', 'unittest'], None),
            (['python3', '-mcompileall', 'scripts/dev'], None),
            (['python3', '-c', 'import scripts.x'], None),
            (['python3', '-'], None),
            (['sh', '-c', 'scripts/dev/check.sh'], None),
            (['sh', '-ec', 'scripts/dev/check.sh'], None),
            (['sh'], None),
            (['orchard-adr-lint', '--root', 'scripts/dev/check.sh'], None),
            (['cargo', 'run', '--manifest-path', 'tools/check.py'], None),
        ]
        for argv, script in cases:
            with self.subTest(argv=argv):
                self.assertEqual(checks._script_of(argv), script)

    def test_celeris_profile_commands_pass_the_preflight(self):
        """Every wrapped command of profiles/celeris.toml starts: its tool is on PATH and its script is in this tree."""
        checks = existing_checks()
        rules = tomllib.loads(CELERIS_PROFILE.read_text(encoding='utf-8'))['rules']
        subs = {'{root}': str(ROOT), '{base}': '0' * 40}
        scripts, modules = [], []
        for rule in checks.RULES:
            commands = rules.get(rule.id, {}).get('commands')
            self.assertTrue(commands, f'{rule.id} has no commands in {CELERIS_PROFILE.name}')
            for argv in commands:
                argv = [subs.get(a, a) for a in argv]
                with self.subTest(rule=rule.id, argv=argv):
                    try:
                        script = checks._preflight(argv, str(ROOT))
                    except checks.Unsupported as exc:
                        self.fail(f'{rule.id} would be unsupported: {exc}')
                    if script is None:
                        # The only commands of this profile without a script are module runs.
                        self.assertEqual(argv[1], '-m', argv)
                        modules.append(argv)
                    else:
                        self.assertTrue((ROOT / script).is_file(), script)
                        scripts.append(script)
        self.assertIn('scripts/dev/check-source-size.py', scripts)
        self.assertIn(['python3', '-m', 'unittest', 'discover', '-s', 'scripts/tests', '-p', 'test_source_size*.py'],
                      modules)


class WrappedCheckEndToEndTests(unittest.TestCase):
    """The shipped checker on an isolated repo whose base profile wraps a module run and a python script."""

    def repo(self):
        temporary = tempfile.TemporaryDirectory(prefix='orchard-', dir=os.environ.get('TMPDIR'))
        self.addCleanup(temporary.cleanup)
        repo = Path(temporary.name) / 'orchard'
        repo.mkdir()
        git(repo, 'init', '-q', '-b', 'main')
        for key, value in (('user.name', 'Fixture'), ('user.email', 'fixture@example.invalid'),
                           ('commit.gpgsign', 'false')):
            git(repo, 'config', key, value)
        write(repo, 'README.md', '# Orchard\n')
        write(repo, 'scripts/check.py', 'import sys\nsys.exit(0)\n')
        write(repo, 'scripts/tests/test_ok.py',
              'import unittest\n\n\nclass Ok(unittest.TestCase):\n    def test_ok(self):\n        pass\n')
        write(repo, PROFILE, '''version = "1"
description = "orchard: wrapped checks"

[rules."doc.links"]
commands = [
    ["python3", "-m", "unittest", "discover", "-s", "scripts/tests", "-p", "test_*.py"],
    ["python3", "-I", "scripts/check.py", "--root", "{root}"],
]
''')
        commit(repo, 'base')
        git(repo, 'switch', '-q', '-c', 'candidate')
        return repo

    def check(self, repo):
        out = repo.parent / 'report.json'
        proc = subprocess.run([sys.executable, '-I', str(TOOL), '--repo', str(repo), '--base', 'main',
                               '--head', 'candidate', '--policy-version', '1', '--out', str(out)],
                              text=True, capture_output=True)
        report = json.loads(out.read_text()) if out.exists() else None
        self.assertIsNotNone(report, proc.stderr)
        return proc, report, {r['id']: r for r in report['rules']}

    def test_module_run_with_path_like_arguments_runs(self):
        """`-s scripts/tests -p test_*.py` are not scripts: the module run starts and doc.links passes."""
        repo = self.repo()
        write(repo, 'README.md', '# Orchard\n\nNotes.\n')
        commit(repo, 'docs')
        proc, report, rules = self.check(repo)
        self.assertEqual(rules['doc.links']['status'], 'pass', rules['doc.links'])
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)

    def test_missing_python_script_is_unsupported(self):
        """python3 exits 2 for a script it cannot open; the pre-launch check makes it unsupported, not a finding."""
        repo = self.repo()
        (repo / 'scripts/check.py').unlink()
        commit(repo, 'drop the wrapped script')
        proc, report, rules = self.check(repo)
        rule = rules['doc.links']
        self.assertEqual(rule['status'], 'unsupported', rule)
        self.assertEqual(rule['reason'], 'command_missing: script scripts/check.py が head の tree に無い', rule)
        self.assertFalse([f for f in report['findings'] if f['rule_id'] == 'doc.links'])
        self.assertEqual(proc.returncode, 2, proc.stdout + proc.stderr)


if __name__ == '__main__':
    unittest.main()
