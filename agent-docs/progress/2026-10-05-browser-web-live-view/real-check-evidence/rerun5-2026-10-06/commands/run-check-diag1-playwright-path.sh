#!/usr/bin/env bash
# Fable rerun 5 2026-10-06: scripts/dev/browser-web-live-check.sh unpatched (f5b9dc4e), test daemon, loopback only.
# ROOT is a git-archive copy of f5b9dc4e in /var/tmp/fable-rerun5-src (web/node_modules + dist added there;
# the child tree has no node_modules and is used read-only).
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
# Diagnostic 1 (wrapper only, script unpatched): HOME is redirected to the test dir, so Playwright looked for
# its browsers under $HOME/.cache. Point it at the operator's existing read-only Playwright cache.
export PLAYWRIGHT_BROWSERS_PATH=/home/rmaeda/.cache/ms-playwright
SCRIPT=${SCRIPT:-/var/tmp/fable-rerun5-src/scripts/dev/browser-web-live-check.sh}
bash "$SCRIPT"
