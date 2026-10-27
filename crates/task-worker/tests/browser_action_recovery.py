"""Load runner functions only: never read /session or run its request loop."""
import ast
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from unittest.mock import patch

source = ast.parse(Path(sys.argv[1]).read_text())
functions = ast.Module(body=[n for n in source.body if isinstance(n, ast.FunctionDef)], type_ignores=[])
ns = dict(json=json, os=os, subprocess=subprocess)
for node in source.body:
    if isinstance(node, ast.Assign) and any(isinstance(t, ast.Name) and t.id == 'RELAY_CODES' for t in node.targets):
        ns['RELAY_CODES'] = ast.literal_eval(node.value)
exec(compile(functions, sys.argv[1], 'exec'), ns)

for mode in ('process_timeout', 'reported_timeout', 'unknown_ref', 'success', 'adopt_failed', 'no_file', 'ambiguous'):
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        guid = '12345678-1234-1234-1234-123456789abc'
        artifact = root / 'download.bin'
        state = {'blocked': False, 'recoveries': 0}

        class Process:
            returncode = 1

            def wait(self, timeout):
                if mode == 'success':
                    artifact.write_bytes(b'pdf')
                    self.returncode = 0
                    return
                if mode == 'unknown_ref':
                    return
                state['blocked'] = True
                if mode != 'no_file':
                    (root / guid).write_bytes(b'pdf')
                if mode == 'ambiguous':
                    (root / guid.replace('abc', 'def')).write_bytes(b'pdf')
                if mode == 'reported_timeout':
                    return
                raise subprocess.TimeoutExpired('fake browser', timeout)

            def kill(self):
                pass  # Killing the CLI does not stop its separate daemon.

            def communicate(self, timeout):
                error = 'Unknown ref' if mode == 'unknown_ref' else 'Download timed out'
                return json.dumps({'error': error}).encode(), None

        def recover():
            state['blocked'] = False
            state['recoveries'] += 1
            return True

        def snapshot(*args, **kwargs):
            if state['blocked']:
                raise subprocess.TimeoutExpired('snapshot', 45)
            return subprocess.CompletedProcess([], 0, b'{"success":true}')

        ns.update(ROOT=root, OUTPUT=root, GUID=re.compile(r'[0-9a-f-]{36}\Z'),
                  EXEC_TIMEOUT=45, DOWNLOAD_TIMEOUT=38,
                  browser_env=lambda: {}, browser_argv=lambda action: ['fake'],
                  command=lambda request: ['snapshot'], recover_browser_daemon=recover)
        real_replace = os.replace

        def replace(src, dst):
            if mode == 'adopt_failed':
                raise OSError('injected rename failure')
            real_replace(src, dst)

        with patch.object(subprocess, 'Popen', return_value=Process()), \
             patch.object(subprocess, 'run', side_effect=snapshot), \
             patch.object(os, 'replace', side_effect=replace):
            result = ns['download']({'phase': 'click', 'artifact': artifact.name}, ['fake'])
            expected_recovery = mode not in ('unknown_ref', 'success')
            assert state['recoveries'] == int(expected_recovery), (mode, result, state)
            assert (result.get('recovery') == 'daemon_restarted') == expected_recovery
            if mode in ('process_timeout', 'reported_timeout'):
                assert result['adopted_guid'] == guid and result['status'] == 0
                assert artifact.read_bytes() == b'pdf'
                assert not (root / guid).exists()
            elif mode != 'success':
                assert result['status'] == 1 and 'adopted_guid' not in result
            assert ns['execute']({'verb': 'snapshot', 'args': []})['status'] == 0
print('7 deterministic download recovery scenarios passed')
