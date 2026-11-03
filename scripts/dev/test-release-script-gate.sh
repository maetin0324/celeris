#!/usr/bin/env bash
# release.sh の TS 制御層の bundle の gate の決定的な試験（ADR 2026-11-03-release-script-bundle-gate）。
#   bash scripts/dev/test-release-script-gate.sh
# (A) 偽の木と偽の cargo・node・corepack で release.sh を本物の script-bundle-gate.sh ごと走らせ、
#     bundle が欠けた・作れない・hash が合わない・古い（source_commit・schema）・shim を書き換えた・install できない・
#     実 bundle 試験が SKIP した、のどれでも gate が落ち、gate.json の failed_step に段の名前が残り、release を作らないこと。
#     通る場合は release に script-bundle/ が入り、試験の段に CELERIS_SCRIPT_REQUIRE_BUNDLE=1・CELERIS_SCRIPT_BUNDLE_DIR が渡ること。
# (B) verify-release（verify.sh が呼ぶ）が gate の段の欠けた release・中身を差し替えた bundle を拒むこと。
# (C) 本物の node・pnpm（offline）で、写した control-ts から install・bundle・verify が通り、bundle の前後で
#     node_modules/.bin の shim が変わらないこと。node_modules を symlink した写しで `pnpm run bundle` しても元の shim が
#     変わらないこと（control-ts/pnpm-workspace.yaml の verifyDepsBeforeRun: error。S4 の副作用の回帰）。
# 外部ネットワークに出ない（pnpm は --offline）。CPU を焼く負荷はかけない。
set -euo pipefail

src="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
sd="$src/scripts/selfdeploy"
root="$(mktemp -d "${TMPDIR:-/tmp}/test-release-script-gate.XXXXXX")"
trap 'chmod -R u+w "$root" 2>/dev/null || true; rm -rf "$root"' EXIT
pass() { echo "ok: $*"; }
fail() {
  echo "FAIL: $*" >&2
  [ -f "$root/release.out" ] && tail -n 30 "$root/release.out" >&2
  exit 1
}

# ---- (A) 偽の道具 -------------------------------------------------------------
mkdir -p "$root/bin" "$root/config" "$root/state/releases"
: >"$root/config/config.toml"
cat >"$root/bin/cargo" <<'EOF'
#!/usr/bin/env bash
printf 'cargo %s require=%s dir=%s\n' "$*" "${CELERIS_SCRIPT_REQUIRE_BUNDLE:-}" "${CELERIS_SCRIPT_BUNDLE_DIR:-}" >>"$FAKE_CARGO_LOG"
case "$1" in
  metadata) printf '{"workspace_members":["x 0.1.0"],"packages":[{"id":"x 0.1.0","name":"x"}]}\n' ;;
  nextest)
    case "$2" in
      --version) echo 'cargo-nextest 0.9.146 (fake)' ;;
      run)
        if [ "${FAKE_SKIP:-}" = 1 ] && [ -n "${CELERIS_SCRIPT_SKIP_LOG:-}" ]; then
          printf 'script_fake\tbundle is not usable\n' >>"$CELERIS_SCRIPT_SKIP_LOG"
        fi
        printf '    Starting 3 tests across 1 binary\n'
        printf '     Summary [   0.010s] 3 tests run: 3 passed, 0 skipped\n' ;;
    esac ;;
  test) printf 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n' ;;
  run) echo '{"bundle_sha256":"fake"}' ;;
  build)
    mkdir -p "$CARGO_TARGET_DIR/release"
    for b in celeris celerisctl celeris-credentiald; do printf '#!/bin/sh\nexit 0\n' >"$CARGO_TARGET_DIR/release/$b"; chmod +x "$CARGO_TARGET_DIR/release/$b"; done ;;
