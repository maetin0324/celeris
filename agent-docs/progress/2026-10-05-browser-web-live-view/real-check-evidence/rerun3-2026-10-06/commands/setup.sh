#!/usr/bin/env bash
# Fable rerun 3 2026-10-06: prepare the test-only launcher config, daemon config and secrets.
# Everything lives under /var/tmp (never /tmp, never ~/.config/celeris, never production DB/sockets).
set -euo pipefail
umask 077
TREE=/local/celeris/data/workspaces/01M46W97H391DSFW1XJ745W0G9/repos/agent-platform
PREV=$TREE/agent-docs/progress/2026-10-05-browser-web-live-view/real-check-evidence/rerun-2026-10-06/config
TGT=/var/tmp/fable-merge-target/debug
E=/var/tmp/cb-1001
C=/var/tmp/celeris-browser-config-1001
[[ ! -e $E && ! -e $C ]] || { echo "stale $E or $C exists" >&2; exit 1; }

mkdir -p "$E" "$E/home" "$E/harness" "$E/web-private" "$E/credentiald" "$E/w"
chmod 711 "$E"; chmod 700 "$E/web-private"
sudo -n install -d -o celeris-browser -g celeris-browser -m 0700 "$E/launcher-state" "$E/launcher-state/sessions"

cp "$PREV/conformance.json" "$E/conformance.json"
cp "$PREV/org.toml" "$E/org.toml"
cp "$PREV/loopback-acp-harness.py" "$E/harness/loopback-acp-harness.py"
# The script now collects egress-denied.json from the launcher session; keep the harness's own
# note out of that path.
sed 's#REAL_CHECK_DENIAL_FILE = "/var/tmp/cb-1001/egress-denied.json"#REAL_CHECK_DENIAL_FILE = "/var/tmp/cb-1001/harness/agent-denial-note.json"#' \
  "$PREV/daemon.toml" > "$E/daemon.toml"

python3 -c 'import secrets;print(secrets.token_hex(32))' > "$E/api.token"
python3 -c 'import secrets;print(secrets.token_urlsafe(24))' > "$E/web.password"
openssl genpkey -algorithm ed25519 -out "$E/web-private/attestation.key" 2>/dev/null
openssl pkey -in "$E/web-private/attestation.key" -pubout -outform DER | tail -c 32 | xxd -p -c 64 > "$E/attestation.pub"
chmod 600 "$E"/api.token "$E"/web.password "$E/web-private/attestation.key"
chmod 644 "$E/attestation.pub"

sudo -n install -d -o root -g root -m 0755 "$C" "$C/bin"
for b in celeris-browser-launcher celeris-browser-sandboxd celeris-browser-egress; do
  sudo -n install -o root -g root -m 0755 "$TGT/$b" "$C/bin/$b"
done
cat > "$E/launcher.toml.src" <<EOF
socket = "$E/launcher.sock"
state_dir = "$E/launcher-state"
session_root = "$E/launcher-state/sessions"
allowed_uids = [$(id -u)]
bwrap = "/usr/bin/bwrap"
sandboxd = "$C/bin/celeris-browser-sandboxd"
egress = "$C/bin/celeris-browser-egress"
chrome = "/opt/celeris-browser/chrome/chrome"
agent_browser = "/opt/celeris-browser/agent-browser/node_modules/.bin/agent-browser"
resolver = "127.0.0.1"
# TEST ONLY: exactly the allowed loopback page (PAGE_PORT 17730). Never in /etc/celeris-browser.
test_loopback_allow = ["127.0.0.1:17730"]
EOF
sudo -n install -o root -g root -m 0644 "$E/launcher.toml.src" "$C/launcher.toml"
rm "$E/launcher.toml.src"
ls -la "$E" "$C" "$C/bin"
