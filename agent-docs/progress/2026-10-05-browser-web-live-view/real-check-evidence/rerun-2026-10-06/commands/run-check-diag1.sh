#!/usr/bin/env bash
# Fable rerun 2026-10-06: scripts/dev/browser-web-live-check.sh unpatched, test daemon, loopback only.
E=/var/tmp/cb-1001
C=/var/tmp/celeris-browser-config-1001
export HOME=$E/home   # keep the test daemon away from ~/.config/celeris
export CELERIS_BROWSER_REAL_CHECK=1 CELERIS_USERNS_TESTS=1
export CELERIS_BROWSER_EVIDENCE_DIR=$E
export CELERIS_BROWSER_TEST_LAUNCHER_CONFIG=$C/launcher.toml
export CELERIS_BROWSER_TEST_DAEMON_CONFIG=$E/daemon.toml
export CELERIS_BROWSER_TEST_LAUNCHER_BIN=$C/bin/celeris-browser-launcher
export CELERIS_BROWSER_TEST_DAEMON_BIN=/var/tmp/fable-merge-target/debug/celeris
export CELERIS_BROWSER_TEST_LAUNCHER_USER=celeris-browser
export CELERIS_BROWSER_CONFORMANCE_FILE=$E/conformance.json
export CELERIS_WEB_PASSWORD_FILE=$E/web.password
export CELERIS_WEB_ATTESTATION_KEY_FILE=$E/web-private/attestation.key
export CELERIS_WEB_OWNER_SOCKET=$E/web-private/owner.sock
export CELERIS_BROWSER_TEST_DENIAL_FILE=$E/egress-denied.json
cd /local/celeris/data/workspaces/01M46W97H391DSFW1XJ745W0G9/repos/agent-platform
bash /tmp/claude-1001/-home-rmaeda-workspace-agent-platform/ae36633b-4d02-4b11-975b-c8070527ae25/scratchpad/browser-web-live-check.diag1.sh
