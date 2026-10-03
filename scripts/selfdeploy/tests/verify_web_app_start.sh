#!/usr/bin/env bash
# ADR-0135 D4: verify records web app probe success/failure without blocking verify.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT
command -v sqlite3 >/dev/null 2>&1 || { echo "verify_web_app_start: skipped (no sqlite3)"; exit 0; }
[ -x /usr/bin/node ] || { echo "verify_web_app_start: skipped (no /usr/bin/node)"; exit 0; }

fail() { echo "FAIL: $*" >&2; [ -f "$root/verify.out" ] && tail -n 50 "$root/verify.out" >&2; exit 1; }
free_ports() {
  python3 - <<'PY'
import socket
socks = []
for _ in range(4):
    s = socket.socket(); s.bind(("127.0.0.1", 0)); socks.append(s)
print(" ".join(str(s.getsockname()[1]) for s in socks))
PY
}

sqlite3 "$root/prod.sqlite3" <<'SQL'
CREATE TABLE tasks (id TEXT, status TEXT);
CREATE TABLE projects (id TEXT);
CREATE TABLE milestones (id TEXT);
CREATE TABLE org_nodes (id TEXT);
CREATE TABLE approvals (id TEXT, decision TEXT);
CREATE TABLE reports (id TEXT, node_id TEXT);
CREATE TABLE messages (id TEXT, node_id TEXT);
CREATE TABLE schema_migrations (version INTEGER);
INSERT INTO schema_migrations VALUES (5);
SQL
read -r api_port gui_port old_port web_port <<<"$(free_ports)"
mkdir -p "$root/bin" "$root/config" "$root/state/releases/aaaaaaaaaaaa/bin" \
  "$root/state/releases/aaaaaaaaaaaa/gui/node_modules/@playwright/test"
: >"$root/config/config.toml"
rel="$root/state/releases/aaaaaaaaaaaa"
printf '{"sha12":"aaaaaaaaaaaa","schema_version":5}\n' >"$rel/manifest.json"
printf '{"ok":true,"web":{"ok":true}}\n' >"$rel/gate.json"

cat >"$root/fake-celeris.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
args=sys.argv[1:]; listen=args[args.index('--listen')+1]; release=args[args.index('--release')+1]
host,port=listen.rsplit(':',1)
class H(BaseHTTPRequestHandler):
    def log_message(self,*a): pass
    def send(self,obj):
        body=json.dumps(obj).encode(); self.send_response(200); self.send_header('Content-Type','application/json'); self.send_header('Content-Length',str(len(body))); self.end_headers(); self.wfile.write(body)
    def do_GET(self):
        p=self.path.split('?')[0]
        if p=='/api/v1/health': return self.send({'schema_version':5,'mode':'verify','release':release})
        if p=='/api/v1/tasks': return self.send({'items':[],'next_cursor':None,'total':0})
        if p=='/api/v1/tasks/t1': return self.send({'task':{'status':'done'}})
        if p=='/api/v1/tasks/t1/events': return self.send({'items':[{'event':{'type':'worker_started'}},{'event':{'type':'worker_finished','outcome':'done: fake'}}]})
        if p=='/api/v1/reports': return self.send({'items':[{'id':'r1','task_id':'t1'}]})
        if p in ('/api/v1/projects','/api/v1/org','/api/v1/approvals'): return self.send({'items':[]})
        return self.send({})
    def do_POST(self):
        n=int(self.headers.get('Content-Length') or 0); self.rfile.read(n); return self.send({'id':'t1','status':'ready'})
ThreadingHTTPServer((host,int(port)),H).serve_forever()
PY
cat >"$rel/gui/server.js" <<'JS'
const http=require('http'); const [host,port]=process.env.CELERIS_GUI_BIND.split(':');
http.createServer((req,res)=>{if(req.url==='/healthz'){res.setHeader('Content-Type','application/json');res.end(JSON.stringify({release:process.env.CELERIS_GUI_RELEASE}));}else res.end('ok');}).listen(Number(port),host);
JS
printf '#!/usr/bin/env bash\nexec python3 %q "$@"\n' "$root/fake-celeris.py" >"$rel/bin/celeris"
chmod +x "$rel/bin/celeris"
mkdir -p "$root/state/releases/bbbbbbbbbbbb/bin"
cp "$rel/manifest.json" "$root/state/releases/bbbbbbbbbbbb/manifest.json"
cp "$rel/gate.json" "$root/state/releases/bbbbbbbbbbbb/gate.json"
printf '#!/usr/bin/env bash\nexec python3 %q "$@"\n' "$root/fake-celeris.py" >"$root/state/releases/bbbbbbbbbbbb/bin/celeris"
chmod +x "$root/state/releases/bbbbbbbbbbbb/bin/celeris"
ln -s releases/bbbbbbbbbbbb "$root/state/current"
cat >"$root/bin/node" <<'SH'
#!/usr/bin/env bash
exec /usr/bin/node "$@"
SH
cat >"$root/bin/pnpm" <<'SH'
#!/usr/bin/env bash
exit 0
SH
chmod +x "$root/bin/node" "$root/bin/pnpm"

cat >"$root/web-app.js" <<'JS'
const http=require('http'); const [host,port]=process.env.CELERIS_WEB_BIND.split(':');
http.createServer((req,res)=>{res.setHeader('Content-Type','application/json');res.end(JSON.stringify({release:process.env.CELERIS_WEB_RELEASE}));}).listen(Number(port),host);
JS
mkdir -p "$rel/web/app/server" "$rel/web/app/node_modules"
cp "$root/web-app.js" "$rel/web/app/server/index.js"

run_verify() {
  CELERIS_STATE_DIR="$root/state" CELERIS_CONFIG_DIR="$root/config" CELERIS_CONFIG="$root/config/config.toml" \
    CELERIS_DB="$root/prod.sqlite3" SD_REPO="$root/no-repo" SD_PNPM_SHIM_DIR="$root/bin" \
    SD_STAGING_API_PORT="$api_port" SD_STAGING_GUI_PORT="$gui_port" SD_STAGING_OLD_API_PORT="$old_port" \
    SD_VERIFY_WEB_PROBE_PORT="$web_port" SD_WEB_NODE="$root/bin/node" SD_WEB_PROBE_TIMEOUT=2 \
    PATH="$root/bin:$PATH" bash "$here/verify.sh" aaaaaaaaaaaa >"$root/verify.out" 2>&1
}

# Successful startup must be recorded and clean up its temporary listener.
run_verify || fail "verify.sh failed despite successful web probe"
python3 - "$rel/verify.json" <<'PY' || fail "successful web probe was not recorded"
import json,sys
v=json.load(open(sys.argv[1])); c=next(c for c in v['checks'] if c['id']=='4d')
assert c['name']=='web-app-start' and c['ok'] is True and '/healthz 200' in c['detail'], c
PY

# Broken app is a failed non-blocking record: overall verification remains successful.
rm -rf "$rel/web/app/node_modules"
run_verify || fail "verify.sh became blocking after web probe failure"
python3 - "$rel/verify.json" <<'PY' || fail "failed web probe was not recorded non-blockingly"
import json,sys
v=json.load(open(sys.argv[1])); c=next(c for c in v['checks'] if c['id']=='4d')
assert v['ok'] is True and c['ok'] is False and 'non-blocking' in c['detail'], (v['ok'],c)
PY

if ss -ltn "sport = :$web_port" 2>/dev/null | tail -n +2 | grep -q .; then fail "web probe left its listener running"; fi
echo "verify_web_app_start: ok"
