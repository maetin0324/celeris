#!/bin/sh
# 漏洩検査（check-ops-r1-leak.py）と、漏洩検査が読む A・B の種（ops-r2-seed.sh make）の試験を、どの cwd からでも起動する。
# 使い方: sh scripts/campaign/run-ops-r1-leak-tests.sh
cd "$(dirname "$0")" || exit 1
exec python3 -m unittest test_check_ops_r1_leak test_ops_r2_seed
