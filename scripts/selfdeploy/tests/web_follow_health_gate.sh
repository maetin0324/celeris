#!/usr/bin/env bash
# ADR-0135 D2/D3: 一時起動・切替後の応答・旧 web の復旧を loopback だけで試す。
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
root="$(mktemp -d)"
cleanup() {
  if [ -f "$root/web.pid" ]; then kill "$(cat "$root/web.pid")" 2>/dev/null || :; fi
  rm -rf "$root"
}
trap cleanup EXIT
mkdir -p "$root/bin" "$root/config" "$root/state/releases" "$root/home"
read -r probe_port prod_port < <(python3 - <<'PY'
import socket
ports=[]
for _ in range(2):
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        ports.append(s.getsockname()[1])
print(*ports)
PY
)
cat >"$root/bin/fake-node" <<'PY'
#!/usr/bin/env python3
import json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
bind = os.environ['CELERIS_WEB_BIND']
port = int(bind.rsplit(':', 1)[1])
if port == int(os.environ['FAKE_PROBE_PORT']) and os.environ.get('FAKE_PROBE_FAIL') == '1':
    sys.exit(2)
release = os.environ['CELERIS_WEB_RELEASE']
if port == int(os.environ['FAKE_PROD_PORT']) and os.environ.get('FAKE_POST_FAIL') == '1':
    release = 'wrong-release'
class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        body = json.dumps({'release': release}).encode()
        self.send_response(200 if self.path == '/healthz' else 404)
        self.send_header('Content-Type', 'application/json')
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *_args):
        pass
HTTPServer(('127.0.0.1', port), Handler).serve_forever()
PY
cat >"$root/bin/systemctl" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$FAKE_SYSTEMCTL_LOG"
[ "$1" = --user ] && shift
verb="$1"; shift
case "$verb" in
  is-active) [ "${FAKE_OLD_INACTIVE:-}" = 1 ] && exit 3; [ "${*: -1}" = "celeris-web@$OLD" ] && exit 0; exit 3 ;;
  start)
    unit="$1"
    if [ "$unit" = "celeris-web@$NEW" ]; then
      [ "${FAKE_START_FAIL:-}" = 1 ] && exit 1
      CELERIS_WEB_BIND="127.0.0.1:$FAKE_PROD_PORT" CELERIS_WEB_RELEASE="$NEW" \
        "$SD_WEB_NODE" server/index.js >/dev/null 2>&1 &
      echo "$!" >"$FAKE_WEB_PID"
    fi ;;
  stop)
    [ "${FAKE_STOP_FAIL:-}" = 1 ] && [ "$1" = "celeris-web@$OLD" ] && exit 1
    if [ "$1" = "celeris-web@$NEW" ] && [ -f "$FAKE_WEB_PID" ]; then
      kill "$(cat "$FAKE_WEB_PID")" 2>/dev/null || :
      rm -f "$FAKE_WEB_PID"
    fi ;;
