#!/usr/bin/env bash
# web probe が CELERIS_WEB_PROBE=1 で起動し、owner socket を渡さないことを検証する。
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }

mkdir -p "$root/app/server" "$root/app/node_modules" "$root/config"
: >"$root/app/server/index.js"
cat >"$root/node" <<EOF
#!/bin/sh
env >"$root/env.out"
EOF
chmod +x "$root/node"
printf 'CELERIS_WEB_OWNER_SOCKET=/run/prod/owner.sock\nCELERIS_WEB_OWNER_ID=alice\nCELERIS_API_URL=http://127.0.0.1:9999\n' >"$root/config/web.env"

port="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')"
(
  export CELERIS_CONFIG_DIR="$root/config" SD_WEB_NODE="$root/node" SD_WEB_PROBE_TIMEOUT=1
  export CELERIS_WEB_OWNER_SOCKET=/run/outer/owner.sock
  # shellcheck disable=SC1091
  source "$here/lib.sh"
  sd_web_app_probe "$root/app" "$port" aaaaaaaaaaaa || true
) >/dev/null 2>&1
[ -s "$root/env.out" ] || fail "fake node was not started"
grep -qx 'CELERIS_WEB_PROBE=1' "$root/env.out" || fail "CELERIS_WEB_PROBE=1 missing"
! grep -q '^CELERIS_WEB_OWNER_SOCKET=' "$root/env.out" || fail "owner socket leaked"
grep -qx 'CELERIS_WEB_OWNER_ID=alice' "$root/env.out" || fail "other web.env value lost"
grep -qx 'CELERIS_API_URL=http://127.0.0.1:9999' "$root/env.out" || fail "web.env override lost"
grep -qx "CELERIS_WEB_BIND=127.0.0.1:$port" "$root/env.out" || fail "bind missing"
grep -qx 'CELERIS_WEB_RELEASE=aaaaaaaaaaaa' "$root/env.out" || fail "release missing"
echo 'web_probe_owner_isolation: all ok'
