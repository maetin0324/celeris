#!/usr/bin/env python3
"""Run fixed browser actions inside the sandboxd child, never on the worker host."""
import json
import os
from pathlib import Path
import re
import subprocess
import time
from urllib.parse import urljoin, urlsplit

ROOT = Path('/session')
REQUESTS = ROOT / 'actions'
CONFIG = json.loads((ROOT / 'action-config.json').read_text())
ALLOWED = set(json.loads((ROOT / 'policy.json').read_text())['allow'])
VERBS = {'open': 'navigate', 'click': 'click', 'snapshot': 'snapshot',
         'extract': 'gettext', 'screenshot': 'screenshot', 'download': 'download',
         'scroll': 'scroll', 'close': 'close'}
REF = re.compile(r'@e[0-9]+\Z')
# Fixed diagnostics (付記 2026-10-10f): `runner_reason` says which step failed and `error_class`
# what agent-browser's error was about. Neither carries page text, URLs or arguments; the launcher
# logs them so a failed screenshot / download is diagnosable without secrets.
RELAY_CODES = ('observation_origin_denied', 'password_field_present', 'redisplay_detected',
               'auth_section_active', 'cdp_command_denied', 'cdp_command_failed')
NAME = re.compile(r'(extract|screenshot|download)-[a-f0-9]{32}\.(json|png|bin)\Z')
# Chrome names a download it saves on its own (Browser.setDownloadBehavior allowAndName) by its guid.
GUID = re.compile(r'[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\Z')
OUTPUT = ROOT / 'output'
EXEC_TIMEOUT = 45


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


def error_class(stdout):
    try:
        data = json.loads(stdout or 'null')
    except ValueError:
        return 'unparsed'
    if not isinstance(data, dict):
        return 'unparsed'
    error = data.get('error')
    if not isinstance(error, str):
        return 'none' if data.get('success') is True else 'no_error_text'
    text = error.lower()
    for code in RELAY_CODES:
        if code in text:
            return code
    for marker, code in (('denied by policy', 'policy_denied'), ('unknown ref', 'unknown_ref'),
                         ('timed out', 'timeout'), ('timeout', 'timeout'),
                         ('download', 'download_error'), ('screenshot', 'screenshot_error'),
                         ('target closed', 'target_closed'), ('no page', 'no_page')):
        if marker in text:
            return code
    return 'other'


def browser_argv(action):
    endpoint = CONFIG.get('cdp_endpoint')
    if (not isinstance(endpoint, str) or
            not re.fullmatch(r'ws://127\.0\.0\.1:9223/[a-f0-9]{64}', endpoint)):
        return None
    return [CONFIG['executable'], '--config', '/session/upstream.json',
            '--session', CONFIG['session_id'], '--action-policy', '/session/policy.json',
            '--cdp', endpoint,
            '--content-boundaries', '--max-output', '16000', '--json'] + action


def browser_env():
    env = {'PATH': '/usr/bin:/bin', 'HOME': '/session/home', 'TMPDIR': '/session/tmp',
           'XDG_RUNTIME_DIR': '/session/run',
           'AGENT_BROWSER_NAMESPACE': 'celeris', 'HTTP_PROXY': 'http://127.0.0.1:3128',
           'HTTPS_PROXY': 'http://127.0.0.1:3128', 'ALL_PROXY': 'http://127.0.0.1:3128',
           'NO_PROXY': ''}
    if CONFIG.get('browser_cache'):
        env['PLAYWRIGHT_BROWSERS_PATH'] = CONFIG['browser_cache']
    return env


def query(action):
    """One short agent-browser read; its `data` (or None). Nothing of it leaves this process."""
    argv = browser_argv(action)
    if argv is None:
        return None
    try:
        result = subprocess.run(argv, cwd=ROOT, env=browser_env(), stdout=subprocess.PIPE,
                                stderr=subprocess.DEVNULL, timeout=10, check=False)
        data = json.loads(result.stdout[:1048576].decode('utf-8', errors='replace') or 'null')
    except (OSError, ValueError, subprocess.TimeoutExpired):
        return None
    if not isinstance(data, dict) or data.get('success') is not True:
        return None
    return data.get('data')


def text_value(data, *keys):
    if isinstance(data, str):
        return data
    if isinstance(data, dict):
        for key in keys:
            if isinstance(data.get(key), str):
                return data[key]
            if key in data and data[key] is None:
                return None
    return None


def link_tokens(ref):
    """付記 2026-10-10j: the download link's static shape as fixed tokens (no URL, no text)."""
    attrs = {}
    for name in ('href', 'target', 'download', 'onclick'):
        data = query(['get', 'attr', ref, name])
        attrs[name] = text_value(data, 'value', 'attribute', name, 'result')
    page = text_value(query(['get', 'url']), 'url', 'value', 'result')
    if all(v is None for v in attrs.values()) and page is None:
        return ['link_unreadable']
    tokens = []
    target = attrs['target']
    tokens.append('target_blank' if target == '_blank' else 'target_named' if target else 'target_none')
    tokens.append('download_attr' if attrs['download'] is not None else 'no_download_attr')
    tokens.append('onclick' if attrs['onclick'] else 'no_onclick')
    href = attrs['href']
    if not href:
        tokens.append('href_none')
        return tokens
    if href.strip().lower().startswith('javascript:'):
        tokens.append('href_javascript')
        return tokens
    try:
        resolved = urlsplit(urljoin(page or '', href))
        here = urlsplit(page or '')
    except ValueError:
        tokens.append('href_unparsed')
        return tokens
    same = (resolved.scheme, resolved.netloc) == (here.scheme, here.netloc) and bool(here.netloc)
    tokens.append('href_same_origin' if same else 'href_other_origin')
    path = resolved.path.lower()
    if path.endswith('.pdf'):
        tokens.append('path_pdf')
    if '/ct/' in path and 'page_' in path:
        tokens.append('path_ct_page')
    if 'file' in path:
        tokens.append('path_file')
    if resolved.query:
        tokens.append('has_query')
    if resolved.fragment:
        tokens.append('has_fragment')
    return tokens


