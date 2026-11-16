#!/usr/bin/env bash
# All Rust/web stages and packaging are required; missing web cannot produce a release.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
# control-ts を持たない偽の木なので、TS 制御層の bundle の段は偽物にする（本物は scripts/dev/test-release-script-gate.sh）。
export SD_SCRIPT_BUNDLE_GATE="$here/tests/fixtures/fake-script-bundle-gate.sh"
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

cat >"$root/bin/cargo" <<'EOF2'
#!/usr/bin/env bash
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
  test) printf '   Doc-tests x\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n' ;;
  build)
    mkdir -p "$CARGO_TARGET_DIR/release"
    for b in celeris celerisctl celeris-credentiald; do printf '#!/bin/sh\nexit 0\n' >"$CARGO_TARGET_DIR/release/$b"; chmod +x "$CARGO_TARGET_DIR/release/$b"; done
    ;;
esac
exit 0
EOF2
# web の pnpm は `corepack pnpm@12.6.0 …` で呼ばれる。WEB_FAIL_STEP=test なら web-pnpm-test を落とす
# （e2e なら web-pnpm-e2e、e2e:chromium なら web-e2e-chromium）。
cat >"$root/bin/corepack" <<'EOF2'
#!/usr/bin/env bash
spec="$1"; shift
printf '%s\t%s\t%s\n' "$PWD" "$spec" "$*" >>"$COREPACK_LOG"
case "$spec" in pnpm@12.6.0) ;; *) echo "unexpected corepack spec: $spec" >&2; exit 2 ;; esac
dir="."
if [ "${1:-}" = -C ]; then dir="$2"; shift 2; fi
[ "${WEB_FAIL_STEP:-}" = "$1" ] && { echo "web $1 failed (fake)"; exit 1; }
case "$1" in
  install) mkdir -p "$dir/node_modules" && : >"$dir/node_modules/.modules.yaml" ;;
  release)
    name="celeris-web-0.1.0-${CELERIS_WEB_RELEASE:?}"
    stage="$(mktemp -d)"
    mkdir -p "$stage/$name/server" "$stage/$name/dist" release
    printf '// gateway\n' >"$stage/$name/server/index.js"
    printf '{"name":"celeris-web","release":"%s"}\n' "$CELERIS_WEB_RELEASE" >"$stage/$name/package.json"
    tar -czf "release/$name.tar.gz" -C "$stage" "$name"
    rm -rf "$stage"
    printf 'release/%s.tar.gz\n' "$name"
    ;;
