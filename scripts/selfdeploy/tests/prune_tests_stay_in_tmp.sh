#!/usr/bin/env bash
set -euo pipefail
repo="$(cd "$(dirname "$0")/../../.." && pwd)"
tests="$repo/scripts/selfdeploy/tests"
# Construct forbidden production path fragments so this file cannot match itself.
prod_data='/local/celeris/'"data"
prod_state='/local/celeris/'"state"
for f in "$tests"/*.sh; do
  [ "$f" = "$0" ] && continue
  if grep -Fq "$prod_data" "$f" || grep -Fq "$prod_state" "$f"; then
    echo "production path literal in ${f##*/}" >&2
    exit 1
  fi
done
outer="$(mktemp -d)"
inner="$(mktemp -d)"
trap 'rm -rf "$outer" "$inner"' EXIT
mkdir -p "$outer/target/debug/deps" "$outer/backups"
printf fixture >"$outer/target/debug/deps/fixture-aaaaaaaa"
printf 'fixture-aaaaaaaa: src/lib.rs\n' >"$outer/target/debug/deps/fixture-aaaaaaaa.d"
printf fixture >"$outer/backups/20261001-000001-pre-000000000001.sqlite3"
if SD_PRUNE_ALLOWED_ROOT="$inner" bash -c 'source "$1"; sd_release_prune_stale_test_binaries "$2" "$3" 9999999999' _ "$repo/scripts/selfdeploy/lib.sh" "$outer/target" "$outer" 2>/dev/null; then
  echo 'stale binary guard accepted outside path' >&2; exit 1
fi
[ -f "$outer/target/debug/deps/fixture-aaaaaaaa" ]
if SD_PRUNE_ALLOWED_ROOT="$inner" bash -c 'source "$1"; sd_release_prune_enforce_limit "$2"' _ "$repo/scripts/selfdeploy/lib.sh" "$outer/target" 2>/dev/null; then
  echo 'target size guard accepted outside path' >&2; exit 1
fi
[ -f "$outer/target/debug/deps/fixture-aaaaaaaa" ]
if SD_PRUNE_ALLOWED_ROOT="$inner" sh "$repo/scripts/selfdeploy/prune-backups.sh" "$outer/backups" 2>/dev/null; then
  echo 'backup guard accepted outside path' >&2; exit 1
fi
[ -f "$outer/backups/20261001-000001-pre-000000000001.sqlite3" ]
echo 'prune_tests_stay_in_tmp: ok'
