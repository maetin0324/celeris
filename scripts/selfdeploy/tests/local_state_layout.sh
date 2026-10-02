#!/usr/bin/env bash
# ADR-0136: selfdeploy uses the selected state tree and keeps legacy defaults.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT
mkdir -p "$root/home" "$root/config" "$root/hot"

fail() {
  printf 'FAIL: %s\n' "$*" >&2
  [ ! -f "$root/release-test.log" ] || tail -n 30 "$root/release-test.log" >&2
  exit 1
}

# A child shell gives each case a fresh lib.sh initialization. Its HOME is
# disposable, so neither case can read the production config or state tree.
env -u CELERIS_STATE_DIR -u CELERIS_BACKUPS_DIR -u CELERIS_LOGS_DIR \
  HOME="$root/home" CELERIS_CONFIG_DIR="$root/config" LIB="$here/lib.sh" \
  bash -c '
    set -euo pipefail
    . "$LIB"
    [ "$SD_RELEASES" = "$HOME/.local/celeris/releases" ]
    [ "$SD_STAGING" = "$HOME/.local/celeris/staging" ]
    [ "$SD_TOOLS" = "$HOME/.local/celeris/tools" ]
    [ "$SD_BACKUPS" = "$HOME/.local/celeris/backups" ]
    [ "$SD_LOGS" = "$SD_BACKUPS" ]
    [ "$SD_BUILD_ROOT" = "$SD_RELEASES/.build" ]
    [ "$SD_PNPM_PROD_CACHE" = "$SD_RELEASES/.pnpm-prod-cache" ]
    [ "$SD_CARGO_TARGET" = "$SD_RELEASES/.cargo-target" ]
  ' || fail 'legacy defaults changed'

HOME="$root/home" CELERIS_CONFIG_DIR="$root/config" \
  CELERIS_STATE_DIR="$root/hot" CELERIS_BACKUPS_DIR="$root/hot/backups" \
  CELERIS_LOGS_DIR="$root/hot/logs" LIB="$here/lib.sh" \
  bash -c '
    set -euo pipefail
    . "$LIB"
    [ "$SD_RELEASES" = "$CELERIS_STATE_DIR/releases" ]
    [ "$SD_STAGING" = "$CELERIS_STATE_DIR/staging" ]
    [ "$SD_TOOLS" = "$CELERIS_STATE_DIR/tools" ]
    [ "$SD_BACKUPS" = "$CELERIS_STATE_DIR/backups" ]
    [ "$SD_LOGS" = "$CELERIS_STATE_DIR/logs" ]
    [ "$SD_CURRENT" = "$CELERIS_STATE_DIR/current" ]
    [ "$SD_PREVIOUS" = "$CELERIS_STATE_DIR/previous" ]
    [ "$SD_BUILD_TREE" = "$CELERIS_STATE_DIR/releases/.build/tree" ]
    [ "$SD_PNPM_PROD_CACHE" = "$CELERIS_STATE_DIR/releases/.pnpm-prod-cache" ]
    [ "$SD_CARGO_TARGET" = "$CELERIS_STATE_DIR/releases/.cargo-target" ]
    sd_mkdirs
    [ -d "$SD_BUILD_ROOT" ] && [ -d "$SD_BACKUPS" ]
  ' || fail 'configured hot layout is wrong'

[ ! -e "$root/home/.local/celeris" ] || fail 'hot layout wrote under HOME'

# This existing isolated release fixture uses stub cargo, pnpm, node and
# celerisctl. It verifies the actual release payload, build tree and prod
# dependency cache under a temporary CELERIS_STATE_DIR.
bash "$here/tests/release_gui_skip_and_shared_tree.sh" >"$root/release-test.log" 2>&1 \
  || fail 'isolated release did not keep payload, build and pnpm cache in the selected state tree'
if grep -q 'scratch dir is on NFS' "$root/release-test.log"; then
  fail 'local scratch was reported as NFS'
fi

printf 'ok local state layout and legacy defaults\n'
