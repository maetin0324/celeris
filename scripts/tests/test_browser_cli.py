"""Run the real CLI transport against an executable fake agent-browser."""
import contextlib
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import socket
import tempfile
import threading
import time
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
        (self.root / 'upstream.json').write_text('{}')
        self.policy(['click', 'close', 'download', 'gettext', 'launch', 'navigate', 'screenshot', 'scroll', 'snapshot'])
        self.mode({})
        root_patch = patch.object(cli, 'ROOT', self.root)
        root_patch.start()
        self.addCleanup(root_patch.stop)
        self.stop_server = threading.Event()
        self.listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.listener.bind(str(self.root.with_suffix('.action.sock')))
        self.listener.listen(4)
        self.listener.settimeout(0.1)
        self.server = threading.Thread(target=self.serve_actions, daemon=True)
        self.server.start()
        self.addCleanup(self.close_server)

    def close_server(self):
        self.stop_server.set()
        self.server.join(timeout=3)
        self.listener.close()
        self.root.with_suffix('.action.sock').unlink(missing_ok=True)

    def serve_actions(self):
        while not self.stop_server.is_set():
            try:
                connection, _ = self.listener.accept()
            except socket.timeout:
                continue
            except OSError:
                return
            with connection:
                data = bytearray()
                while True:
                    part = connection.recv(16384)
                    if not part:
                        break
                    data.extend(part)
                try:
                    request = json.loads(data)
                    verb, args = request['verb'], request['args']
                    artifact = request.get('artifact')
                    if verb == 'open':
                        command = ['open'] + args
                    elif verb == 'snapshot':
                        command = ['snapshot', '-i']
                    elif verb == 'extract':
                        command = ['get', 'text'] + args
                    elif verb in ('screenshot', 'download'):
                        command = [verb] + args + [str(self.output / artifact)]
                    else:
                        command = [verb] + args
                    argv = [str(self.executable), '--config', str(self.root / 'upstream.json'),
                            '--session', self.config['session_id'], '--action-policy', str(self.root / 'policy.json'),
                            '--allowed-domains', ','.join(self.config['allowed_domains']),
                            '--content-boundaries', '--max-output', '16000', '--json'] + command
                    env = {'AGENT_BROWSER_NAMESPACE': 'celeris', 'PATH': os.environ.get('PATH', '/usr/bin:/bin')}
                    result = cli.subprocess.run(argv, cwd=self.root, env=env, stdout=cli.subprocess.PIPE,
                                                stderr=cli.subprocess.DEVNULL, timeout=45, check=False)
                    response = {'status': result.returncode, 'stdout': result.stdout.decode()}
                except (OSError, cli.subprocess.TimeoutExpired, ValueError, KeyError, TypeError):
                    response = {'status': 1, 'stdout': ''}
                connection.sendall(json.dumps(response).encode())

    def policy(self, allow):
        """Write policy.json and config.json the way the supervisor does (hash-bound)."""
        raw = json.dumps({'default': 'deny', 'allow': allow}, separators=(',', ':')).encode()
        (self.root / 'policy.json').write_bytes(raw)
        self.config['policy_sha256'] = hashlib.sha256(raw).hexdigest()
        (self.root / 'config.json').write_text(json.dumps(self.config))

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
        pid_file = self.root / 'child.pid'
        class StartedPopen(cli.subprocess.Popen):
            # Start the timeout only after the child has recorded its pid, so a slow
            # Python start under workspace-test load cannot outlast the timeout.
            def communicate(self, *args, **kwargs):
                deadline = time.monotonic() + 60
                while not pid_file.exists() and self.poll() is None and time.monotonic() < deadline:
                    time.sleep(0.05)
                return super().communicate(*args, **kwargs)
        def short_timeout(*args, **kwargs):
            kwargs['timeout'] = 2
            return original_run(*args, **kwargs)
        with patch.object(cli.subprocess, 'run', side_effect=short_timeout), \
                patch.object(cli.subprocess, 'Popen', StartedPopen):
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

    def test_missing_empty_or_broken_policy_fails_closed_without_invoking_substrate(self):
        # agent-browser 0.38.1 runs eval with these policies (fail-open); the shim must not call it.
        good = (self.root / 'policy.json').read_text()
        cases = [None, '', '{"default":"deny","allow":', json.dumps({'default': 'deny', 'allow': []}),
                 json.dumps({'default': 'allow', 'allow': ['navigate']}), json.dumps({'default': 'deny'}),
                 json.dumps({'default': 'deny', 'allow': ['navigate', 'evaluate']}),
                 json.dumps({'default': 'deny', 'allow': ['navigate'], 'deny': []}), '[]',
                 json.dumps({'default': 'deny', 'allow': ['navigate']}),
                 json.dumps({'default': 'deny', 'allow': ['launch', 'close']}),
                 json.dumps({'default': 'deny', 'allow': ['close', 'launch', 'navigate', 'navigate']}),
                 json.dumps({'default': 'deny', 'allow': ['close', 'launch', 'navigate', 'cookies_get']}),
                 json.dumps({'default': 'deny', 'allow': ['close', 'launch', 'navigate', 'state_save']})]
        for policy in cases:
            with self.subTest(policy=policy):
                (self.root / 'invocation.json').unlink(missing_ok=True)
                (self.root / 'policy.json').unlink(missing_ok=True)
                if policy is not None:
                    (self.root / 'policy.json').write_text(policy)
                    # Hash matches, so the shape check alone must refuse it.
                    self.config['policy_sha256'] = hashlib.sha256(policy.encode()).hexdigest()
                    (self.root / 'config.json').write_text(json.dumps(self.config))
                code, stdout = self.run_cli('open', 'https://example.com/public')
                self.assertEqual(code, 2)
                self.assertFalse((self.root / 'invocation.json').exists())
                self.assertEqual(self.events()[-1], {'operation': 'policy_block', 'status': 'blocked'})
        (self.root / 'policy.json').write_text(good)
        self.policy(json.loads(good)['allow'])
        for domains in [[], [''], ['example.com,evil.com'], ['*'], ['*.'], ['Example.com'], None]:
            with self.subTest(domains=domains):
                (self.root / 'invocation.json').unlink(missing_ok=True)
                (self.root / 'config.json').write_text(json.dumps(dict(self.config, allowed_domains=domains)))
                code, _ = self.run_cli('snapshot')
                self.assertEqual(code, 2)
                self.assertFalse((self.root / 'invocation.json').exists())

    def test_shim_action_allowlist_matches_generator_vocabulary(self):
        source = (SOURCE.parents[2] / 'task-core/src/browser.rs').read_text()
        block = source[source.index('// harness-upstream-actions:begin'):source.index('// harness-upstream-actions:end')]
        names = set(re.findall(r'"([a-z_]+)"', block))
        self.assertEqual(names, cli.ACTIONS)
        self.assertEqual(set(cli.VERB_ACTIONS.values()) | {'launch'}, cli.ACTIONS)

    def test_tampered_policy_hash_fails_closed(self):
        # A same-UID harness rewriting policy.json (e.g. adding evaluate) is refused.
        for allow in [['close', 'launch', 'navigate', 'snapshot', 'evaluate'], ['close', 'launch', 'navigate']]:
            with self.subTest(allow=allow):
                (self.root / 'policy.json').write_text(json.dumps({'default': 'deny', 'allow': allow}))
                code, _ = self.run_cli('open', 'https://example.com/public')
                self.assertEqual(code, 2)
                self.assertFalse((self.root / 'invocation.json').exists())
        (self.root / 'config.json').write_text(json.dumps({k: v for k, v in self.config.items() if k != 'policy_sha256'}))
        code, _ = self.run_cli('snapshot')
        self.assertEqual(code, 2)
        self.assertFalse((self.root / 'invocation.json').exists())

    def test_actions_outside_task_policy_never_reach_substrate(self):
        # Generated for a task allowed only snapshot + extract.
        self.policy(['close', 'gettext', 'launch', 'snapshot'])
        for args in [('open', 'https://example.com/public'), ('click', '@e1'), ('screenshot',),
                     ('download', '@e1'), ('scroll', 'down', '10')]:
            with self.subTest(verb=args[0]):
                code, stdout = self.run_cli(*args)
                self.assertEqual(code, 2)
                self.assertIn('not permitted', stdout)
                self.assertFalse((self.root / 'invocation.json').exists())
                self.assertEqual(self.events()[-1], {'operation': 'policy_block', 'status': 'blocked'})
        self.assertEqual(list(self.output.iterdir()), [])
        for args in [('snapshot',), ('extract', '@e1'), ('close',)]:
            with self.subTest(allowed=args[0]):
                code, _ = self.run_cli(*args)
                self.assertEqual(code, 0)

    def test_request_approval_freezes_one_operation_and_approval_actions_hint(self):
        # ADR 2026-10-08 D2: click is under approval → not in allow, listed in approval_actions.
        self.config['approval_actions'] = ['click']
        self.policy(['close', 'download', 'gettext', 'launch', 'navigate', 'screenshot', 'scroll', 'snapshot'])
        code, out = self.run_cli('click', '@e1')
        self.assertEqual(code, 2)
        self.assertIn('needs a human approval', out)
        self.assertIn('request-approval', out)
        self.assertFalse((self.root / 'invocation.json').exists())
        # Blocked requests: wrong action, a verb still allowed, bad ref, origin with a path or
        # outside the allowed domains, empty purpose, wrong arity.
        for args in (['download', '@e1', 'https://example.com', 'p'],
                     ['snapshot', '@e1', 'https://example.com', 'p'],
                     ['click', 'e1', 'https://example.com', 'p'],
                     ['click', '@e1', 'https://example.com/login', 'p'],
                     ['click', '@e1', 'https://evil.test', 'p'],
                     ['click', '@e1', 'https://example.com', ''],
                     ['click', '@e1', 'https://example.com']):
            code, out = self.run_cli('request-approval', *args)
            self.assertEqual(code, 2, args)
            self.assertIn('approval request blocked', out)
        self.assertFalse((self.root / 'approval-request.json').exists())
        code, out = self.run_cli('request-approval', 'click', '@e1', 'https://example.com', 'Press export')
        self.assertEqual(code, 0)
        self.assertEqual(json.loads(out), {'success': True, 'status': 'waiting_for_approval'})
        self.assertEqual(json.loads((self.root / 'approval-request.json').read_text()),
                         {'action': 'click', 'target': '@e1', 'origin': 'https://example.com', 'purpose': 'Press export'})
        # One request per session; the substrate never ran.
        code, out = self.run_cli('request-approval', 'click', '@e2', 'https://example.com', 'Again')
        self.assertEqual(code, 2)
        self.assertFalse((self.root / 'invocation.json').exists())
        self.assertEqual([e['operation'] for e in self.events()],
                         ['policy_block'] * 8 + ['approval_request', 'policy_block'])
        # Without the approval_actions list the hint is the plain policy refusal.
        del self.config['approval_actions']
        self.policy(['close', 'launch', 'navigate'])
        code, out = self.run_cli('click', '@e1')
        self.assertEqual(code, 2)
        self.assertIn('not permitted by the task browser policy', out)

    def test_navigation_outside_allowed_domains_is_blocked(self):
        # Page content asking the model to go elsewhere is untrusted: the shim refuses.
        for url in ['https://evil.example/', 'https://example.org/', 'https://example.com.evil.example/',
                    'https://notexample.org/', 'https://evil-example.com/', 'https://example.com./',
                    'http://ｅxample.com/', 'https://a.example.org.evil.example/']:
            with self.subTest(url=url):
                (self.root / 'invocation.json').unlink(missing_ok=True)
                code, stdout = self.run_cli('open', url)
                self.assertEqual(code, 2)
                self.assertFalse((self.root / 'invocation.json').exists())
                self.assertNotIn('evil', stdout)
        for url in ['https://example.com/public', 'https://app.example.org/', 'https://A.B.Example.org/x']:
            with self.subTest(allowed=url):
                code, _ = self.run_cli('open', url)
                self.assertEqual(code, 0)


if __name__ == '__main__':
    unittest.main()
