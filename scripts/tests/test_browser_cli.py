"""Run the real CLI transport against an executable fake agent-browser."""
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SOURCE = Path(__file__).resolve().parents[2] / 'crates/task-worker/src/browser_cli.py'
spec = importlib.util.spec_from_file_location('browser_cli', SOURCE)
cli = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cli)
SECRET = 'DO_NOT_LEAK_password_TOTP_829172'
FAKE = r'''#!/usr/bin/env python3
import json, os, pathlib, sys, time
root = pathlib.Path.cwd()
(root / 'child.pid').write_text(str(os.getpid()))
(root / 'invocation.json').write_text(json.dumps({'argv': sys.argv[1:], 'env': {k:v for k,v in os.environ.items() if k.startswith('AGENT_BROWSER_')}}))
mode = json.loads((root / 'fake.json').read_text())
args = sys.argv[1:]
command = args[args.index('--json')+1:]
if command[0] in ('screenshot', 'download'):
    pathlib.Path(command[-1]).write_bytes(b'public artifact')
if mode.get('sleep'):
    print(mode.get('stderr', ''), flush=True)
    time.sleep(mode['sleep'])
if 'raw' in mode:
    print(mode['raw'])
else:
    print(json.dumps(mode.get('payload', {'success': True, 'data': {'text': 'public result'}})))
print(mode.get('stderr', ''), file=sys.stderr)
sys.exit(mode.get('exit', 0))
'''


class BrowserCliTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.output = self.root / 'artifacts'
        self.output.mkdir()
        self.executable = self.root / 'agent-browser'
        self.executable.write_text(FAKE)
        self.executable.chmod(0o700)
        self.config = {'executable': str(self.executable), 'output': str(self.output),
                       'session_id': 'celeris-test-session', 'allowed_domains': ['example.com', '*.example.org']}
        (self.root / 'config.json').write_text(json.dumps(self.config))
        (self.root / 'upstream.json').write_text('{}')
        (self.root / 'policy.json').write_text(json.dumps({'default': 'deny', 'allow': ['navigate', 'snapshot', 'gettext', 'screenshot', 'download', 'click', 'scroll', 'close', 'launch']}))
        self.mode({})
        root_patch = patch.object(cli, 'ROOT', self.root)
        root_patch.start()
        self.addCleanup(root_patch.stop)

    def mode(self, value):
        (self.root / 'fake.json').write_text(json.dumps(value))

    def run_cli(self, *args):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            result = cli.main(list(args))
        return result, out.getvalue()

    def events(self):
        path = self.root / 'events.jsonl'
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def assert_no_secret(self, stdout):
        self.assertNotIn(SECRET, stdout)
        self.assertNotIn(SECRET, json.dumps(self.events()))
        for artifact in self.output.iterdir():
            self.assertNotIn(SECRET.encode(), artifact.read_bytes())

    def test_exact_policy_session_command_argv_and_environment(self):
        with patch.dict(os.environ, {'AGENT_BROWSER_PASSWORD': SECRET, 'AGENT_BROWSER_STATE': '/private/state',
                                     'AGENT_BROWSER_CDP': 'http://127.0.0.1:9222', 'AGENT_BROWSER_NAMESPACE': 'foreign'}):
            code, stdout = self.run_cli('open', 'https://example.com/public')
        self.assertEqual(code, 0)
        invocation = json.loads((self.root / 'invocation.json').read_text())
        self.assertEqual(invocation['argv'], ['--config', str(self.root / 'upstream.json'), '--session', 'celeris-test-session',
            '--action-policy', str(self.root / 'policy.json'), '--allowed-domains', 'example.com,*.example.org',
            '--content-boundaries', '--max-output', '16000', '--json', 'open', 'https://example.com/public'])
        self.assertEqual(invocation['env'], {'AGENT_BROWSER_NAMESPACE': 'celeris'})
        self.assertEqual(self.events(), [{'operation': 'navigate', 'status': 'success'}])
        self.assert_no_secret(stdout)

    def test_privileged_flags_secret_urls_and_path_traversal_are_blocked(self):
        cases = [('eval', SECRET), ('cookies', 'get'), ('storage', 'local'), ('auth', 'save', SECRET),
                 ('--cdp', SECRET, 'snapshot'), ('connect', '9222'), ('fill', '@e1', SECRET),
                 ('snapshot', '--json'), ('open', 'https://user:' + SECRET + '@example.com'),
                 ('open', 'https://example.com/?token=' + SECRET), ('open', 'https://example.com/#' + SECRET),
                 ('open', 'file:///etc/passwd'), ('screenshot', '../../outside.png'),
                 ('download', '@e1', '../../outside.bin'), ('extract', '../../secret'),
                 ('click', '#password'), ('click', '@e1', '--cdp', SECRET), ('scroll', 'down', '999999')]
        for args in cases:
            with self.subTest(verb=args[0]):
                code, stdout = self.run_cli(*args)
                self.assertEqual(code, 2)
                self.assert_no_secret(stdout)
        self.assertFalse((self.root / 'invocation.json').exists())
        self.assertTrue(all(e == {'operation': 'policy_block', 'status': 'blocked'} for e in self.events()))
        self.assertEqual(list(self.output.iterdir()), [])

    def test_extract_screenshot_download_artifacts_and_event_metadata(self):
        for args, operation, suffix in [(('extract', '@e2'), 'extract', '.json'), (('screenshot',), 'screenshot', '.png'), (('download', '@e3'), 'download', '.bin')]:
            code, stdout = self.run_cli(*args)
            self.assertEqual(code, 0)
            event = self.events()[-1]
            self.assertEqual(set(event), {'operation', 'status', 'artifact'})
            self.assertEqual(event['operation'], operation)
            name = event['artifact']
            self.assertTrue(name.startswith(operation + '-'))
            self.assertTrue(name.endswith(suffix))
            self.assertTrue((self.output / name).is_file())
            if operation == 'extract':
                self.assertEqual(json.loads((self.output / name).read_text()), {'untrusted': True, 'data': {'text': 'public result'}})
                self.assertIn('<untrusted_browser_content>', stdout)
            else:
                self.assertEqual((self.output / name).read_bytes(), b'public artifact')
        self.assertEqual(len(list(self.output.iterdir())), 3)

    def test_snapshot_uses_upstream_interactive_refs(self):
        boundary = {'nonce': 'upstream-test-nonce', 'origin': 'https://example.com/public'}
        self.mode({'payload': {'success': True, 'data': {'refs': {}}, '_boundary': boundary}})
        code, stdout = self.run_cli('snapshot')
        self.assertEqual(code, 0)
        self.assertIn('<untrusted_browser_content>', stdout)
        self.assertEqual(json.loads(stdout.splitlines()[1])['_boundary'], boundary)
        invocation = json.loads((self.root / 'invocation.json').read_text())
        self.assertEqual(invocation['argv'][-2:], ['snapshot', '-i'])
        self.assertEqual(list(self.output.iterdir()), [])

    def test_unknown_refs_fail_in_substrate_without_success_events(self):
        self.mode({'payload': {'success': False, 'error': 'unknown reference'}})
        for verb in ['click', 'extract', 'download']:
            code, stdout = self.run_cli(verb, '@e999999')
            self.assertEqual(code, 1)
            self.assertEqual(self.events()[-1]['status'], 'failure')
            self.assertNotIn('unknown reference', stdout)
        self.assertEqual(list(self.output.iterdir()), [])

    def test_failure_stdout_stderr_and_partial_artifacts_are_not_exposed(self):
        for mode in [{'exit': 3, 'payload': {'success': True, 'data': SECRET}, 'stderr': SECRET},
                     {'payload': {'success': False, 'error': SECRET}, 'stderr': SECRET},
                     {'raw': SECRET, 'stderr': SECRET}]:
            with self.subTest(mode=mode.get('exit')):
                self.mode(mode)
                code, stdout = self.run_cli('screenshot')
                self.assertEqual(code, 1)
                self.assertEqual(list(self.output.iterdir()), [])
                self.assertEqual(self.events()[-1], {'operation': 'screenshot', 'status': 'failure'})
                self.assert_no_secret(stdout)

    def test_malformed_json_shape_fails_closed_and_cleans_artifact(self):
        for raw in ['null', '[]', '"' + SECRET + '"']:
            with self.subTest(shape=raw[:4]):
                self.mode({'raw': raw})
                code, stdout = self.run_cli('screenshot')
                self.assertEqual(code, 1)
                self.assertEqual(list(self.output.iterdir()), [])
                self.assert_no_secret(stdout)

    def test_timed_out_process_is_reaped_and_partial_artifact_removed(self):
        self.mode({'sleep': 10, 'stderr': SECRET})
        original_run = cli.subprocess.run
        def short_timeout(*args, **kwargs):
            kwargs['timeout'] = 0.1
            return original_run(*args, **kwargs)
        with patch.object(cli.subprocess, 'run', side_effect=short_timeout):
            code, stdout = self.run_cli('screenshot')
        self.assertEqual(code, 1)
        self.assertEqual(list(self.output.iterdir()), [])
        self.assertEqual(self.events(), [{'operation': 'screenshot', 'status': 'failure'}])
        child_pid = int((self.root / 'child.pid').read_text())
        with self.assertRaises(ProcessLookupError):
            os.kill(child_pid, 0)
        self.assert_no_secret(stdout)

    def test_missing_executable_is_opaque_and_audited(self):
        self.executable.unlink()
        code, stdout = self.run_cli('close')
        self.assertEqual(code, 1)
        self.assertEqual(self.events(), [{'operation': 'close', 'status': 'failure'}])
        self.assertNotIn(str(self.executable), stdout)

    def test_upstream_policy_blocks_are_distinct_without_reflected_secret(self):
        for reason in ['Action denied by policy: ', 'URL not in allowed domains: ']:
            self.mode({'exit': 1, 'payload': {'success': False, 'error': reason + SECRET}})
            code, stdout = self.run_cli('open', 'https://example.com/public')
            self.assertEqual(code, 1)
            self.assertEqual(self.events()[-1], {'operation': 'policy_block', 'status': 'blocked'})
            self.assert_no_secret(stdout)


if __name__ == '__main__':
    unittest.main()
