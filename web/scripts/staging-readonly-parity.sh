#!/usr/bin/env bash
# verify.sh hook: run the released gateway against its live staging snapshot.
set -euo pipefail

[ "$#" -eq 5 ] || { echo "usage: $0 <sha12> <release-dir> <api-url> <gui-url> <token-file>" >&2; exit 2; }
sha12="$1"
release_dir="$2"
api_url="$3"
gui_url="$4"
token_file="$5"
[ "$api_url" = 'http://127.0.0.1:7711' ] || { echo "refusing non-staging API: $api_url" >&2; exit 2; }
[ "$gui_url" = 'http://127.0.0.1:7701' ] || { echo "refusing non-staging GUI: $gui_url" >&2; exit 2; }
[ -f "$release_dir/web/app/server/index.js" ] || { echo "release web/app missing" >&2; exit 1; }
: "${WEB_STAGING_LOG_DIR:?set WEB_STAGING_LOG_DIR to the run artifacts directory}"
mkdir -p "$WEB_STAGING_LOG_DIR"

repo_dir="$(cd "$(dirname "$0")/../.." && pwd)"
private_dir="$(mktemp -d)"
chmod 700 "$private_dir"
gateway_pid=""
cleanup() {
  if [ -n "$gateway_pid" ]; then
    kill "$gateway_pid" 2>/dev/null || true
    wait "$gateway_pid" 2>/dev/null || true
  fi
  rm -rf "$private_dir"
}
trap cleanup EXIT INT TERM
head -c 32 /dev/urandom | base64 >"$private_dir/session-secret"
head -c 32 /dev/urandom | base64 >"$private_dir/password"

port="$(python3 - <<'PY'
import socket
for wanted in (7720, 0):
    try:
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', wanted))
            print(sock.getsockname()[1])
            break
    except OSError:
        continue
else:
    raise SystemExit('no staging gateway port')
PY
)"
base="http://127.0.0.1:$port"
(
  cd "$release_dir/web/app"
  CELERIS_WEB_BIND="127.0.0.1:$port" \
  CELERIS_API_URL="$api_url" \
  CELERIS_API_TOKEN_FILE="$token_file" \
  CELERIS_WEB_PASSWORD_FILE="$private_dir/password" \
  CELERIS_WEB_SESSION_SECRET_FILE="$private_dir/session-secret" \
  CELERIS_WEB_RELEASE="$sha12" \
  exec node server/index.js
) >"$WEB_STAGING_LOG_DIR/web-gateway.log" 2>&1 &
gateway_pid=$!

ready=false
for _ in $(seq 1 60); do
  if curl --fail --silent "$base/healthz" >"$WEB_STAGING_LOG_DIR/web-health.json"; then
    ready=true
    break
  fi
  kill -0 "$gateway_pid" 2>/dev/null || break
  sleep 1
done
[ "$ready" = true ] || { echo "web gateway did not become healthy" >&2; exit 1; }
curl --fail --silent "$gui_url/healthz" >"$WEB_STAGING_LOG_DIR/gui-health.json"

echo "staging gui=$gui_url api=$api_url web=$base sha12=$sha12"
set +e
(
  cd "$repo_dir/web"
  WEB_E2E_REAL_BASE_URL="$base" \
  WEB_E2E_PASSWORD_FILE="$private_dir/password" \
  WEB_E2E_SHA12="$sha12" \
  corepack pnpm@12.6.0 e2e parity/real-staging-readonly.spec.ts
) >"$WEB_STAGING_LOG_DIR/web-parity-e2e.log" 2>&1
e2e_exit=$?
set -e
echo "web parity e2e exit=$e2e_exit"
tail -n 12 "$WEB_STAGING_LOG_DIR/web-parity-e2e.log"
exit "$e2e_exit"