def completed_guids():
    try:
        names = set(os.listdir(OUTPUT))
    except OSError:
        return set()
    return {n for n in names if GUID.fullmatch(n) and n + '.crdownload' not in names
            and (OUTPUT / n).is_file() and not (OUTPUT / n).is_symlink()}


def download(request, argv):
    """付記 2026-10-10j: run agent-browser's download, and adopt a download Chrome completed on its
    own meanwhile (a link opening a new tab) when agent-browser does not see it. The launcher
    checks the adopted guid against the downloads its controller let through."""
    tokens = link_tokens(request['args'][0])
    before = completed_guids()
    try:
        proc = subprocess.Popen(argv, cwd=ROOT, env=browser_env(), stdout=subprocess.PIPE,
                                stderr=subprocess.DEVNULL)
    except OSError:
        return {'status': 1, 'stdout': '', 'runner_reason': 'exec_failed', 'link': tokens}
    # agent-browser is never cut short for a file it may still be saving itself; a download it did
    # not see is adopted only after it gave up.
    reason = None
    try:
        proc.wait(timeout=EXEC_TIMEOUT)
    except subprocess.TimeoutExpired:
        reason = 'exec_timeout'
        proc.kill()
    try:
        out, _ = proc.communicate(timeout=5)
    except subprocess.TimeoutExpired:
        out = b''
    stdout = out[:1048576].decode('utf-8', errors='replace')
    if reason is None and proc.returncode == 0:
        return {'status': 0, 'stdout': stdout}
    fresh = sorted(completed_guids() - before)
    artifact = OUTPUT / request['artifact']
    # Only when agent-browser waited in vain for a download (not when it failed for its own reason,
    # e.g. an unknown ref) is a file Chrome completed meanwhile this action's download.
    waited = reason == 'exec_timeout' or error_class(stdout) == 'timeout'
    if waited and len(fresh) == 1 and not artifact.exists():
        try:
            os.replace(OUTPUT / fresh[0], artifact)
        except OSError:
            return {'status': 1, 'stdout': '', 'runner_reason': 'adopt_failed', 'link': tokens}
        return {'status': 0, 'stdout': json.dumps({'success': True, 'data': {'adopted': True}}),
                'adopted_guid': fresh[0], 'runner_reason': 'adopted_download', 'link': tokens}
    response = {'status': 1, 'stdout': stdout if reason is None else '',
                'runner_reason': reason or 'agent_browser_exit',
                'error_class': error_class(stdout) if reason is None else 'none', 'link': tokens}
    if len(fresh) > 1:
        response['runner_reason'] = 'downloads_ambiguous'
    return response


def execute(request):
    if request.get('verb') == '__version__' and request.get('args') == []:
        argv = [CONFIG['executable'], '--version']
    else:
        action = command(request)
        argv = browser_argv(action)
        if argv is None:
            return {'status': 1, 'stdout': '', 'runner_reason': 'config_invalid'}
        if request.get('verb') == 'download':
            return download(request, argv)
    env = browser_env()
    try:
        result = subprocess.run(argv, cwd=ROOT, env=env, stdout=subprocess.PIPE,
                                stderr=subprocess.DEVNULL, timeout=EXEC_TIMEOUT, check=False)
    except subprocess.TimeoutExpired:
        return {'status': 1, 'stdout': '', 'runner_reason': 'exec_timeout'}
    except OSError:
        return {'status': 1, 'stdout': '', 'runner_reason': 'exec_failed'}
    stdout = result.stdout[:1048576].decode('utf-8', errors='replace')
    response = {'status': result.returncode, 'stdout': stdout}
    if result.returncode != 0:
        response['runner_reason'] = 'agent_browser_exit'
        response['error_class'] = error_class(stdout)
    return response


def finish_artifact(request, response):
    """The launcher (another host UID) reads screenshot / download files to hand them to Celeris
    (protocol v8); the generated name was validated by command()."""
    artifact = request.get('artifact')
    if (response.get('status') != 0 or request.get('verb') not in ('screenshot', 'download')
            or not isinstance(artifact, str) or not NAME.fullmatch(artifact)):
        return response
    produced = OUTPUT / artifact
    if produced.is_symlink() or not produced.is_file():
        # agent-browser reported success but left no file under the generated name.
        return {'status': 3, 'stdout': '', 'runner_reason': 'output_missing',
                'error_class': error_class(response.get('stdout'))}
    try:
        produced.chmod(0o644)
    except OSError:
        return {'status': 3, 'stdout': '', 'runner_reason': 'output_chmod_failed'}
    return response


REQUESTS.mkdir(exist_ok=True)
while True:
    for path in sorted(REQUESTS.glob('*.request')):
        try:
            request = json.loads(path.read_text())
        except (OSError, ValueError):
            request = None
        try:
            if not isinstance(request, dict):
                raise ValueError()
            response = finish_artifact(request, execute(request))
        except ValueError:
            response = {'status': 2, 'stdout': '', 'runner_reason': 'request_invalid'}
        except (OSError, KeyError, TypeError):
            response = {'status': 2, 'stdout': '', 'runner_reason': 'runner_error'}
        result = path.with_suffix('.result')
        temporary = path.with_suffix('.next')
        temporary.write_text(json.dumps(response))
        temporary.chmod(0o644)
        temporary.replace(result)
        path.unlink(missing_ok=True)
    time.sleep(0.02)
