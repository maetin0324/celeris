#!/bin/sh
# scripts/dev/test-affected.sh --list を小さな git fixture（最小 cargo workspace）で確かめる。
# 確かめること: (a) 変更 crate+逆依存 crate、(b) workspace 共通 file で全 crate + 警告、
# (c) web のみ・docs のみは crate を出さない。cargo の試験は走らせない（--list のみ）。
set -eu

here="$(cd "$(dirname "$0")/../../.." && pwd)"
work="$(mktemp -d "${TMPDIR:-/tmp}/test-affected-list.XXXXXX")"
trap 'rm -rf "$work"' EXIT
cd "$work"
git init -q -b main .
git config user.email t@example.com
git config user.name t
d=docs
mkdir -p crates/a/src crates/b/src crates/c/src web "$d"
printf '[workspace]\nmembers = ["crates/a", "crates/b", "crates/c"]\nresolver = "2"\n' >Cargo.toml
mk() { # name [dep]
  printf '[package]\nname = "%s"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\n' "$1" >"crates/$1/Cargo.toml"
  [ -z "${2:-}" ] || printf '%s = { path = "../%s" }\n' "$2" "$2" >>"crates/$1/Cargo.toml"
  echo "" >"crates/$1/src/lib.rs"
}
mk a
mk b a
mk c
echo '{"scripts":{"test":"vitest run"},"packageManager":"pnpm@1.0.0"}' >web/package.json
echo x >"$d"/x.md
git add -A
git commit -q -m base
base="$(git rev-parse HEAD)"

fail() { echo "FAIL: $*" >&2; exit 1; }
run() { CELERIS_WU_BASE="$base" bash "$here/scripts/dev/test-affected.sh" --list 2>"$work/err"; }

# (a) a を変更 → a と逆依存 b。c は出ない。未追跡 file も数える。
echo "// x" >>crates/a/src/lib.rs
[ "$(run)" = "a
b" ] || fail "(a) changed a: got [$(run)]"
git checkout -q -- .
echo "// new" >crates/c/src/new.rs
[ "$(run)" = "c" ] || fail "(a) untracked c: got [$(run)]"
rm crates/c/src/new.rs

# (b) Cargo.lock → 全 crate + 警告
echo "# x" >Cargo.lock
[ "$(run)" = "a
b
c" ] || fail "(b) all: got [$(run)]"
grep -q '全 crate' "$work/err" || fail "(b) no warning"
rm Cargo.lock

# (c) web のみ・docs のみ → 空
echo "// w" >web/a.ts
[ -z "$(run)" ] || fail "(c) web only not empty"
rm web/a.ts
echo y >>"$d"/x.md
[ -z "$(run)" ] || fail "(c) docs only not empty"
CELERIS_WU_BASE="$base" bash "$here/scripts/dev/test-affected.sh" 2>/dev/null || fail "(c) docs only exit"

echo "test-affected-list: ok"
