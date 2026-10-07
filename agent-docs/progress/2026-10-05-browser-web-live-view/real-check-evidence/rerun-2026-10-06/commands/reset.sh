#!/usr/bin/env bash
# Reset /var/tmp/cb-1001 runtime state between attempts (keeps configs/secrets).
set -euo pipefail
E=/var/tmp/cb-1001
cd "$E"
rm -rf test.sqlite3* checks.json health.json web-health.json *.log page page-response.html denied-page \
  live-upstream-frames.jsonl egress-denied.json launcher.sock web-private/owner.sock browser-identity-keys harness.log w home
mkdir -p w home
sudo -n find "$E/launcher-state" -mindepth 1 -maxdepth 1 ! -name sessions -exec rm -rf {} +
sudo -n find "$E/launcher-state/sessions" -mindepth 1 -maxdepth 1 -exec rm -rf {} +
rm -rf /tmp/celeris-browser-$(id -u)
