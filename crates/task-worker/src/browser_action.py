#!/usr/bin/env python3
"""Run fixed browser actions inside the sandboxd child, never on the worker host."""
import json
import os
from pathlib import Path
import re
import subprocess
import time
from urllib.parse import urlsplit

ROOT = Path('/session')
REQUESTS = ROOT / 'actions'
CONFIG = json.loads((ROOT / 'action-config.json').read_text())
ALLOWED = set(json.loads((ROOT / 'policy.json').read_text())['allow'])
VERBS = {'open': 'navigate', 'click': 'click', 'snapshot': 'snapshot',
         'extract': 'gettext', 'screenshot': 'screenshot', 'download': 'download',
         'scroll': 'scroll', 'close': 'close'}
REF = re.compile(r'@e[0-9]+\Z')
NAME = re.compile(r'(extract|screenshot|download)-[a-f0-9]{32}\.(json|png|bin)\Z')


def allowed_origin(url):
    """task-core origin containment: scheme and port match, `*.base` admits subdomains only."""
    try:
        port = url.port or {'https': 443, 'http': 80}.get(url.scheme)
    except ValueError:
        return False
    host = url.hostname or ''
    if not host or host.startswith('*.') or port is None:
        return False
    for domain in CONFIG['allowed_domains']:
        scheme, _, authority = domain.partition('://')
        base, sep, dport = ('', '', '') if authority.endswith(']') else authority.rpartition(':')
        if not sep:
            base, dport = authority, '443' if scheme == 'https' else '80'
        base = base.strip('[]')
        if (scheme == url.scheme and int(dport) == port
                and (host.endswith(base[1:]) if base.startswith('*.') else host == base)):
            return True
    return False


def command(request):
    verb = request.get('verb')
    args = request.get('args')
    artifact = request.get('artifact')
    if verb not in VERBS or not isinstance(args, list) or any(not isinstance(a, str) for a in args):
        raise ValueError()
    if VERBS[verb] not in ALLOWED or len(args) > 2 or any(len(a) > 4096 for a in args):
        raise ValueError()
    if verb == 'open':
        if len(args) != 1:
            raise ValueError()
        url = urlsplit(args[0])
        if (url.scheme not in ('http', 'https') or not url.hostname or url.username or url.password
                or url.query or url.fragment or '\\' in args[0] or not allowed_origin(url)):
            raise ValueError()
        action = ['open', args[0]]
    elif verb in ('click', 'extract', 'download'):
        if len(args) != 1 or not REF.fullmatch(args[0]):
            raise ValueError()
        action = {'click': ['click', args[0]], 'extract': ['get', 'text', args[0]],
                  'download': ['download', args[0], '/session/output/' + str(artifact)]}[verb]
    elif verb == 'snapshot':
        if args:
            raise ValueError()
        # The full accessibility tree (body text included) with link URLs, so the agent can read
        # the page and `open` a link instead of clicking it. `-i` (interactive only) dropped the text.
        action = ['snapshot', '--urls']
    elif verb == 'screenshot':
        if args:
            raise ValueError()
        action = ['screenshot', '/session/output/' + str(artifact)]
    elif verb == 'scroll':
        if len(args) != 2 or args[0] not in ('up', 'down') or not args[1].isdigit() or not 1 <= int(args[1]) <= 2000:
            raise ValueError()
        action = ['scroll'] + args
    else:
        if args:
            raise ValueError()
        action = ['close']
    if verb in ('screenshot', 'download') and (not isinstance(artifact, str) or not NAME.fullmatch(artifact)):
        raise ValueError()
    if verb not in ('screenshot', 'download') and artifact is not None:
        raise ValueError()
    return action


def execute(request):
    if request.get('verb') == '__version__' and request.get('args') == []:
        argv = [CONFIG['executable'], '--version']
    else:
        action = command(request)
        endpoint = CONFIG.get('cdp_endpoint')
        if (not isinstance(endpoint, str) or
                not re.fullmatch(r'ws://127\.0\.0\.1:9223/[a-f0-9]{64}', endpoint)):
            return {'status': 1, 'stdout': ''}
        argv = [CONFIG['executable'], '--config', '/session/upstream.json',
                '--session', CONFIG['session_id'], '--action-policy', '/session/policy.json',
                '--cdp', endpoint,
                '--content-boundaries', '--max-output', '16000', '--json'] + action
    env = {'PATH': '/usr/bin:/bin', 'HOME': '/session/home', 'TMPDIR': '/session/tmp',
           'XDG_RUNTIME_DIR': '/session/run',
           'AGENT_BROWSER_NAMESPACE': 'celeris', 'HTTP_PROXY': 'http://127.0.0.1:3128',
           'HTTPS_PROXY': 'http://127.0.0.1:3128', 'ALL_PROXY': 'http://127.0.0.1:3128',
           'NO_PROXY': ''}
    if CONFIG.get('browser_cache'):
        env['PLAYWRIGHT_BROWSERS_PATH'] = CONFIG['browser_cache']
    try:
        result = subprocess.run(argv, cwd=ROOT, env=env, stdout=subprocess.PIPE,
                                stderr=subprocess.DEVNULL, timeout=45, check=False)
        return {'status': result.returncode, 'stdout': result.stdout[:1048576].decode('utf-8', errors='replace')}
    except (OSError, subprocess.TimeoutExpired):
        return {'status': 1, 'stdout': ''}


REQUESTS.mkdir(exist_ok=True)
while True:
    for path in sorted(REQUESTS.glob('*.request')):
        try:
            request = json.loads(path.read_text())
            response = execute(request)
        except (OSError, ValueError, KeyError, TypeError):
            response = {'status': 2, 'stdout': ''}
        result = path.with_suffix('.result')
        temporary = path.with_suffix('.next')
        temporary.write_text(json.dumps(response))
        temporary.chmod(0o644)
        temporary.replace(result)
        path.unlink(missing_ok=True)
    time.sleep(0.02)
