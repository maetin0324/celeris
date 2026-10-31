#!/bin/sh
# 漏洩検査（check-ops-r1-leak.py）の試験を、どの cwd からでも起動する。
# 使い方: sh scripts/campaign/run-ops-r1-leak-tests.sh
cd "$(dirname "$0")" || exit 1
exec python3 -m unittest test_check_ops_r1_leak
