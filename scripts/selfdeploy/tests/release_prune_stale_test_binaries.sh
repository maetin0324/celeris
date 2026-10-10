#!/usr/bin/env bash
# Deterministic D3 coverage using a fake cargo and a temporary scratch lease.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT
mkdir -p "$root/bin" "$root/config" "$root/scratch/targets/release-build/target/debug/deps" "$root/scratch/targets/release-build/target/debug/build"
git init -q "$root/repo"
mkdir -p "$root/repo/crates/example/src" "$root/repo/crates/example/tests" "$root/repo/tests/e2e/src" "$root/repo/tests/e2e/tests"
printf '[workspace]\nmembers = ["crates/example", "tests/e2e"]\n' >"$root/repo/Cargo.toml"
printf '[package]\nname = "example-crate"\nversion = "0.1.0"\nedition = "2021"\n' >"$root/repo/crates/example/Cargo.toml"
printf 'pub fn example() {}\n' >"$root/repo/crates/example/src/lib.rs"
# Integration test binaries are named for the test file (notify-<hash>), not for the package.
printf '#[test]\nfn notify() {}\n' >"$root/repo/crates/example/tests/notify.rs"
# A member outside crates/ (like tests/e2e) with its own integration test.
printf '[package]\nname = "example-e2e"\nversion = "0.1.0"\nedition = "2021"\n' >"$root/repo/tests/e2e/Cargo.toml"
printf '\n' >"$root/repo/tests/e2e/src/lib.rs"
printf '#[test]\nfn scenario() {}\n' >"$root/repo/tests/e2e/tests/api_scenarios.rs"
git -C "$root/repo" add .
git -C "$root/repo" -c user.email=t@example.invalid -c user.name=t commit -q -m init
sha="$(git -C "$root/repo" rev-parse HEAD)"
# cargo fails every step (release.sh stops right after the prune). `cargo metadata` answers only
# when FAKE_CARGO_METADATA names a JSON file; otherwise the prune uses its manifest fallback.
cat >"$root/bin/cargo" <<'EOF'
#!/usr/bin/env bash
if [ "$1" = metadata ] && [ -n "${FAKE_CARGO_METADATA:-}" ]; then cat "$FAKE_CARGO_METADATA"; exit 0; fi
exit 1
EOF
printf '#!/usr/bin/env bash\nexit 0\n' >"$root/bin/pnpm"
cat >"$root/bin/celerisctl" <<'EOF'
#!/usr/bin/env bash
if [ "$1 $2" = "scratch lease" ]; then printf '%s\n' "$FAKE_SCRATCH/targets/release-build/target"; fi
exit 0
EOF
chmod +x "$root/bin/"*
: >"$root/config/config.toml"
target="$root/scratch/targets/release-build/target"
deps="$target/debug/deps"
registry="/home/u/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f"
# dep_file <stem-or-lib> <source>: a cargo .d file whose first rule names <source>.
dep_file() { local stem="${1#lib}"; printf '%s: %s\n' "$deps/$1" "$2" >"$deps/${stem%.rlib}.d"; }
printf 'old executable\n' >"$deps/example_crate-aaaaaaaa"
dep_file example_crate-aaaaaaaa "$root/repo/crates/example/src/lib.rs"
printf 'new executable\n' >"$deps/example_crate-bbbbbbbb"
printf 'new deps\n' >"$deps/example_crate-bbbbbbbb.d"
printf 'dependency archive\n' >"$deps/libserde-cccccccc.rlib"
printf 'build output\n' >"$target/debug/build/placeholder"
# Old integration test binary of a crates/ member (test file name, not package name).
printf 'old integration test\n' >"$deps/notify-dddddddd"
dep_file notify-dddddddd "$root/repo/crates/example/tests/notify.rs"
# Old integration test binary of the member outside crates/.
printf 'old e2e test\n' >"$deps/api_scenarios-eeeeeeee"
dep_file api_scenarios-eeeeeeee "$root/repo/tests/e2e/tests/api_scenarios.rs"
# Dependency crate with the same name as a workspace target (notify): its .d points at the registry.
printf 'dependency rlib\n' >"$deps/libnotify-ffffffff.rlib"
printf 'dependency rmeta\n' >"$deps/libnotify-ffffffff.rmeta"
dep_file libnotify-ffffffff.rlib "$registry/notify-6.1.1/src/lib.rs"
# Dependency from a git checkout whose name equals a workspace package.
printf 'git dependency rlib\n' >"$deps/libexample_e2e-44444444.rlib"
printf '%s: %s\n' "$deps/libexample_e2e-44444444.rlib" "/home/u/.cargo/git/checkouts/example-e2e-0123/abcdef0/src/lib.rs" >"$deps/example_e2e-44444444.d"
# Old library outputs of the workspace crate (rlib + rmeta) are pruned with their .d.
printf 'old ws rlib\n' >"$deps/libexample_crate-11111111.rlib"
printf 'old ws rmeta\n' >"$deps/libexample_crate-11111111.rmeta"
dep_file libexample_crate-11111111.rlib "$root/repo/crates/example/src/lib.rs"
# A [[test]] target whose name matches no file: only cargo metadata knows it.
printf 'old custom test\n' >"$deps/custom_name-22222222"
dep_file custom_name-22222222 "$root/repo/crates/example/tests/odd/path.rs"
# Integration test binary built after the marker stays.
printf 'new integration test\n' >"$deps/notify-33333333"
dep_file notify-33333333 "$root/repo/crates/example/tests/notify.rs"
touch -d @100 "$deps/example_crate-aaaaaaaa" "$deps/example_crate-aaaaaaaa.d" \
  "$deps/notify-dddddddd" "$deps/notify-dddddddd.d" "$deps/api_scenarios-eeeeeeee" "$deps/api_scenarios-eeeeeeee.d" \
  "$deps/libnotify-ffffffff.rlib" "$deps/libnotify-ffffffff.rmeta" "$deps/notify-ffffffff.d" \
  "$deps/libexample_e2e-44444444.rlib" "$deps/example_e2e-44444444.d" \
  "$deps/libexample_crate-11111111.rlib" "$deps/libexample_crate-11111111.rmeta" "$deps/example_crate-11111111.d" \
  "$deps/custom_name-22222222" "$deps/custom_name-22222222.d"
