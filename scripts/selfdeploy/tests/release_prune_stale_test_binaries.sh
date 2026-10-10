#!/usr/bin/env bash
# Deterministic D3 coverage using a fake cargo and a temporary scratch lease.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT
mkdir -p "$root/bin" "$root/config" "$root/scratch/targets/release-build/target/debug/deps" "$root/scratch/targets/release-build/target/debug/build"
git init -q "$root/repo"
mkdir -p "$root/repo/crates/example/src"
printf '[workspace]\nmembers = ["crates/example"]\n' >"$root/repo/Cargo.toml"
printf '[package]\nname = "example-crate"\nversion = "0.1.0"\nedition = "2021"\n' >"$root/repo/crates/example/Cargo.toml"
printf 'pub fn example() {}\n' >"$root/repo/crates/example/src/lib.rs"
git -C "$root/repo" add .
git -C "$root/repo" -c user.email=t@example.invalid -c user.name=t commit -q -m init
sha="$(git -C "$root/repo" rev-parse HEAD)"
printf '#!/usr/bin/env bash\nexit 1\n' >"$root/bin/cargo"
printf '#!/usr/bin/env bash\nexit 0\n' >"$root/bin/pnpm"
cat >"$root/bin/celerisctl" <<'EOF'
#!/usr/bin/env bash
if [ "$1 $2" = "scratch lease" ]; then printf '%s\n' "$FAKE_SCRATCH/targets/release-build/target"; fi
exit 0
EOF
chmod +x "$root/bin/"*
: >"$root/config/config.toml"
target="$root/scratch/targets/release-build/target"
printf 'old executable\n' >"$target/debug/deps/example_crate-aaaaaaaa"
printf '%s\n' "$root/repo/crates/example/src/lib.rs" >"$target/debug/deps/example_crate-aaaaaaaa.d"
printf 'new executable\n' >"$target/debug/deps/example_crate-bbbbbbbb"
printf 'new deps\n' >"$target/debug/deps/example_crate-bbbbbbbb.d"
printf 'dependency archive\n' >"$target/debug/deps/libserde-cccccccc.rlib"
printf 'build output\n' >"$target/debug/build/placeholder"
touch -d @100 "$target/debug/deps/example_crate-aaaaaaaa" "$target/debug/deps/example_crate-aaaaaaaa.d"
touch -d @300 "$target/debug/deps/example_crate-bbbbbbbb" "$target/debug/deps/example_crate-bbbbbbbb.d" "$target/debug/deps/libserde-cccccccc.rlib" "$target/debug/build/placeholder"
printf '150\n' >"$target/.celeris-release-build-start"
run_release() {
  CELERIS_STATE_DIR="$root/state" CELERIS_CONFIG_DIR="$root/config" CELERIS_CONFIG="$root/config/config.toml" \
    SD_REPO="$root/repo" SD_CELERISCTL="$root/bin/celerisctl" SD_PNPM_SHIM_DIR="$root/bin" \
    FAKE_SCRATCH="$root/scratch" SD_RELEASE_NOW=200 SD_GATE_TEST_RUNNER=cargo-test \
    PATH="$root/bin:$PATH" bash "$here/release.sh" "$sha" >"$root/release.out" 2>&1
}
if run_release; then echo 'fake cargo unexpectedly succeeded' >&2; exit 1; fi
[ -e "$target/debug/deps/example_crate-aaaaaaaa" ] && { cat "$root/release.out" >&2; exit 1; }
[ ! -e "$target/debug/deps/example_crate-aaaaaaaa.d" ]
[ -e "$target/debug/deps/example_crate-bbbbbbbb" ]
[ -e "$target/debug/deps/example_crate-bbbbbbbb.d" ]
[ -e "$target/debug/deps/libserde-cccccccc.rlib" ]
[ -e "$target/debug/build/placeholder" ]
grep -q 'removed [1-9][0-9]* bytes' "$root/release.out"
if SD_RELEASE_TARGET_MAX_BYTES=1 run_release; then echo 'fake cargo unexpectedly succeeded after recreation' >&2; exit 1; fi
grep -q 'exceeds 1; recreating target' "$root/release.out"
[ -f "$target/.celeris-release-build-start" ]
[ "$(cat "$target/.celeris-release-build-start")" = 200 ]
echo 'release_prune_stale_test_binaries: ok'
