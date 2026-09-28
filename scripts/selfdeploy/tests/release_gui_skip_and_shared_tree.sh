#!/usr/bin/env bash
# Phase SD-1: release.sh の
#   (1) GUI の検査だけの段（pnpm-test / pnpm-mobile-audit / pnpm-e2e-mock）を、`current` から gui/ に変更が
#       無いときだけ飛ばし、gate.json に `skipped: true, reason: "no change under gui/"` を残す。gui/ が
#       変わった・`current` が無い・`SD_GATE_FORCE_GUI=1` のときは飛ばさない。Rust の段は常に回す。
#   (2) 作業ツリー `.build/tree` を使い回す（2 回目は tree_reused、target が同じ作業ツリーから作られていれば
#       cargo-workspace-clean を飛ばす）。
#   (3) gui の本番依存を `.pnpm-prod-cache/<key>` で 1 度だけ install し、リリースの gui/node_modules を
#       そこへの相対 symlink にする。lockfile が変わると別の key になり、指されなくなった entry は消える。
# 偽の cargo / pnpm / node / celerisctl を使い、本番のパス・ネットワークには触れない（一時ディレクトリだけ）。
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT
mkdir -p "$root/bin" "$root/config" "$root/state/releases"
: >"$root/config/config.toml"

fail() {
  echo "FAIL: $*" >&2
  [ -f "$root/release.out" ] && tail -n 40 "$root/release.out" >&2
  exit 1
}

# ---- 偽の道具 ----------------------------------------------------------------

cat >"$root/bin/cargo" <<'EOF'
#!/usr/bin/env bash
printf '%s\t%s\n' "${CARGO_TARGET_DIR:-}" "$*" >>"$CARGO_LOG"
case "$1" in
  metadata) printf '{"workspace_members":["x 0.1.0"],"packages":[{"id":"x 0.1.0","name":"x"}]}\n' ;;
  nextest)
    case "$2" in
      --version) echo 'cargo-nextest 0.9.146 (fake)' ;;
      run)
        printf '    Starting 3 tests across 1 binary (1 test skipped)\n'
        printf '     Summary [   0.010s] 3 tests run: 3 passed, 1 skipped\n'
        ;;
    esac
    ;;
  test)
    if [ "${2:-}" != --doc ]; then
      printf '     Running unittests src/lib.rs (target/debug/deps/x-1)\n'
      printf 'test result: ok. 3 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.01s\n'
    fi
    printf '   Doc-tests x\n'
    printf 'test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n'
    ;;
  build)
    mkdir -p "$CARGO_TARGET_DIR/release"
    for b in celeris celerisctl; do printf '#!/bin/sh\nexit 0\n' >"$CARGO_TARGET_DIR/release/$b"; chmod +x "$CARGO_TARGET_DIR/release/$b"; done
    ;;
esac
exit 0
EOF
cat >"$root/bin/pnpm" <<'EOF'
#!/usr/bin/env bash
printf '%s\t%s\n' "$PWD" "$*" >>"$PNPM_LOG"
case "$1" in
  --version) echo 11.27.0 ;;
  install) mkdir -p node_modules && : >node_modules/.modules.yaml ;;
  build) mkdir -p build && echo ok >build/index.html ;;