touch -d @300 "$deps/example_crate-bbbbbbbb" "$deps/example_crate-bbbbbbbb.d" "$deps/libserde-cccccccc.rlib" \
  "$target/debug/build/placeholder" "$deps/notify-33333333" "$deps/notify-33333333.d"
printf '150\n' >"$target/.celeris-release-build-start"
run_release() {
  CELERIS_STATE_DIR="$root/state" CELERIS_CONFIG_DIR="$root/config" CELERIS_CONFIG="$root/config/config.toml" \
    SD_REPO="$root/repo" SD_CELERISCTL="$root/bin/celerisctl" SD_PNPM_SHIM_DIR="$root/bin" \
    FAKE_SCRATCH="$root/scratch" SD_RELEASE_NOW=200 SD_GATE_TEST_RUNNER=cargo-test \
    PATH="$root/bin:$PATH" bash "$here/release.sh" "$sha" >"$root/release.out" 2>&1
}
fail() { echo "release_prune_stale_test_binaries: $*" >&2; cat "$root/release.out" >&2; exit 1; }
# 1. Dry run: nothing is removed, candidates are reported
#    (3 stale test binaries + 1 stale workspace lib = 4 .d + 5 outputs = 9 files).
if SD_RELEASE_PRUNE_DRY_RUN=1 run_release; then fail 'fake cargo unexpectedly succeeded (dry run)'; fi
grep -q 'release prune: dry run: would remove 9 files ([1-9][0-9]* bytes allocated).*names from fallback; kept 2 dependency-crate' "$root/release.out" \
  || fail 'dry run did not report 9 candidates'
for f in example_crate-aaaaaaaa notify-dddddddd api_scenarios-eeeeeeee libexample_crate-11111111.rlib custom_name-22222222; do
  [ -e "$deps/$f" ] || fail "dry run removed $f"