esac
exit 0
EOF
cat >"$root/bin/corepack" <<'EOF'
#!/usr/bin/env bash
spec="$1"; shift
case "$spec" in pnpm@12.6.0) ;; *) echo "unexpected corepack spec: $spec" >&2; exit 2 ;; esac
case "$1" in
  install)
    case "$PWD" in */control-ts) [ "${FAKE_INSTALL_FAIL:-}" = 1 ] && { echo 'offline install failed (fake)'; exit 1; } ;; esac
    mkdir -p node_modules/.bin && : >node_modules/.modules.yaml
    for n in esbuild tsc vitest; do printf '#!/bin/sh\n# %s shim\n' "$n" >"node_modules/.bin/$n"; done ;;
  release)
    name="celeris-web-0.1.0-${CELERIS_WEB_RELEASE:?}"
    stage="$(mktemp -d)"
    mkdir -p "$stage/$name/server" release
    printf '// gateway\n' >"$stage/$name/server/index.js"
    printf '{"name":"celeris-web"}\n' >"$stage/$name/package.json"
    tar -czf "release/$name.tar.gz" -C "$stage" "$name"
    rm -rf "$stage" ;;
esac
exit 0
EOF
# 偽の node: build-bundle.mjs は FAKE_BUNDLE の型で dist を書く。--check・web の import 確認は通す。
cat >"$root/bin/node" <<'EOF'
#!/usr/bin/env bash
case "${1:-}" in
  --version) echo v24.0.0; exit 0 ;;
  --check) exit 0 ;;
  scripts/build-bundle.mjs) ;;
  *) exit 0 ;;
esac
mode="${FAKE_BUNDLE:-ok}"
case "$mode" in
  fail) echo 'bundle: esbuild failed (fake)' >&2; exit 1 ;;
  missing) exit 0 ;;
  shim) printf '#!/bin/sh\n# rewritten\n' >node_modules/.bin/tsc ;;
esac
mkdir -p dist
printf 'import { hostVersion } from "celeris:host";\nexport const v = hostVersion;\n' >dist/bundle.mjs
python3 -I - "$mode" "$(git rev-parse HEAD)" <<'PY'
import hashlib, json, sys
mode, commit = sys.argv[1:3]
sha = lambda p: hashlib.sha256(open(p, "rb").read()).hexdigest()
cfg = json.load(open("bundle.config.json"))
m = {"schema": "celeris.script-manifest/1", "bundle_sha256": sha("dist/bundle.mjs"), "host_api": cfg["host_api"],
     "schema_sha256": sha("../docs/api/script-host/host-v1.schema.json"),
     "toolchain": {"typescript": "7.0.2", "esbuild": "0.28.2", "node": "24.0.0"}, "source_commit": commit,
     "domains": sorted(cfg["domains"]), "capabilities": sorted({c for cs in cfg["domains"].values() for c in cs}),
     "checkpoint_versions": []}
