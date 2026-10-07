#!/bin/sh
# scripts/dev/check-test-tmp-leftovers.sh <cargo test args...> — 試験が TMPDIR に一時 dir を残さないことを検査する。
# 新しい空 dir を TMPDIR にして `cargo test <args>` を走らせ、終了後に残った項目を表示して非 0 で終わる。
# 例: sh scripts/dev/check-test-tmp-leftovers.sh -p task-core
# userns が要る試験は既定の skip のまま（CELERIS_USERNS_TESTS は触らない）。
set -u
base="$(mktemp -d "${TMPDIR:-/tmp}/celeris-tmp-leftovers.XXXXXX")" || exit 2
trap 'chmod -R u+w "$base" 2>/dev/null; rm -rf "$base"' EXIT INT TERM
mkdir "$base/tmp" || exit 2
TMPDIR="$base/tmp" cargo test "$@"
rc=$?
left="$(ls -A "$base/tmp")"
if [ -n "$left" ]; then
  echo "check-test-tmp-leftovers: leftovers in TMPDIR after cargo test $*:" >&2
  echo "$left" | sed 's/^/  /' >&2
  exit 1
fi
if [ "$rc" -ne 0 ]; then
  echo "check-test-tmp-leftovers: cargo test failed (exit $rc)" >&2
  exit "$rc"
fi
echo "check-test-tmp-leftovers: no leftovers"
