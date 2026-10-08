"""Loopback-only Shibboleth fixture; two HTTPS origins and real form POSTs."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import ssl
import threading
from urllib.parse import parse_qs

class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def send(self, body, status=200, location=None):
        data = body.encode()
        self.send_response(status)
        if location:
            self.send_header('Location', location)
        self.send_header('Content-Type', 'text/html')
        self.send_header('Content-Length', str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path == '/entry':
            self.send('', 302, '/relay?execution=e1s1')
        elif self.path.startswith('/relay'):
            # POST replaces the document; no password exists in this relay.
            self.send('''<body>Loading Session Information<form name=form1 method=post action="/password?execution=e1s2"><button>Continue</button></form><script>document.form1.submit()</script>''')
        elif self.path == '/cross':
            self.send('', 302, other_origin + '/bounce')
        elif self.path == '/bounce':
            self.send('', 302, idp_origin + '/relay?execution=e1s1')
        else:
            self.send('<body>Loading Session Information')

    def do_POST(self):
        data = self.rfile.read(int(self.headers.get('Content-Length', '0')))
        if self.path.startswith('/password'):
            self.send('''<body><form method=post action=/submit><input name=j_password type=password><button name=_eventId_proceed>Login</button></form>''')
        elif self.path == '/submit':
            password = parse_qs(data.decode()).get('j_password', [''])[0]
            # Fixture tests observe receipt at the SP, not the input's value.
            self.send(f'''<body><form name=saml method=post action="{other_origin}/sp"><input type=hidden name=SAMLResponse value="{password}"></form><script>document.saml.submit()</script>''')
        elif self.path == '/sp':
            password = parse_qs(data.decode()).get('SAMLResponse', [''])[0]
            Path('received.tmp').write_text(password)
            Path('received.tmp').replace('received')
            self.send('<body>SP signed in')
        else:
            self.send('missing', 404)

context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain('cert.pem', 'key.pem')
servers = [ThreadingHTTPServer(('127.0.0.1', 0), Handler) for _ in range(2)]
idp_origin, other_origin = [f'https://127.0.0.1:{s.server_port}' for s in servers]
for server in servers:
    server.socket = context.wrap_socket(server.socket, server_side=True)
    threading.Thread(target=server.serve_forever, daemon=True).start()
Path('ports.tmp').write_text(' '.join(str(s.server_port) for s in servers))
Path('ports.tmp').replace('ports')
threading.Event().wait()