esac
exit 0
EOF
cat >"$root/bin/node" <<'EOF'
#!/usr/bin/env bash
[ "${1:-}" = --version ] && echo v24.0.0
exit 0
EOF
cat >"$root/bin/celerisctl" <<'EOF'
#!/usr/bin/env bash
if [ "$1 $2" = "scratch lease" ]; then
  owner=""
  while [ $# -gt 0 ]; do
    [ "$1" = "--owner" ] && owner="$2"
    shift
  done
  printf '%s\n' "$FAKE_SCRATCH/targets/$owner/target"
fi
exit 0
EOF
chmod +x "$root/bin/"*

# ---- 元のリポジトリ ------------------------------------------------------------

repo="$root/repo"
mkdir -p "$repo/gui" "$repo/crates/task-core/src" "$repo/crates/celeris" "$repo/crates/x/src" "$repo/scripts/selfdeploy"
printf '{"name":"celeris-gui","version":"0.1.0","packageManager":"pnpm@11.27.0"}\n' >"$repo/gui/package.json"
printf 'lockfileVersion: 9.0\n' >"$repo/gui/pnpm-lock.yaml"
printf 'packages:\n  - "."\n' >"$repo/gui/pnpm-workspace.yaml"
printf '// server\n' >"$repo/gui/server.js"
printf 'pub const SCHEMA_VERSION: u32 = 7;\n' >"$repo/crates/task-core/src/store.rs"
printf '[package]\nname = "celeris"\nversion = "0.1.0"\n' >"$repo/crates/celeris/Cargo.toml"
printf '// v0\n' >"$repo/crates/x/src/lib.rs"
cp "$here"/*.sh "$repo/scripts/selfdeploy/"
chmod +x "$repo/scripts/selfdeploy/"*.sh
mkdir -p "$repo/scripts/dev" "$repo/tools/nextest"
cp "$here/../dev/test-parallel.sh" "$repo/scripts/dev/"
cp "$here/../../tools/nextest/VERSION" "$repo/tools/nextest/"
git init -q "$repo"
gitc() { git -C "$repo" -c user.email=t@example.invalid -c user.name=t "$@"; }
commit() { gitc add -A && gitc commit -q -m "$1" && git -C "$repo" rev-parse HEAD; }
c0="$(commit base)"

# `current` = c0 のリリース（gate 済みとみなす。gui/node_modules は持たない＝Phase SD-1 より前の形でもよい）。
c0_12="${c0:0:12}"
mkdir -p "$root/state/releases/$c0_12"
printf '{"sha":"%s","sha12":"%s","schema_version":7}\n' "$c0" "$c0_12" >"$root/state/releases/$c0_12/manifest.json"
printf '{"ok":true}\n' >"$root/state/releases/$c0_12/gate.json"
ln -s "releases/$c0_12" "$root/state/current"

run_release() {
  : >"$root/cargo.log"
  : >"$root/pnpm.log"
  CELERIS_STATE_DIR="$root/state" CELERIS_CONFIG_DIR="$root/config" CELERIS_CONFIG="$root/config/config.toml" \
    SD_REPO="$repo" SD_CELERISCTL="$root/bin/celerisctl" SD_PNPM_SHIM_DIR="$root/bin" \
    CARGO_LOG="$root/cargo.log" PNPM_LOG="$root/pnpm.log" FAKE_SCRATCH="$root/scratch" \
    PATH="$root/bin:$PATH" bash "$here/release.sh" "$1" >"$root/release.out" 2>&1
}

# `gate <sha12> <python expr over g>` — gate.json を読んで式が真か。
gate_is() {
  python3 - "$root/state/releases/$1/gate.json" "$2" <<'PY'
import json, sys
g = json.load(open(sys.argv[1], encoding="utf-8"))
steps = {s["step"]: s for s in g["steps"]}
sys.exit(0 if eval(sys.argv[2], {"g": g, "steps": steps}) else 1)
PY
}
GUI_SKIPPED='all(steps[n]["skipped"] and steps[n]["reason"] == "no change under gui/" and steps[n]["exit"] == 0 for n in ("pnpm-test", "pnpm-mobile-audit", "pnpm-e2e-mock"))'
GUI_RAN='not any(steps[n]["skipped"] for n in ("pnpm-test", "pnpm-mobile-audit", "pnpm-e2e-mock"))'
ALWAYS_RAN='not any(steps[n]["skipped"] for n in ("cargo-fmt-check", "cargo-test", "cargo-clippy", "cargo-build", "pnpm-install", "pnpm-typecheck", "pnpm-build"))'

# ---- 1. Rust だけ変えた → GUI の検査の段を飛ばす。作業ツリーは新規、workspace は掃除する ----
printf '// v1\n' >"$repo/crates/x/src/lib.rs"
c1="$(commit rust-only)"
c1_12="${c1:0:12}"
run_release "$c1" || fail "release of a rust-only change failed"
gate_is "$c1_12" "$GUI_SKIPPED" || fail "gui steps were not skipped for a rust-only change"
gate_is "$c1_12" "$ALWAYS_RAN" || fail "a rust/bundle step was skipped"
gate_is "$c1_12" "g['gui_skip_base'] == '$c0_12'" || fail "gui_skip_base is not current ($c0_12)"
gate_is "$c1_12" "g['build']['tree_reused'] is False and not steps['cargo-workspace-clean']['skipped']" \
  || fail "first release should create the build tree and clean the workspace members"
gate_is "$c1_12" "g['build']['scratch_owner'] == 'release-build'" || fail "scratch owner is not release-build"
gate_is "$c1_12" "{k: g['cargo_test'][k] for k in ('runner', 'binaries', 'passed', 'failed', 'ignored')} == {'runner': 'nextest', 'binaries': 2, 'passed': 5, 'failed': 0, 'ignored': 1}" \
  || fail "cargo_test summary is wrong"
if cut -f2 "$root/pnpm.log" | grep -qxE 'test|mobile-audit|e2e:mock'; then
  fail "a skipped gui step was run: $(cat "$root/pnpm.log")"
fi
grep -q $'\tnextest run --workspace' "$root/cargo.log" || fail "cargo nextest run did not run"
grep -q $'\ttest --doc --workspace' "$root/cargo.log" || fail "cargo test --doc did not run"
[ -L "$root/state/releases/$c1_12/gui/node_modules" ] || fail "gui/node_modules is not a symlink"
link1="$(readlink "$root/state/releases/$c1_12/gui/node_modules")"
case "$link1" in ../../.pnpm-prod-cache/*/node_modules) ;; *) fail "unexpected node_modules link: $link1" ;; esac
[ -f "$root/state/releases/$c1_12/gui/node_modules/.modules.yaml" ] || fail "node_modules symlink does not resolve"
grep -q $'\tinstall --prod --frozen-lockfile$' "$root/pnpm.log" || fail "cache miss did not run pnpm install --prod --frozen-lockfile"
python3 -c 'import json,sys; m=json.load(open(sys.argv[1])); d=m["gui_prod_deps"]; sys.exit(0 if d["reused"] is False and d["created_by_sha12"]==sys.argv[2] else 1)' \
  "$root/state/releases/$c1_12/manifest.json" "$c1_12" || fail "manifest gui_prod_deps (miss) is wrong"