if mode == "badhash": m["bundle_sha256"] = "0" * 64
if mode == "stale": m["source_commit"] = "1" * 40
if mode == "dirty": m["source_commit"] = commit + "-dirty"
if mode == "schema": m["schema_sha256"] = "2" * 64
if mode == "hostapi": m["host_api"] = "^2"
if mode == "domains": m["domains"] = ["host_selftest"]
json.dump(m, open("dist/manifest.json", "w"))
PY
EOF
cat >"$root/bin/celerisctl" <<'EOF'
#!/usr/bin/env bash
if [ "$1 $2" = "scratch lease" ]; then
  owner=""
  while [ $# -gt 0 ]; do [ "$1" = "--owner" ] && owner="$2"; shift; done
  printf '%s\n' "$FAKE_SCRATCH/targets/$owner/target"
fi
exit 0
EOF
chmod +x "$root/bin/"*

# ---- (A) 偽の木 ---------------------------------------------------------------
repo="$root/repo"
mkdir -p "$repo/crates/task-core/src" "$repo/crates/celeris" "$repo/crates/script-runtime/src" \
  "$repo/scripts/selfdeploy" "$repo/scripts/dev" "$repo/tools/nextest" "$repo/web/server" \
  "$repo/control-ts/scripts" "$repo/docs/api/script-host"
printf 'pub const SCHEMA_VERSION: u32 = 7;\n' >"$repo/crates/task-core/src/store.rs"
printf '[package]\nname = "celeris"\nversion = "0.1.0"\n' >"$repo/crates/celeris/Cargo.toml"
grep -E '^pub const HOST_API_VERSION' "$src/crates/script-runtime/src/dto.rs" >"$repo/crates/script-runtime/src/dto.rs"
sed -n '/^pub const KNOWN_DOMAINS/,/^];/p' "$src/crates/script-runtime/src/generation.rs" >"$repo/crates/script-runtime/src/generation.rs"
cp "$src/docs/api/script-host/host-v1.schema.json" "$repo/docs/api/script-host/"
cp "$src/control-ts/package.json" "$src/control-ts/pnpm-lock.yaml" "$src/control-ts/bundle.config.json" "$src/control-ts/.gitignore" "$repo/control-ts/"
printf '// fake\n' >"$repo/control-ts/scripts/build-bundle.mjs"
printf '{"name":"celeris-web","version":"0.1.0","packageManager":"pnpm@12.6.0"}\n' >"$repo/web/package.json"
printf 'lockfileVersion: 9.0\n' >"$repo/web/pnpm-lock.yaml"
printf '// gateway\n' >"$repo/web/server/index.js"
cp "$sd"/*.sh "$repo/scripts/selfdeploy/"
cp "$src/scripts/dev/test-parallel.sh" "$src/scripts/dev/source-size-report.py" "$repo/scripts/dev/"
cp "$src/tools/nextest/VERSION" "$repo/tools/nextest/"
git init -q "$repo"
git -C "$repo" -c user.email=t@example.invalid -c user.name=t add -A
git -C "$repo" -c user.email=t@example.invalid -c user.name=t commit -q -m base
c1="$(git -C "$repo" rev-parse HEAD)"
c1_12="${c1:0:12}"

run_release() {
  : >"$root/cargo.log"
  env -u SD_SCRIPT_BUNDLE_GATE -u CELERIS_SCRIPT_REQUIRE_BUNDLE -u CELERIS_SCRIPT_BUNDLE_DIR \
    CELERIS_STATE_DIR="$root/state" CELERIS_CONFIG_DIR="$root/config" CELERIS_CONFIG="$root/config/config.toml" \
    SD_REPO="$repo" SD_CELERISCTL="$root/bin/celerisctl" SD_PNPM_SHIM_DIR="$root/bin" SD_RELEASE_PRUNE=0 \
    FAKE_CARGO_LOG="$root/cargo.log" FAKE_SCRATCH="$root/scratch" \
    PATH="$root/bin:$PATH" bash "$sd/release.sh" "$c1" >"$root/release.out" 2>&1
}
gate_is() {
  python3 -I - "$1" "$2" <<'PY'
import json, sys
g = json.load(open(sys.argv[1], encoding="utf-8"))
steps = {s["step"]: s for s in g["steps"]}
sys.exit(0 if eval(sys.argv[2], {"g": g, "steps": steps}) else 1)
PY
}

# `expect_fail <env assignment> <failed_step>`: gate が落ち、failed_step と段のログが残り、release を作らない。
expect_fail() {
  local assign="$1" step="$2" g="$root/state/releases/.build/$c1_12/gate.json"
  if ( export "${assign?}"; run_release ); then
    fail "($assign) release succeeded"
  fi
  [ -f "$g" ] || fail "($assign) gate.json was not written"
  gate_is "$g" "g['ok'] is False and g['failed_step'] == '$step' and steps['$step']['exit'] != 0 and not steps['$step']['skipped']" \
    || fail "($assign) failed_step is not $step: $(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["failed_step"])' "$g")"
  [ -f "$root/state/releases/.build/$c1_12/.gate-$step.log" ] || fail "($assign) .gate-$step.log is missing"
  [ ! -d "$root/state/releases/$c1_12" ] || fail "($assign) a release directory was created"
  pass "($assign) gate fails at $step"
}

expect_fail FAKE_BUNDLE=missing script-bundle-verify
grep -q 'bundle missing' "$root/state/releases/.build/$c1_12/.gate-script-bundle-verify.log" || fail "missing: reason not logged"
expect_fail FAKE_BUNDLE=fail control-ts-bundle
expect_fail FAKE_BUNDLE=badhash script-bundle-verify
expect_fail FAKE_BUNDLE=stale script-bundle-verify
expect_fail FAKE_BUNDLE=dirty script-bundle-verify
expect_fail FAKE_BUNDLE=schema script-bundle-verify
expect_fail FAKE_BUNDLE=hostapi script-bundle-verify
expect_fail FAKE_BUNDLE=domains script-bundle-verify
expect_fail FAKE_BUNDLE=shim control-ts-bundle
grep -q 'changed node_modules/.bin shims' "$root/state/releases/.build/$c1_12/.gate-control-ts-bundle.log" || fail "shim: reason not logged"
expect_fail FAKE_INSTALL_FAIL=1 control-ts-install
expect_fail FAKE_SKIP=1 script-bundle-tests
grep -q 'does not accept SKIP' "$root/state/releases/.build/$c1_12/.gate-script-bundle-tests.log" || fail "skip: reason not logged"

# 通る場合: release に script-bundle/ が入り、gate.json の段と hash が揃い、試験に bundle が渡る。
run_release || fail "release with a valid bundle failed"
rel="$root/state/releases/$c1_12"
gate_is "$rel/gate.json" "g['ok'] is True and all(steps[n]['exit'] == 0 and not steps[n]['skipped'] for n in ('control-ts-install','control-ts-bundle','script-bundle-verify','cargo-test','script-bundle-tests')) and g['script_bundle']['dir'] == 'script-bundle'" \
  || fail "gate.json of the passing release is wrong"
order="$(python3 -I -c 'import json,sys; print(" ".join(s["step"] for s in json.load(open(sys.argv[1]))["steps"]))' "$rel/gate.json")"
case "$order" in *"script-bundle-verify cargo-test script-bundle-tests"*) ;; *) fail "unexpected step order: $order" ;; esac
[ -f "$rel/script-bundle/bundle.mjs" ] && [ -f "$rel/script-bundle/manifest.json" ] || fail "script-bundle/ is not in the release"
grep -q "^cargo nextest run --locked -p celeris -p celeris-script .* require=1 dir=$root/state/releases/.build/tree/control-ts/dist$" "$root/cargo.log" \
  || fail "script tests did not get CELERIS_SCRIPT_REQUIRE_BUNDLE=1 and CELERIS_SCRIPT_BUNDLE_DIR: $(grep nextest "$root/cargo.log")"
grep -q "^cargo run --locked -q -p celeris-script --bin script-load-check -- --bundle-dir $root/state/releases/.build/tree/control-ts/dist require=1" "$root/cargo.log" \
  || fail "script-load-check did not run on the bundle"
# test-parallel.sh（cargo-test 段）にも同じ env が渡る。
grep -q "^cargo nextest run --workspace.* require=1 dir=$root/state/releases/.build/tree/control-ts/dist$" "$root/cargo.log" \
  || fail "cargo-test did not get the bundle env: $(grep 'nextest run' "$root/cargo.log" | head -3)"
pass "valid bundle: release built, script-bundle/ included, REQUIRE=1 and BUNDLE_DIR passed to the tests"

# ---- (B) verify-release ------------------------------------------------------
bash "$sd/script-bundle-gate.sh" verify-release "$rel" >/dev/null 2>&1 || fail "verify-release rejected a good release"
cp -r "$rel" "$root/rel-tampered"
printf 'export const v = 2;\n' >"$root/rel-tampered/script-bundle/bundle.mjs"
if bash "$sd/script-bundle-gate.sh" verify-release "$root/rel-tampered" >/dev/null 2>&1; then fail "verify-release accepted a tampered bundle"; fi
cp -r "$rel" "$root/rel-old"
python3 -I - "$root/rel-old/gate.json" <<'PY'
import json, sys
g = json.load(open(sys.argv[1]))
g["steps"] = [s for s in g["steps"] if s["step"] != "script-bundle-tests"]
json.dump(g, open(sys.argv[1], "w"))
PY
if bash "$sd/script-bundle-gate.sh" verify-release "$root/rel-old" >/dev/null 2>&1; then fail "verify-release accepted a release without script-bundle-tests"; fi
grep -q 'script-bundle-gate.sh" verify-release "$REL"' "$sd/verify.sh" || fail "verify.sh does not call verify-release"
pass "verify-release rejects tampered bundles and releases without the script gate"

# ---- (C) 本物の node・pnpm（offline） -----------------------------------------
command -v node >/dev/null 2>&1 || fail "node not found (the real-tool part does not SKIP)"
command -v corepack >/dev/null 2>&1 || fail "corepack not found (the real-tool part does not SKIP)"
real="$root/real"
mkdir -p "$real/docs/api" "$real/crates/script-runtime/src"
cp -r "$src/docs/api/script-host" "$real/docs/api/"
cp "$src/crates/script-runtime/src/dto.rs" "$src/crates/script-runtime/src/generation.rs" "$real/crates/script-runtime/src/"
(cd "$src" && git ls-files -z control-ts) | (cd "$src" && xargs -0 cp --parents -t "$real")
git init -q "$real"
git -C "$real" -c user.email=t@example.invalid -c user.name=t add -A
git -C "$real" -c user.email=t@example.invalid -c user.name=t commit -q -m real
g="$sd/script-bundle-gate.sh"
bash "$g" install "$real/control-ts" >"$root/real-install.log" 2>&1 || { cat "$root/real-install.log" >&2; fail "real install failed"; }
shims_before="$(cd "$real/control-ts/node_modules/.bin" && sha256sum esbuild tsc vitest)"
bash "$g" bundle "$real/control-ts" >"$root/real-bundle.log" 2>&1 || { cat "$root/real-bundle.log" >&2; fail "real bundle failed"; }
bash "$g" verify "$real" "$real/control-ts/dist" "$(git -C "$real" rev-parse HEAD)" >"$root/real-verify.log" 2>&1 \
  || { cat "$root/real-verify.log" >&2; fail "real verify failed"; }
[ "$shims_before" = "$(cd "$real/control-ts/node_modules/.bin" && sha256sum esbuild tsc vitest)" ] || fail "real bundle changed the shims"
# 欠けた bundle は verify で落ちる（SKIP しない）。
mv "$real/control-ts/dist/bundle.mjs" "$root/bundle.mjs.bak"
if bash "$g" verify "$real" "$real/control-ts/dist" "$(git -C "$real" rev-parse HEAD)" >/dev/null 2>&1; then fail "verify passed without bundle.mjs"; fi
mv "$root/bundle.mjs.bak" "$real/control-ts/dist/bundle.mjs"
pass "real node/pnpm: install, bundle, verify pass; shims unchanged; missing bundle fails"

# S4 の副作用: node_modules を symlink した写しで `pnpm run bundle` しても、元の木の shim は書き換わらない。
copy="$root/copy"
mkdir -p "$copy/docs/api" "$copy/control-ts"
cp -r "$src/docs/api/script-host" "$copy/docs/api/"
(cd "$real/control-ts" && git ls-files -z) | (cd "$real/control-ts" && xargs -0 cp --parents -t "$copy/control-ts")
ln -s "$real/control-ts/node_modules" "$copy/control-ts/node_modules"
pm="$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["packageManager"])' "$real/control-ts/package.json")"
(cd "$copy/control-ts" && corepack "$pm" run bundle) >"$root/copy-run.log" 2>&1 || true
[ "$shims_before" = "$(cd "$real/control-ts/node_modules/.bin" && sha256sum esbuild tsc vitest)" ] \
  || { cat "$root/copy-run.log" >&2; fail "pnpm run bundle in a symlinked copy rewrote the original shims"; }
pass "pnpm run bundle in a symlinked copy leaves the original shims unchanged"

echo "test-release-script-gate: ok"
