#!/usr/bin/env bash
# ADR 2026-11-11-disk-aware-dispatch D3: release-build の target は build の後にも刈られ、上限を超えれば作り直される。
# 守る不変条件: build で育った target が上限を超えたまま lease に残らない。既存の release_target_delete_retry.sh・
# release_prune_stale_test_binaries.sh は開始時の確認だけを見ていて、終了時の確認の欠落を検出できない。
# 本番の状態・cargo・大きな file は使わない（大きさは sd_release_prune_size の差し替えで与える）。
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
root="$(mktemp -d)"
trap 'command rm -rf "$root"' EXIT
export CELERIS_CONFIG_DIR="$root/config" CELERIS_STATE_DIR="$root/state"
source "$here/lib.sh"
export SD_PRUNE_ALLOWED_ROOT="$root"
export SD_RELEASE_TARGET_MAX_BYTES=100
target="$root/target"
tree="$root/tree"
mkdir -p "$target/debug/deps" "$tree"
calls=""
sd_release_prune_stale_test_binaries() { calls="$calls stale:$1:$2:$3"; }
fake_size=50
sd_release_prune_size() { printf "%s\n" "$fake_size"; }
printf '1700000000\n' >"$target/.celeris-release-build-start"
printf 'kept\n' >"$target/debug/deps/kept"
# 上限以下: 刈る段は今の build の開始時刻で呼ばれ、target は残る。
sd_release_target_settle "$target" "$tree"
[ "$calls" = " stale:$target:$tree:1700000000" ]
[ -f "$target/debug/deps/kept" ]
# build で上限を超えた: 作り直す（中身は消え、開始時刻は記録し直される）。
fake_size=200
SD_RELEASE_NOW=1700000100 sd_release_target_settle "$target" "$tree"
[ ! -e "$target/debug/deps/kept" ]
[ "$(cat "$target/.celeris-release-build-start")" = 1700000100 ]
# target が無ければ何もしない。
command rm -rf "$target"
sd_release_target_settle "$target" "$tree"
[ ! -e "$target" ]
# release.sh は終了時（EXIT trap）に settle を呼ぶ。
grep -q "trap '.*sd_release_target_settle \"\$SD_CARGO_TARGET\" \"\$BUILD\"" "$here/release.sh"
echo 'release_target_settled_after_build: ok (pruned and capped after the build)'
