#!/usr/bin/env python3
"""D2 mount table checks; does not launch bwrap or need userns."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).with_name('ops-r2-sandbox.sh')


class SandboxTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(dir='/tmp')
        self.root = Path(self.tmp.name).resolve()
        for name in ('state', 'release', 'account', 'tool chain', 'human'):
            (self.root / name).mkdir()
        self.env = dict(os.environ, HOME=str(self.root / 'human'))
        self.env.pop('CELERIS_KNOWLEDGE_ROOT', None)

    def tearDown(self):
        self.tmp.cleanup()

    def run_script(self, *extra, state=None):
        return subprocess.run(['sh', str(SCRIPT), '--state', str(state or self.root / 'state'),
                               '--release', str(self.root / 'release'), '--account',
                               str(self.root / 'account'), *extra, '--print-args', '--',
                               'echo', 'hello world', ''], env=self.env,
                              text=True, capture_output=True)

    def test_exact_mount_table_and_command(self):
        r = self.run_script('--ro', str(self.root / 'tool chain'))
        self.assertEqual(r.returncode, 0, r.stderr)
        args = r.stdout.splitlines()
        expected = ['--unshare-pid', '--unshare-ipc', '--new-session', '--die-with-parent',
                    '--clearenv', '--setenv', 'PATH', '/usr/bin:/bin', '--tmpfs', '/tmp',
                    '--proc', '/proc', '--dev', '/dev', '--ro-bind', str(self.root / 'release'),
                    str(self.root / 'release')]
        for path in reversed(['/usr', '/bin', '/sbin', '/lib', '/lib64', '/etc']):
            if Path(path).exists():
                expected += ['--ro-bind', path, path]
        expected += ['--bind', str(self.root / 'state'), str(self.root / 'state'),
                     '--setenv', 'HOME', str(self.root / 'state/home'), '--chdir',
                     str(self.root / 'state'), '--bind', str(self.root / 'account'),
                     str(self.root / 'account'), '--ro-bind', str(self.root / 'tool chain'),
                     str(self.root / 'tool chain'), '--', 'echo', 'hello world', '']
        self.assertEqual(args, expected)
        self.assertFalse((self.root / 'state/home').exists())

    def test_forbidden_sources(self):
        for path in (self.root / 'human/.claude', self.root / 'human/.config/celeris',
                     self.root / 'human', Path('/local/celeris/data/workspaces'),
                     Path('/home/rmaeda/workspace/agent-platform'), Path('/')):
            for flag in ('--ro', '--release', '--account', '--state'):
                with self.subTest(path=path, flag=flag):
                    r = self.run_script(flag, str(path))
                    self.assertEqual(r.returncode, 2, r.stderr)
                    self.assertEqual(r.stdout, '')

    def test_symlink_and_repository(self):
        (self.root / 'link').symlink_to(self.root / 'human/.claude')
        self.assertEqual(self.run_script('--ro', str(self.root / 'link')).returncode, 2)
        (self.root / 'tool chain/.git').write_text('gitdir: somewhere')
        self.assertEqual(self.run_script('--ro', str(self.root / 'tool chain')).returncode, 2)
        self.assertEqual(self.run_script('--ro', str(self.root)).returncode, 2)

    def test_production_kb_and_old_account(self):
        self.env['CELERIS_KNOWLEDGE_ROOT'] = str(self.root / 'tool chain')
        self.assertEqual(self.run_script('--ro', str(self.root / 'tool chain')).returncode, 2)
        (self.root / 'account/projects').mkdir()
        self.assertEqual(self.run_script().returncode, 2)

    def test_missing_command_and_bad_option(self):
        self.assertEqual(subprocess.run(['sh', str(SCRIPT)], capture_output=True).returncode, 2)
        self.assertEqual(self.run_script('--unknown').returncode, 2)


if __name__ == '__main__':
    unittest.main()
