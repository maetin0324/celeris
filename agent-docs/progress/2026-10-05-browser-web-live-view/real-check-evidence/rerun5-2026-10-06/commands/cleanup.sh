#!/usr/bin/env bash
# Fable rerun 5: remove all test state after evidence was copied. Never touches production paths.
set -euo pipefail
sudo -n rm -rf /var/tmp/cb-1001 /var/tmp/celeris-browser-config-1001
rm -rf /var/tmp/fable-rerun5-src /var/tmp/fable-rerun5-out[123].txt /var/tmp/fable-rerun5-start[123].txt /var/tmp/fable-rerun5-harness-v1.py