done
printf '150\n' >"$target/.celeris-release-build-start"
# 2. Fallback naming (cargo metadata fails): package names + tests/*.rs etc. of every member.
if run_release; then fail 'fake cargo unexpectedly succeeded'; fi
grep -q 'release prune: removed 9 files ([1-9][0-9]* bytes allocated).*names from fallback; kept 2 dependency-crate' "$root/release.out" \
  || fail 'fallback prune count'
for f in example_crate-aaaaaaaa example_crate-aaaaaaaa.d notify-dddddddd notify-dddddddd.d \
  api_scenarios-eeeeeeee api_scenarios-eeeeeeee.d \
  libexample_crate-11111111.rlib libexample_crate-11111111.rmeta example_crate-11111111.d; do
  [ ! -e "$deps/$f" ] || fail "stale $f was not pruned"
done
for f in example_crate-bbbbbbbb example_crate-bbbbbbbb.d libserde-cccccccc.rlib \
  libnotify-ffffffff.rlib libnotify-ffffffff.rmeta notify-ffffffff.d \
  libexample_e2e-44444444.rlib example_e2e-44444444.d notify-33333333 notify-33333333.d \
  custom_name-22222222 custom_name-22222222.d; do
  [ -e "$deps/$f" ] || fail "$f must be kept"
done
[ -e "$target/debug/build/placeholder" ] || fail 'build script output must be kept'
# 3. cargo metadata naming: the [[test]] target whose name matches no file is pruned too.
cat >"$root/metadata.json" <<'EOF'
{"packages":[
 {"name":"example-crate","targets":[{"kind":["lib"],"name":"example_crate"},{"kind":["test"],"name":"custom-name"},{"kind":["test"],"name":"notify"}]},
 {"name":"example-e2e","targets":[{"kind":["lib"],"name":"example_e2e"},{"kind":["test"],"name":"api_scenarios"}]}]}
EOF
if FAKE_CARGO_METADATA="$root/metadata.json" run_release; then fail 'fake cargo unexpectedly succeeded (metadata)'; fi
grep -q 'release prune: removed 2 files .*names from metadata; kept 2 dependency-crate' "$root/release.out" || fail 'metadata prune count'
[ ! -e "$deps/custom_name-22222222" ] || fail 'metadata-only target was not pruned'
for f in libnotify-ffffffff.rlib notify-ffffffff.d libexample_e2e-44444444.rlib notify-33333333 example_crate-bbbbbbbb; do
  [ -e "$deps/$f" ] || fail "$f must be kept (metadata run)"
done
# Apparent size alone must not trigger a rebuild. First exercise btrfs output.
truncate -s 16M "$target/debug/sparse-fixture"
cat >"$root/bin/btrfs" <<'EOF'
#!/usr/bin/env bash
printf 'Total Exclusive Set shared Filename\n99999999 0 0 %s\n' "$4"
EOF
chmod +x "$root/bin/btrfs"
if SD_RELEASE_TARGET_MAX_BYTES=1048576 run_release; then echo 'fake cargo unexpectedly succeeded with btrfs size fixture' >&2; exit 1; fi
if grep -q 'exceeds 1048576; recreating target' "$root/release.out"; then
  echo 'btrfs unique size was not used' >&2
  exit 1
fi
[ -e "$target/debug/sparse-fixture" ]
# Force btrfs unavailable to exercise portable inode/block accounting too.
cat >"$root/bin/btrfs" <<'EOF'
#!/usr/bin/env bash
exit 1
EOF
chmod +x "$root/bin/btrfs"
if SD_RELEASE_TARGET_MAX_BYTES=1048576 run_release; then echo 'fake cargo unexpectedly succeeded with sparse target' >&2; exit 1; fi
if grep -q 'exceeds 1048576; recreating target' "$root/release.out"; then
  echo 'sparse target was incorrectly counted by apparent size' >&2
  exit 1
fi
[ -e "$target/debug/sparse-fixture" ]
if SD_RELEASE_TARGET_MAX_BYTES=1 run_release; then echo 'fake cargo unexpectedly succeeded after recreation' >&2; exit 1; fi
grep -q 'exceeds 1; recreating target' "$root/release.out"
[ -f "$target/.celeris-release-build-start" ]
[ "$(cat "$target/.celeris-release-build-start")" = 200 ]
echo 'release_prune_stale_test_binaries: ok'