esac
exit 0
EOF2
cat >"$root/bin/node" <<'EOF2'
#!/usr/bin/env bash
[ "${1:-}" = --version ] && echo v24.0.0
exit 0
EOF2
cat >"$root/bin/celerisctl" <<'EOF2'
#!/usr/bin/env bash
if [ "$1 $2" = "scratch lease" ]; then
  owner=""
  while [ $# -gt 0 ]; do [ "$1" = "--owner" ] && owner="$2"; shift; done
  printf '%s\n' "$FAKE_SCRATCH/targets/$owner/target"
fi
exit 0
EOF2
chmod +x "$root/bin/"*

# ---- 元のリポジトリ（web/ は c1 で足す） --------------------------------------------

repo="$root/repo"
mkdir -p "$repo/crates/task-core/src" "$repo/crates/celeris" "$repo/crates/x/src" "$repo/scripts/selfdeploy"
printf 'pub const SCHEMA_VERSION: u32 = 7;\n' >"$repo/crates/task-core/src/store.rs"
printf '[package]\nname = "celeris"\nversion = "0.1.0"\n' >"$repo/crates/celeris/Cargo.toml"
printf '// v0\n' >"$repo/crates/x/src/lib.rs"
cp "$here"/*.sh "$here"/*.py "$repo/scripts/selfdeploy/"
# release.sh は cos-skills.txt の CoS skill を梱包する（sd_bundle_cos_skills）。偽の木は skill を持たないので空の一覧を置く。
printf '# fixture: no CoS skills\n' >"$repo/scripts/selfdeploy/cos-skills.txt"
chmod +x "$repo/scripts/selfdeploy/"*.sh
mkdir -p "$repo/scripts/dev" "$repo/tools/nextest"
cp "$here/../dev/test-parallel.sh" "$repo/scripts/dev/"
cp "$here/../dev/source-size-report.py" "$repo/scripts/dev/"
# test-parallel.sh が読む helper と、release.sh が同梱する検査器（sd_bundle_repo_health は repo_health.py と
# source-size-report.py の無い build を失敗にする）。
mkdir -p "$repo/scripts/dev/lib" "$repo/scripts/dev/repo-health"
cp "$here/../dev/lib/test-tmp-lifecycle.sh" "$here/../dev/lib/check-slot.sh" "$repo/scripts/dev/lib/"
cp "$here/../dev/repo-health/repo_health.py" "$repo/scripts/dev/repo-health/"
cp "$here/../../tools/nextest/VERSION" "$repo/tools/nextest/"
git init -q "$repo"
gitc() { git -C "$repo" -c user.email=t@example.invalid -c user.name=t "$@"; }
commit() { gitc add -A && gitc commit -q -m "$1" && git -C "$repo" rev-parse HEAD; }
c0="$(commit base-without-web)"
c0_12="${c0:0:12}"

run_release() {
  : >"$root/pnpm.log"
  : >"$root/corepack.log"
  CELERIS_STATE_DIR="$root/state" CELERIS_CONFIG_DIR="$root/config" CELERIS_CONFIG="$root/config/config.toml" \
    SD_REPO="$repo" SD_CELERISCTL="$root/bin/celerisctl" SD_PNPM_SHIM_DIR="$root/bin" \
    PNPM_LOG="$root/pnpm.log" COREPACK_LOG="$root/corepack.log" FAKE_SCRATCH="$root/scratch" \
    PATH="$root/bin:$PATH" bash "$here/release.sh" "$1" >"$root/release.out" 2>&1
}

# `gate_is <gate.json> <python expr over g, steps>`
gate_is() {
  python3 - "$1" "$2" <<'PY'
import json, sys
g = json.load(open(sys.argv[1], encoding="utf-8"))
steps = {s["step"]: s for s in g["steps"]}
sys.exit(0 if eval(sys.argv[2], {"g": g, "steps": steps}) else 1)
PY
}
RUST_GATE_RAN='all(not steps[n]["skipped"] and steps[n]["exit"] == 0 for n in ("cargo-fmt-check", "cargo-test", "cargo-clippy", "cargo-build"))'
WEB_STEP_NAMES='("web-pnpm-install", "web-pnpm-typecheck", "web-pnpm-test", "web-pnpm-release", "web-e2e-chromium", "web-pnpm-e2e")'

# A source tree without web must fail and leave failure evidence.
if run_release "$c0"; then fail "release without web succeeded"; fi
g0="$root/state/releases/.build/$c0_12/gate.json"
gate_is "$g0" "g['ok'] is False and g['failed_step'] == 'web-config'" || fail "missing web not recorded"
[ ! -d "$root/state/releases/$c0_12" ] || fail "missing web published"

# ---- (1) web/ を足す → web の段が回り、配布物がリリースに入る ----
mkdir -p "$repo/web/server"
printf '{"name":"celeris-web","version":"0.1.0","packageManager":"pnpm@12.6.0"}\n' >"$repo/web/package.json"
printf 'lockfileVersion: 9.0\n' >"$repo/web/pnpm-lock.yaml"
printf '// gateway\n' >"$repo/web/server/index.js"
c1="$(commit add-web)"
c1_12="${c1:0:12}"
run_release "$c1" || fail "release with a passing web stage failed"
g1="$root/state/releases/$c1_12/gate.json"
gate_is "$g1" "g['ok'] is True and g['failed_step'] == '' and $RUST_GATE_RAN" || fail "Rust gate is not ok with web/"
gate_is "$g1" "all(not steps[n]['skipped'] and steps[n]['exit'] == 0 for n in $WEB_STEP_NAMES)" || fail "web steps did not all run/pass"
gate_is "$g1" "g['web']['ok'] is True and g['web']['failed_step'] == '' and g['web']['pnpm'] == 'pnpm@12.6.0' and g['web']['bundle']['tarball'] == 'web/celeris-web-0.1.0-$c1_12.tar.gz' and g['web']['bundle']['app'] == 'web/app'" \
  || fail "gate.web (pass) is wrong"
# web の pnpm は corepack pnpm@12.6.0 で、unversioned pnpm は web/ では呼ばれない。
grep -q $'\tpnpm@12.6.0\tinstall --frozen-lockfile$' "$root/corepack.log" || fail "web install did not use corepack pnpm@12.6.0"
grep -q $'\tpnpm@12.6.0\trelease$' "$root/corepack.log" || fail "web release did not run"
# e2e（functional）は配る dist/ を作る release の後に、chromium の確認の後で流れる（ADR 2026-11-16-release-gate-web-e2e）。
gate_is "$g1" "[s['step'] for s in g['steps'] if s['step'].startswith('web-')][-3:] == ['web-pnpm-release', 'web-e2e-chromium', 'web-pnpm-e2e'] and g['web']['steps'][-2:] == ['web-e2e-chromium', 'web-pnpm-e2e']" \
  || fail "web e2e steps are not after web-pnpm-release in gate.json"
grep -q $'\tpnpm@12.6.0\te2e:chromium$' "$root/corepack.log" || fail "web e2e chromium check did not run"
grep -q $'\tpnpm@12.6.0\te2e$' "$root/corepack.log" || fail "web e2e did not run"
grep -q $'\tpnpm@12.6.0\tinstall --prod --offline --frozen-lockfile$' "$root/corepack.log" || fail "web/app was not prod-installed offline"
if grep -q "/web	" "$root/pnpm.log"; then fail "unversioned pnpm was called under web/: $(grep '/web	' "$root/pnpm.log")"; fi
rel1="$root/state/releases/$c1_12"
[ -f "$rel1/web/celeris-web-0.1.0-$c1_12.tar.gz" ] || fail "web tarball is not in the release"
[ -f "$rel1/web/app/server/index.js" ] || fail "web/app is not extracted"
[ -f "$rel1/web/app/node_modules/.modules.yaml" ] || fail "web/app has no prod node_modules"
python3 -c 'import json,sys; m=json.load(open(sys.argv[1])); sys.exit(0 if m["gate_ok"] is True and m["web"]["ok"] is True and m["web"]["bundle"]["app"]=="web/app" else 1)' \
  "$rel1/manifest.json" || fail "manifest (pass) is wrong"
[ -x "$rel1/bin/celeris" ] || fail "bin/celeris missing"
ln -sfn "releases/$c1_12" "$root/state/current"

# A failed web test prevents publication and keeps the failing step/log.
printf '// v2\n' >"$repo/web/server/index.js"
c2="$(commit web-change)"; c2_12="${c2:0:12}"
if WEB_FAIL_STEP=test run_release "$c2"; then fail "failed web test published"; fi
[ ! -d "$root/state/releases/$c2_12" ] || fail "failed web published"
g2="$root/state/releases/.build/$c2_12/gate.json"
gate_is "$g2" "g['ok'] is False and g['failed_step'] == 'web-pnpm-test' and g['web']['blocking'] is True" || fail "web failure not blocking"
[ -f "$root/state/releases/.build/$c2_12/.gate-web-pnpm-test.log" ] || fail "failure log missing"
# A failed web e2e prevents publication: gate.json ok=false and failed_step=web-pnpm-e2e, with the log kept.
printf '// v2e\n' >"$repo/web/server/index.js"
c2e="$(commit web-e2e-change)"; c2e_12="${c2e:0:12}"
if WEB_FAIL_STEP=e2e run_release "$c2e"; then fail "failed web e2e published"; fi
[ ! -d "$root/state/releases/$c2e_12" ] || fail "failed web e2e published a release dir"
g2e="$root/state/releases/.build/$c2e_12/gate.json"
gate_is "$g2e" "g['ok'] is False and g['failed_step'] == 'web-pnpm-e2e' and g['web']['ok'] is False and g['web']['failed_step'] == 'web-pnpm-e2e' and steps['web-pnpm-e2e']['exit'] != 0" \
  || fail "web e2e failure not recorded as failed_step web-pnpm-e2e"
[ -f "$root/state/releases/.build/$c2e_12/.gate-web-pnpm-e2e.log" ] || fail "web e2e failure log missing"
# A host without the e2e chromium fails at web-e2e-chromium (not skipped) and e2e does not run.
printf '// v2c\n' >"$repo/web/server/index.js"
c2c="$(commit web-chromium-change)"; c2c_12="${c2c:0:12}"
if WEB_FAIL_STEP=e2e:chromium run_release "$c2c"; then fail "release without e2e chromium published"; fi
[ ! -d "$root/state/releases/$c2c_12" ] || fail "release without e2e chromium published a release dir"
g2c="$root/state/releases/.build/$c2c_12/gate.json"
gate_is "$g2c" "g['ok'] is False and g['failed_step'] == 'web-e2e-chromium' and g['web']['failed_step'] == 'web-e2e-chromium' and 'web-pnpm-e2e' not in steps" \
  || fail "missing e2e chromium not recorded as failed_step web-e2e-chromium"
if grep -q $'\tpnpm@12.6.0\te2e$' "$root/corepack.log"; then fail "web e2e ran although the chromium check failed"; fi
# Reuse of the build checkout must not skip web checks on an unchanged web tree.
printf '// v3\n' >"$repo/crates/x/src/lib.rs"
c3="$(commit rust-only)"; c3_12="${c3:0:12}"
run_release "$c3" || fail "Rust-only release failed"
gate_is "$root/state/releases/$c3_12/gate.json" "g['build']['tree_reused'] is True and all(not steps[n]['skipped'] for n in $WEB_STEP_NAMES)" || fail "web gates skipped or build tree not reused"
echo "release_web_required: ok"
