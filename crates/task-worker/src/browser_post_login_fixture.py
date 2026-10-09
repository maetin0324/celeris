"""Loopback-only IdP + LMS fixture (ADR 2026-10-09 credential username / post-login).

Three HTTPS origins: the IdP (username and password on one form, not at the origin root), the LMS
("manaba"-like assignment list behind a session cookie) and an unrelated origin. Files in the
working directory steer the flow: `reject` makes the IdP refuse the login, `landing_pw` lands the
SP on a page with a password field. `received` records what the IdP got (for the test only).
"""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import html
import ssl
import threading
from urllib.parse import parse_qs

COOKIE = 'sid=lms-session-cookie-7f3a'


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
                self.send('', 302, '/idp/form?execution=e1s1')
            elif self.path.startswith('/idp/form'):
                self.send('''<body><h1>Unified login</h1><form method=post action="/idp/submit">
<input name=j_username type=text autocomplete=username>
<input name=j_password type=password autocomplete=current-password>
<button name=_eventId_proceed>Login</button></form>''')
            else:
                self.send('<body>IdP home (no form here)')
        elif me == lms_origin:
            if self.path == '/ct/logout':
                self.send('', 302, '/ct/home', [('Set-Cookie', 'sid=; Max-Age=0; Path=/; Secure')])
            elif not self.signed_in():
                self.send('', 302, idp_origin + '/idp/login')
            elif self.path == '/ct/home':
                user, _ = received()
                self.send(f'''<body><h1>Assignments</h1>
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
                threading.Event().wait(2)
                try:
                    self.wfile.write(body)
                except OSError:
                    pass
            else:
                self.send('<body>Other site page')

    def do_POST(self):
        data = self.rfile.read(int(self.headers.get('Content-Length', '0'))).decode()
        form = parse_qs(data)
        me = self.origin()
        if me == idp_origin and self.path == '/idp/submit':
            user = form.get('j_username', [''])[0]
            password = form.get('j_password', [''])[0]
            Path('received.tmp').write_text(user + '\n' + password)
            Path('received.tmp').replace('received')
            if Path('reject').exists() or not user or not password:
                self.send('''<body><p>Login failed</p><form method=post action="/idp/submit">
<input name=j_username type=text><input name=j_password type=password>
<button name=_eventId_proceed>Login</button></form>''')
                return
            self.send(f'''<body><form name=saml method=post action="{lms_origin}/sp">
<input type=hidden name=SAMLResponse value="assertion-ok"></form>
<script>document.saml.submit()</script>''')
        elif me == lms_origin and self.path == '/sp':
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