esac
exit 0
SH
chmod +x "$root/bin/fake-node" "$root/bin/systemctl"
export PATH="$root/bin:$PATH" HOME="$root/home" CELERIS_CONFIG_DIR="$root/config" CELERIS_STATE_DIR="$root/state"
export SD_WEB_NODE="$root/bin/fake-node" SD_WEB_PROBE_PORT="$probe_port" SD_WEB_PROBE_TIMEOUT=2 SD_WEB_HEALTH_TIMEOUT=2
export FAKE_PROBE_PORT="$probe_port" FAKE_PROD_PORT="$prod_port" FAKE_SYSTEMCTL_LOG="$root/calls.log" FAKE_WEB_PID="$root/web.pid"
export OLD=aaaaaaaaaaaa NEW=bbbbbbbbbbbb
printf 'CELERIS_WEB_BIND=127.0.0.1:%s\n' "$prod_port" >"$root/config/web.env"
mkdir -p "$root/state/releases/$NEW/web/app/server"
: >"$root/state/releases/$NEW/web/app/server/index.js"
printf '{"web":{"ok":true}}\n' >"$root/state/releases/$NEW/gate.json"
fail() { echo "FAIL: $*" >&2; cat "$root/calls.log" "$root/out.log" >&2; exit 1; }
run_follow() {
  : >"$root/calls.log"
  bash "$here/web-follow.sh" "$NEW" "$OLD" >"$root/out.log" 2>&1 || fail 'nonzero exit'
  if grep -q 'celeris@' "$root/calls.log"; then fail 'daemon unit touched'; fi
}
no_old_stop() { ! grep -q -- "--user stop celeris-web@$OLD" "$root/calls.log" || fail 'old web stopped'; }
wait_probe_port() {
  local i
  for i in {1..30}; do
    if python3 - "$probe_port" <<'PY' 2>/dev/null
import socket, sys
with socket.socket() as s:
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(('127.0.0.1', int(sys.argv[1])))
PY
    then return 0; fi
    sleep 0.1
  done
  fail 'probe process kept its port'
}
# (a) 依存が無ければ起動・切替を試みない。
run_follow
no_old_stop
! grep -q -- "--user start celeris-web@$NEW" "$root/calls.log" || fail 'missing dependencies accepted'
# 既存の gate・入口・旧 unit 条件も保持する。
mkdir -p "$root/state/releases/$NEW/web/app/node_modules"
printf '{"web":{"ok":false}}\n' >"$root/state/releases/$NEW/gate.json"
run_follow
no_old_stop
! grep -q -- "--user start celeris-web@$NEW" "$root/calls.log" || fail 'web gate ignored'
printf '{"web":{"ok":true}}\n' >"$root/state/releases/$NEW/gate.json"
rm "$root/state/releases/$NEW/web/app/server/index.js"
run_follow
no_old_stop
! grep -q -- "--user start celeris-web@$NEW" "$root/calls.log" || fail 'missing entry accepted'
: >"$root/state/releases/$NEW/web/app/server/index.js"
export FAKE_OLD_INACTIVE=1
run_follow
no_old_stop
! grep -q -- "--user start celeris-web@$NEW" "$root/calls.log" || fail 'inactive old ignored'
unset FAKE_OLD_INACTIVE
# (b) 一時起動の失敗では旧 web を保持。
export FAKE_PROBE_FAIL=1
run_follow
no_old_stop
! grep -q -- "--user start celeris-web@$NEW" "$root/calls.log" || fail 'failed probe accepted'
unset FAKE_PROBE_FAIL
# 旧 unit の停止自体が失敗した場合も新 unit を起動しない。
export FAKE_STOP_FAIL=1
run_follow
! grep -q -- "--user start celeris-web@$NEW" "$root/calls.log" || fail 'new web started while old stop failed'
unset FAKE_STOP_FAIL
# (c) 切替後の応答が別 release なら旧へ戻す。
export FAKE_POST_FAIL=1
run_follow
grep -q -- "--user stop celeris-web@$NEW" "$root/calls.log" || fail 'new web not stopped'
grep -q -- "--user disable celeris-web@$NEW" "$root/calls.log" || fail 'new web not disabled'
grep -q -- "--user start celeris-web@$OLD" "$root/calls.log" || fail 'old web not restarted'
grep -q -- "--user enable celeris-web@$OLD" "$root/calls.log" || fail 'old web not enabled'
unset FAKE_POST_FAIL
wait_probe_port
# (d) 成功時は切替と LAN unit の復旧を記録。
run_follow
grep -q -- "--user start celeris-web@$NEW" "$root/calls.log" || fail 'new web not started'
grep -q -- "--user stop celeris-web@$OLD" "$root/calls.log" || fail 'old web not stopped on success'
grep -q -- '--user reset-failed celeris-web-lan.service celeris-web-lan.socket' "$root/calls.log" || fail 'LAN reset missing'
grep -q -- '--user start celeris-web-lan.socket' "$root/calls.log" || fail 'LAN socket start missing'
# (e) systemctl start failure also restores old.
wait_probe_port
export FAKE_START_FAIL=1
run_follow
grep -q -- "--user start celeris-web@$OLD" "$root/calls.log" || fail 'old web not started after start failure'
echo 'web_follow_health_gate: all ok'
