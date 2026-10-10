"""Loopback-only IdP + LMS fixture (ADR 2026-10-09 credential username / post-login).

Three HTTPS origins: the IdP (Shibboleth-shaped: the login URL redirects to a localStorage
interstitial that auto-POSTs before the username + password form; after the form another
localStorage interstitial and a SAML auto-POST page lead to the SP), the LMS ("manaba"-like
assignment list behind a session cookie) and an unrelated origin. Files in the working directory
steer the flow: `reject` makes the IdP show the login form again, `consent` stops on an
attribute-release consent page (`consent_again`: shown again after it is submitted; each
submission is recorded in `consent_posts`), `landing_pw` lands the SP on a page with a password field,
`slow_ms` delays each post-login auto-POST hop. `received` records what the IdP got (test only).
"""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import html
import ssl
import threading
from urllib.parse import parse_qs

COOKIE = 'sid=lms-session-cookie-7f3a'
SSO = '/idp/profile/SAML2/Unsolicited/SSO'
LOGIN_FORM = '''<body><h1>Unified login</h1>
<form method=post action="/idp/profile/SAML2/Unsolicited/SSO?execution=e1s2">
<input name=j_username type=text autocomplete=username>
<input name=j_password type=password autocomplete=current-password>
<input type=checkbox name=donotcache value=1>
<button name=_eventId_proceed>Login</button></form>'''


def consent_page():
    user, _ = received()
    return f'''<body><h1>Information to be provided to the service</h1>
<table><tr><td>uid</td><td>{html.escape(user)}</td></tr><tr><td>mail</td><td>{html.escape(user)}@u.example</td></tr></table>
<form method=post action="/idp/profile/SAML2/Unsolicited/SSO?execution=e1s4">
<input type=hidden name=_shib_idp_consentIds value=uid>
<label><input type=radio name=_shib_idp_consentOptions value=_shib_idp_doNotRememberConsent> Ask me again at next login</label>
<label><input type=radio name=_shib_idp_consentOptions value=_shib_idp_rememberConsent checked> Ask me again if information changes</label>
<input type=submit name=_eventId_AttributeReleaseRejected value=Reject>
<input type=submit name=_eventId_proceed value=Accept></form>'''


def saml_post():
    return f'''<body onload="setTimeout(function(){{document.forms[0].submit()}},{slow_ms()})">
<form method=post action="{lms_origin}/Shibboleth.sso/SAML2/POST">
<input type=hidden name=RelayState value="cookie">
<input type=hidden name=SAMLResponse value="assertion-ok"></form>'''


def slow_ms():
    try:
        return int(Path('slow_ms').read_text())
    except (OSError, ValueError):
        return 0


def interstitial(action, delay):
    return f'''<body>Loading Session Information<form name=form1 method=post action="{action}">
<input type=hidden name="shib_idp_ls_exception.shib_idp_session_ss" value="">
<input type=hidden name="shib_idp_ls_success.shib_idp_session_ss" value="true">
<input type=hidden name="_eventId_proceed" value=""></form>
<script>setTimeout(function(){{document.form1.submit()}},{delay})</script>'''


