#!/usr/bin/env bash
# Deterministic deletion failures; no release, cargo, or production state.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
root="$(mktemp -d)"
trap 'command rm -rf "$root"' EXIT
export CELERIS_CONFIG_DIR="$root/config" CELERIS_STATE_DIR="$root/state"
source "$here/lib.sh"
export SD_PRUNE_ALLOWED_ROOT="$root"
export SD_RELEASE_TARGET_MAX_BYTES=1
export SD_RELEASE_TARGET_SEED="$root/seed"
mkdir -p "$SD_RELEASE_TARGET_SEED"
printf 'seed\n' > "$SD_RELEASE_TARGET_SEED/seed-marker"
sd_release_prune_size() { printf '2\n'; }
sd_release_prune_record_start() { printf 'started\n' > "$1/start-marker"; }
target="$root/target"
mode=transient
calls=0
rm() {
  if [ "$2" = "$target" ]; then
    calls=$((calls + 1))
    if [ "$mode" = persistent ]; then return 1; fi
    if [ "$calls" -eq 1 ]; then
      command rm -rf "$target"
      mkdir -p "$target/debug/deps"
      printf 'late write\n' > "$target/debug/deps/late"
      printf 'rm: Directory not empty\n' >&2
      return 1
    fi
  fi
  command rm "$@"
}
mkdir -p "$target"
sd_release_prune_enforce_limit "$target"
[ "$calls" -eq 2 ]
[ -f "$target/seed-marker" ]
[ -f "$target/start-marker" ]
[ ! -e "$target/debug/deps/late" ]
[ ! -e "${target}.recreate.$$" ]
command rm -rf "$target"
mkdir -p "$target"
printf 'original\n' > "$target/original"
mode=persistent
calls=0
if sd_release_prune_enforce_limit "$target"; then
  echo 'persistent deletion failure must stop recreation' >&2
  exit 1
fi
[ "$calls" -eq 3 ]
[ -f "$target/original" ]
[ ! -e "$target/start-marker" ]
[ ! -e "$target/seed-marker" ]
[ ! -e "${target}.recreate.$$" ]
echo 'release_target_delete_retry: ok (transient recovery and persistent failure)'
