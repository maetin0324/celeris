#!/usr/bin/env bash
# Credential ledger behavior is exercised alongside the shared release/browser-ledger fixture.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
exec bash "$here/browser_ledger_release_stages.sh" "$@"