def received():
    try:
        user, password = Path('received').read_text().split('\n', 1)
        return user, password
    except (OSError, ValueError):
        return '', ''


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def send(self, body, status=200, location=None, headers=()):
        data = body.encode()
        self.send_response(status)
        if location:
            self.send_header('Location', location)
        for name, value in headers:
            self.send_header(name, value)
        if not any(name == 'Content-Type' for name, _ in headers):
            self.send_header('Content-Type', 'text/html; charset=utf-8')
        self.send_header('Content-Length', str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def origin(self):
        return f'https://127.0.0.1:{self.server.server_port}'

    def signed_in(self):
        return COOKIE in (self.headers.get('Cookie') or '')

    def do_GET(self):
        me = self.origin()
        if me == idp_origin:
            if self.path == '/idp/login':
                self.send('', 302, SSO + '?execution=e1s1')
            elif self.path.startswith(SSO + '?execution=e1s1'):
                # localStorage interstitial (no password field), auto-POST to the same step.
                self.send(interstitial(SSO + '?execution=e1s1', 0))
            else:
                self.send('<body>IdP home (no form here)')
        elif me == lms_origin:
            if self.path == '/ct/go_idp':
                self.send('', 302, idp_origin + '/')
            elif self.path == '/ct/go_other':
                self.send('', 302, other_origin + '/page')
            elif self.path == '/ct/logout':
                self.send('', 302, '/ct/home', [('Set-Cookie', 'sid=; Max-Age=0; Path=/; Secure')])
            elif not self.signed_in():
                self.send('', 302, idp_origin + '/idp/login')
            elif self.path == '/ct/home':
                user, _ = received()
                # manaba-like: a collapsed login widget with an empty password input on the home page.
                self.send(f'''<body><div id=relogin style="display:none"><input type=password name=userpass></div>
<h1>Assignments</h1>
<p id=who>Signed in as {html.escape(user)}</p>
<ul><li><a id=report href="/ct/report_1">Report 1: Fluid dynamics essay</a></li></ul>
<a id=dl href="/ct/files/handout.pdf">Handout</a>
<a id=dl-other href="{other_origin}/files/other.bin">Other file</a>''')
            elif self.path == '/ct/report_1':
                self.send('<body><h1>Report 1 detail</h1><p>Due 2026-10-20 23:59</p>')
            elif self.path == '/ct/files/handout.pdf':
                self.send('%PDF-1.4 handout for report 1', headers=[
                    ('Content-Type', 'application/pdf'),
                    ('Content-Disposition', 'attachment; filename="handout.pdf"')])
            elif self.path == '/ct/autofilled':
                # A hidden password input that holds a value is still live.
                self.send('<body><h1>Course</h1><div style="display:none"><input type=password value="filled-by-page"></div>')
            elif self.path == '/ct/settings':
                self.send('<body><h1>Settings</h1><form><input type=password name=new_password></form>')
            elif self.path == '/ct/leak':
                _, password = received()
                self.send(f'<body><p>Your password is {html.escape(password)}</p>')
            else:
                self.send('missing', 404)
        else:
            if self.path == '/files/other.bin':
                # Headers first, the body a little later (a download in progress when it begins).
                body = b'other origin file'
                self.send_response(200)
                self.send_header('Content-Type', 'application/octet-stream')
                self.send_header('Content-Disposition', 'attachment; filename="other.bin"')
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.flush()
                threading.Event().wait(8)
                try:
                    self.wfile.write(body)
                except OSError:
                    pass
            else:
                self.send('<body>Other site page')

    def do_POST(self):
        data = self.rfile.read(int(self.headers.get('Content-Length', '0'))).decode()
        form = parse_qs(data, keep_blank_values=True)
        me = self.origin()
        if me == idp_origin and self.path == SSO + '?execution=e1s1':
            self.send(LOGIN_FORM)
        elif me == idp_origin and self.path == SSO + '?execution=e1s2':
            user = form.get('j_username', [''])[0]
            password = form.get('j_password', [''])[0]
            Path('received.tmp').write_text(user + '\n' + password)
            Path('received.tmp').replace('received')
            # Like Shibboleth / Spring Web Flow: the submitter's event (`_eventId_proceed`) must be in
            # the POST, otherwise the IdP shows the login form again.
            if (Path('reject').exists() or not user or not password
                    or '_eventId_proceed' not in form):
                self.send(LOGIN_FORM.replace('<h1>', '<p>Login failed</p><h1>'))
                return
            # localStorage write interstitial after the login, then consent or the SAML POST.
            self.send(interstitial(SSO + '?execution=e1s3', slow_ms()))
        elif me == idp_origin and self.path == SSO + '?execution=e1s3':
            if Path('consent').exists():
                self.send(consent_page())
                return
            self.send(saml_post())
        elif me == idp_origin and self.path == SSO + '?execution=e1s4':
            # The consent form's submission (test record: which button and option were sent).
            sent = [k for k in form if k.startswith('_eventId_')]
            option = form.get('_shib_idp_consentOptions', [''])[0]
            with open('consent_posts', 'a') as f:
                f.write(','.join(sent) + ' ' + option + '\n')
            if Path('consent_again').exists() or sent != ['_eventId_proceed']:
                self.send(consent_page())
                return
            self.send(saml_post())
        elif me == lms_origin and self.path == '/Shibboleth.sso/SAML2/POST':
            landing = '/ct/settings' if Path('landing_pw').exists() else '/ct/home'
            self.send('', 302, landing, [('Set-Cookie', COOKIE + '; Path=/; Secure; HttpOnly')])
        else:
            self.send('missing', 404)


context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain('cert.pem', 'key.pem')
servers = [ThreadingHTTPServer(('127.0.0.1', 0), Handler) for _ in range(3)]
idp_origin, lms_origin, other_origin = [f'https://127.0.0.1:{s.server_port}' for s in servers]
for server in servers:
    server.socket = context.wrap_socket(server.socket, server_side=True)
    threading.Thread(target=server.serve_forever, daemon=True).start()
Path('ports.tmp').write_text(' '.join(str(s.server_port) for s in servers))
Path('ports.tmp').replace('ports')
threading.Event().wait()