[ -f "$root/scratch/targets/release-build/target/.celeris-release-tree" ] || fail "target tree mark was not written"

# ---- 2. gui/ を変えた → GUI の段も回す。作業ツリーと target は使い回し、依存の cache は当たる ----
printf 'x\n' >"$repo/gui/app.txt"
c2="$(commit gui-change)"
c2_12="${c2:0:12}"
run_release "$c2" || fail "release of a gui change failed"
gate_is "$c2_12" "$GUI_RAN" || fail "gui steps were skipped although gui/ changed"
gate_is "$c2_12" "$ALWAYS_RAN" || fail "a rust/bundle step was skipped"
gate_is "$c2_12" "g['gui_skip_base'] is None" || fail "gui_skip_base should be null when gui/ changed"
gate_is "$c2_12" "g['build']['tree_reused'] is True and steps['cargo-workspace-clean']['skipped']" \
  || fail "second release should reuse the build tree and skip cargo-workspace-clean"
cut -f2 "$root/pnpm.log" | grep -qx 'mobile-audit' || fail "pnpm mobile-audit did not run"
if grep -q -- '--prod' "$root/pnpm.log"; then fail "cache hit still ran pnpm install --prod"; fi
[ "$(readlink "$root/state/releases/$c2_12/gui/node_modules")" = "$link1" ] || fail "cache hit points at a different entry"
python3 -c 'import json,sys; m=json.load(open(sys.argv[1])); d=m["gui_prod_deps"]; sys.exit(0 if d["reused"] is True and d["created_by_sha12"]==sys.argv[2] else 1)' \
  "$root/state/releases/$c2_12/manifest.json" "$c1_12" || fail "manifest gui_prod_deps (hit) is wrong"
[ "$(git -C "$root/state/releases/.build/tree" rev-parse HEAD)" = "$c2" ] || fail "build tree is not at c2"

# ---- 3. SD_GATE_FORCE_GUI=1 → Rust だけの変更でも GUI の段を回す ----
printf '// v3\n' >"$repo/crates/x/src/lib.rs"
c3="$(commit rust-only-2)"
c3_12="${c3:0:12}"
# current から見ると gui/app.txt が増えているので、base を c2 にしておく（gui/ に差が無い状態で強制を試す）。
mkdir -p "$root/state/releases/$c2_12"
ln -sfn "releases/$c2_12" "$root/state/current"
SD_GATE_FORCE_GUI=1 run_release "$c3" || fail "forced release failed"
gate_is "$c3_12" "$GUI_RAN" || fail "SD_GATE_FORCE_GUI=1 did not run the gui steps"

# ---- 4. current が無い → 飛ばさない。lockfile が変わる → 新しい cache entry、古い entry は消える ----
rm -f "$root/state/current"
printf 'lockfileVersion: 9.0\n# changed\n' >"$repo/gui/pnpm-lock.yaml"
printf '// v4\n' >"$repo/crates/x/src/lib.rs"
c4="$(commit lock-change)"
c4_12="${c4:0:12}"
run_release "$c4" || fail "release without current failed"
gate_is "$c4_12" "$GUI_RAN" || fail "gui steps were skipped without a current release"
link4="$(readlink "$root/state/releases/$c4_12/gui/node_modules")"
[ "$link4" != "$link1" ] || fail "a lockfile change did not change the cache key"
# c1..c3 は prune された（current / previous / 検証済みが無い）ので、link1 の entry はどこからも指されていない。
old_key="$(basename "$(dirname "$link1")")"
[ ! -e "$root/state/releases/.pnpm-prod-cache/$old_key" ] || fail "unreferenced cache entry $old_key was not pruned"
[ "$(find "$root/state/releases/.pnpm-prod-cache" -mindepth 1 -maxdepth 1 | wc -l)" -eq 1 ] || fail "cache should hold exactly one entry"

echo "release_gui_skip_and_shared_tree: ok"
