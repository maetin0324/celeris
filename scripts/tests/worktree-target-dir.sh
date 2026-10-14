#!/bin/sh
# scripts/dev/worktree-target-dir.sh の試験（ADR 2026-10-14-eval-trial-scratch-target）。
# 一時 git repo で: env の CARGO_TARGET_DIR を優先する・既定は scratch 配下で /var/tmp を含まない・置き場が無い／ルート fs なら
# 書かない・同じ basename の 2 worktree が別 target になる、を確かめる。呼び出し: sh scripts/tests/worktree-target-dir.sh
set -u
here="$(cd "$(dirname "$0")/../.." && pwd)"
script="$here/scripts/dev/worktree-target-dir.sh"
work="$(mktemp -d "${TMPDIR:-/tmp}/wtd-test.XXXXXX")" || exit 1
trap 'rm -rf "$work"' EXIT
fails=0
pass(){ echo "ok - $1"; }
fail(){ echo "not ok - $1"; fails=$((fails + 1)); }

mkrepo(){
  mkdir -p "$1"
  git -C "$1" init -q
  printf '[workspace]\n' > "$1/Cargo.toml"
}
# 置き場を別 fs と見なすため、ルート fs の代わりに /proc を指す（試験の一時 dir がどの fs にあっても決定的）。
run(){ env -u CARGO_TARGET_DIR -u CELERIS_BUILD_CACHE WORKTREE_TARGET_DIR_ROOTFS=/proc WORKTREE_TARGET_DIR_QUIET=1 "$@"; }
target_of(){ sed -n 's/^target-dir = "\(.*\)"$/\1/p' "$1/.cargo/config.toml"; }

# 1. CARGO_TARGET_DIR が env にあれば config を書かない（手での呼び出しと hook の両方）
r="$work/env/repo"; mkrepo "$r"
run CARGO_TARGET_DIR="$work/ctd" CELERIS_SCRATCH_DIR="$work/scratch1" bash "$script" "$r"
[ ! -e "$r/.cargo/config.toml" ] && pass "env CARGO_TARGET_DIR: no config" || fail "env CARGO_TARGET_DIR: config written"
mkdir -p "$work/scratch1"
printf '{"cwd":"%s","tool_input":{"command":"cargo test"}}' "$r" \
  | run CARGO_TARGET_DIR="$work/ctd" CELERIS_SCRATCH_DIR="$work/scratch1" bash "$script" --hook
[ ! -e "$r/.cargo/config.toml" ] && pass "env CARGO_TARGET_DIR (hook): no config" || fail "env CARGO_TARGET_DIR (hook): config written"

# 2. 既定: <scratch>/dev-targets/<basename>-<hash>。/var/tmp を含まない
r="$work/default/repo"; mkrepo "$r"; mkdir -p "$work/scratch2"
run CELERIS_SCRATCH_DIR="$work/scratch2" bash "$script" "$r"
t="$(target_of "$r" 2>/dev/null)"
case "$t" in
  "$work/scratch2/dev-targets/repo-"*) [ -d "$t" ] && pass "default: under scratch/dev-targets ($t)" || fail "default: target dir not created";;
  *) fail "default: unexpected target '$t'";;
esac
grep -q '/var/tmp' "$r/.cargo/config.toml" 2>/dev/null && fail "default: config mentions /var/tmp" || pass "default: config has no /var/tmp"
grep -q '/var/tmp' "$script" && fail "script still mentions /var/tmp" || pass "script has no /var/tmp"
# 冪等: 2 回目も同じ内容
before="$(cat "$r/.cargo/config.toml")"
run CELERIS_SCRATCH_DIR="$work/scratch2" bash "$script" "$r"
[ "$before" = "$(cat "$r/.cargo/config.toml")" ] && pass "default: idempotent" || fail "default: not idempotent"
# env が後から入ると、前にこの script が書いた config を消す
run CARGO_TARGET_DIR="$work/ctd" CELERIS_SCRATCH_DIR="$work/scratch2" bash "$script" "$r"
[ ! -e "$r/.cargo/config.toml" ] && pass "env CARGO_TARGET_DIR: stale generated config removed" || fail "env CARGO_TARGET_DIR: stale config kept"

# 3. 置き場が無い: scratch 無し・CELERIS_BUILD_CACHE 無し → 書かない（hook も exit 0）
r="$work/nobase/repo"; mkrepo "$r"
run CELERIS_SCRATCH_DIR="$work/missing-scratch" bash "$script" "$r"; rc=$?
[ $rc -eq 0 ] && [ ! -e "$r/.cargo/config.toml" ] && [ ! -e "$work/missing-scratch" ] \
  && pass "missing scratch: no config, exit 0" || fail "missing scratch: rc=$rc or config written"
run CELERIS_BUILD_CACHE="$work/missing-cache" CELERIS_SCRATCH_DIR="$work/scratch2" bash "$script" "$r"; rc=$?
[ $rc -eq 0 ] && [ ! -e "$r/.cargo/config.toml" ] && [ ! -e "$work/missing-cache" ] \
  && pass "missing CELERIS_BUILD_CACHE: no config, exit 0" || fail "missing CELERIS_BUILD_CACHE: rc=$rc or config written"
printf '{"cwd":"%s","tool_input":{"command":"cargo build"}}' "$r" \
  | run CELERIS_SCRATCH_DIR="$work/missing-scratch" bash "$script" --hook; rc=$?
[ $rc -eq 0 ] && [ ! -e "$r/.cargo/config.toml" ] && pass "missing scratch (hook): no config, exit 0" || fail "missing scratch (hook): rc=$rc"

# 4. 置き場がルート fs 上 → 書かない（ルート fs を一時 dir の fs と見なす）
r="$work/rootfs/repo"; mkrepo "$r"; mkdir -p "$work/cache"
env -u CARGO_TARGET_DIR WORKTREE_TARGET_DIR_ROOTFS="$work" WORKTREE_TARGET_DIR_QUIET=1 \
  CELERIS_BUILD_CACHE="$work/cache" bash "$script" "$r"; rc=$?
[ $rc -eq 0 ] && [ ! -e "$r/.cargo/config.toml" ] && pass "root fs: no config, exit 0" || fail "root fs: rc=$rc or config written"

# 5. 同じ basename "repo" の 2 worktree は別 target
a="$work/trials/t1/repo"; b="$work/trials/t2/repo"; mkrepo "$a"; mkrepo "$b"
run CELERIS_BUILD_CACHE="$work/cache" bash "$script" "$a"
run CELERIS_BUILD_CACHE="$work/cache" bash "$script" "$b"
ta="$(target_of "$a" 2>/dev/null)"; tb="$(target_of "$b" 2>/dev/null)"
[ -n "$ta" ] && [ -n "$tb" ] && [ "$ta" != "$tb" ] && pass "same basename: distinct targets ($ta, $tb)" \
  || fail "same basename: targets '$ta' '$tb'"

if [ "$fails" -ne 0 ]; then echo "FAILED: $fails"; exit 1; fi
echo "all passed"
