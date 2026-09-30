#!/usr/bin/env bash
# Phase SD-2（ADR-0041 §8）: release.sh の `cargo-test` 段は `scripts/dev/test-parallel.sh`
# （`cargo nextest run --workspace` + `cargo test --doc --workspace`）を回し、
#   (1) 通れば gate.json の `cargo_test` に runner / バイナリ数（nextest + doc-test）/ 合格・失敗・無視 / 並列数が入る
#   (2) nextest のテストが 1 つでも落ちたら gate が `cargo-test` で落ち、リリースを作らず、`.build/<sha12>/gate.json`
#       の `cargo_test.failed` に数が残る（後の段は回らない）
#   (3) doc-test が落ちても同じく落ちる
#   (4) nextest が exit 0 でも Starting / Summary の行が無い（＝全部走った証拠が無い）なら落ちる
#   (5) cargo-nextest が無ければ、作業ツリーを作る前に入れ方を示して落ちる
#   (6) `SD_GATE_TEST_RUNNER=cargo-test` なら従来の `cargo test --workspace` を回し、runner = cargo-test
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

# 偽の cargo。nextest は本物の出力の形（Starting / Summary 行）を真似る。
#   FAKE_NEXTEST_FAIL=1 → 1 つ落ちる（exit 100）、FAKE_DOC_FAIL=1 → doc-test が落ちる（exit 101）、
#   FAKE_NEXTEST_NOSUMMARY=1 → exit 0 だが集計の行を出さない、FAKE_NO_NEXTEST=1 → `cargo nextest` が無い
cat >"$root/bin/cargo" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$CARGO_LOG"
case "$1" in
  metadata) printf '{"workspace_members":["x 0.1.0"],"packages":[{"id":"x 0.1.0","name":"x"}]}\n' ;;
  nextest)
    [ "${FAKE_NO_NEXTEST:-0}" = 1 ] && { echo "error: no such command: \`nextest\`" >&2; exit 101; }
    case "$2" in
      --version) echo 'cargo-nextest 0.9.146 (fake)' ;;
      run)
        [ "${FAKE_NEXTEST_NOSUMMARY:-0}" = 1 ] && exit 0
        printf '    Starting 7 tests across 2 binaries (2 tests skipped)\n'
        printf '        PASS [   0.004s] x tests::a\n'
        if [ "${FAKE_NEXTEST_FAIL:-0}" = 1 ]; then
          printf '        FAIL [   0.004s] x tests::b\n'
          printf '     Summary [   0.020s] 7 tests run: 6 passed, 1 failed, 2 skipped\n'
          printf 'error: test run failed\n' >&2
          exit 100
        fi
        printf '     Summary [   0.020s] 7 tests run: 7 passed, 2 skipped\n'
        ;;
    esac
    ;;
  test)
    if [ "${2:-}" = --doc ]; then
      printf '   Doc-tests x\n'
      if [ "${FAKE_DOC_FAIL:-0}" = 1 ]; then
        printf 'test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n'
        exit 101
      fi
      printf 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n'
    else
      printf '     Running unittests src/lib.rs (target/debug/deps/x-1)\n'
      printf 'test result: ok. 4 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.01s\n'
      printf '   Doc-tests x\n'
      printf 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n'
    fi
    ;;
  build)
    mkdir -p "$CARGO_TARGET_DIR/release"
    for b in celeris celerisctl celeris-credentiald; do printf '#!/bin/sh\nexit 0\n' >"$CARGO_TARGET_DIR/release/$b"; chmod +x "$CARGO_TARGET_DIR/release/$b"; done
    ;;
esac
exit 0
EOF
cat >"$root/bin/pnpm" <<'EOF'
#!/usr/bin/env bash
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

repo="$root/repo"
mkdir -p "$repo/gui" "$repo/crates/task-core/src/store" "$repo/crates/celeris" "$repo/crates/x/src" "$repo/scripts/selfdeploy"
cp "$here"/*.sh "$repo/scripts/selfdeploy/"
chmod +x "$repo/scripts/selfdeploy/"*.sh
# runner はビルドする sha の中のものを使う（release.sh の隣ではない）。
mkdir -p "$repo/scripts/dev" "$repo/tools/nextest"
cp "$here/../dev/test-parallel.sh" "$repo/scripts/dev/"
cp "$here/../dev/source-size-report.py" "$repo/scripts/dev/"
cp "$here/../../tools/nextest/VERSION" "$repo/tools/nextest/"
printf '{"name":"celeris-gui","version":"0.1.0","packageManager":"pnpm@11.27.0"}\n' >"$repo/gui/package.json"
printf 'lockfileVersion: 9.0\n' >"$repo/gui/pnpm-lock.yaml"
printf 'packages:\n  - "."\n' >"$repo/gui/pnpm-workspace.yaml"
printf '// server\n' >"$repo/gui/server.js"
printf 'pub const SCHEMA_VERSION: u32 = 7;\n' >"$repo/crates/task-core/src/store/migrations.rs"
printf '[package]\nname = "celeris"\nversion = "0.1.0"\n' >"$repo/crates/celeris/Cargo.toml"
git init -q "$repo"
# `$(new_commit)` はサブシェルで回るので、版の番号はファイルに持つ。
echo 0 >"$root/n"
new_commit() {
  local n
  n=$(($(cat "$root/n") + 1))
  echo "$n" >"$root/n"
  printf '// v%s\n' "$n" >"$repo/crates/x/src/lib.rs"
  git -C "$repo" add -A
  git -C "$repo" -c user.email=t@example.invalid -c user.name=t commit -q -m "c$n"
  git -C "$repo" rev-parse HEAD
}

run_release() {
  : >"$root/cargo.log"
  CELERIS_STATE_DIR="$root/state" CELERIS_CONFIG_DIR="$root/config" CELERIS_CONFIG="$root/config/config.toml" \
    SD_REPO="$repo" SD_CELERISCTL="$root/bin/celerisctl" SD_PNPM_SHIM_DIR="$root/bin" \
    CARGO_LOG="$root/cargo.log" FAKE_SCRATCH="$root/scratch" SD_RELEASE_PRUNE=0 \
    PATH="$root/bin:$PATH" bash "$here/release.sh" "$1" >"$root/release.out" 2>&1
}

# `gate_is <gate.json> <python expr over g>`
gate_is() {
  python3 - "$1" "$2" <<'PY'
import json, sys
g = json.load(open(sys.argv[1], encoding="utf-8"))
steps = {s["step"]: s for s in g["steps"]}
sys.exit(0 if eval(sys.argv[2], {"g": g, "steps": steps}) else 1)
PY
}

# ---- 1. 通る: counts と並列数 ----
c="$(new_commit)"
c12="${c:0:12}"
CELERIS_TEST_JOBS=3 run_release "$c" || fail "release with passing tests failed"
g="$root/state/releases/$c12/gate.json"
gate_is "$g" "g['ok'] and steps['cargo-test']['exit'] == 0" || fail "cargo-test step did not pass"
gate_is "$g" "{k: g['cargo_test'][k] for k in ('runner', 'binaries', 'nextest_binaries', 'doc_binaries', 'passed', 'failed', 'ignored', 'jobs')} == {'runner': 'nextest', 'binaries': 3, 'nextest_binaries': 2, 'doc_binaries': 1, 'passed': 8, 'failed': 0, 'ignored': 2, 'jobs': 3}" \
  || fail "cargo_test summary is wrong: $(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["cargo_test"])' "$g")"
grep -q -- '^nextest run --workspace --no-fail-fast .*--test-threads 3' "$root/cargo.log" || fail "nextest did not get --test-threads 3: $(cat "$root/cargo.log")"
grep -qx 'test --doc --workspace --color never' "$root/cargo.log" || fail "doc-tests did not run"
if grep -qx 'test --workspace' "$root/cargo.log"; then fail "plain cargo test --workspace ran with the nextest runner"; fi

# ---- 2. nextest のテストが落ちる → gate が cargo-test で落ち、リリースは無く、失敗数が残る ----
c="$(new_commit)"
c12="${c:0:12}"
if FAKE_NEXTEST_FAIL=1 run_release "$c"; then fail "release succeeded although a test failed"; fi
[ ! -e "$root/state/releases/$c12" ] || fail "a release was created although a test failed"
g="$root/state/releases/.build/$c12/gate.json"
[ -f "$g" ] || fail "no gate.json for the failed gate"
gate_is "$g" "not g['ok'] and g['failed_step'] == 'cargo-test' and steps['cargo-test']['exit'] != 0" || fail "gate did not fail at cargo-test"
gate_is "$g" "g['cargo_test']['failed'] == 1 and g['cargo_test']['passed'] == 7 and g['cargo_test']['nextest_exit'] == 100" \
  || fail "failed count not recorded: $(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["cargo_test"])' "$g")"
gate_is "$g" "'cargo-clippy' not in steps" || fail "a step after cargo-test ran"
# doc-test は nextest が落ちても回す（全部の失敗を 1 回で出す）。
grep -qx 'test --doc --workspace --color never' "$root/cargo.log" || fail "doc-tests were not run after a nextest failure"

# ---- 3. doc-test が落ちる → 落ちる ----
c="$(new_commit)"
c12="${c:0:12}"
if FAKE_DOC_FAIL=1 run_release "$c"; then fail "release succeeded although a doc-test failed"; fi
g="$root/state/releases/.build/$c12/gate.json"
gate_is "$g" "not g['ok'] and g['failed_step'] == 'cargo-test' and g['cargo_test']['failed'] == 1" || fail "doc-test failure did not fail the gate"

# ---- 4. nextest が exit 0 でも集計の行が無い → 落ちる ----
c="$(new_commit)"
c12="${c:0:12}"
if FAKE_NEXTEST_NOSUMMARY=1 run_release "$c"; then fail "release succeeded without proof that every binary ran"; fi
g="$root/state/releases/.build/$c12/gate.json"
gate_is "$g" "not g['ok'] and g['failed_step'] == 'cargo-test' and g['cargo_test']['summary_parsed'] is False" \
  || fail "missing nextest summary did not fail the gate"

# ---- 5. cargo-nextest が無い → 作業ツリーを作る前に入れ方を示して落ちる ----
c="$(new_commit)"
rm -rf "$root/state/releases/.build/tree"
if FAKE_NO_NEXTEST=1 run_release "$c"; then fail "release succeeded without cargo-nextest"; fi
grep -q 'cargo install cargo-nextest --locked --version 0.9.146' "$root/release.out" || fail "missing install instructions"
[ ! -e "$root/state/releases/.build/tree" ] || fail "build tree was created before the nextest check"

# ---- 6. SD_GATE_TEST_RUNNER=cargo-test → 従来の cargo test --workspace ----
FAKE_NO_NEXTEST=1 SD_GATE_TEST_RUNNER=cargo-test run_release "$c" || fail "release with the cargo-test runner failed"
c12="${c:0:12}"
g="$root/state/releases/$c12/gate.json"
gate_is "$g" "g['cargo_test'] == {'runner': 'cargo-test', 'binaries': 2, 'passed': 5, 'failed': 0, 'ignored': 1}" \
  || fail "legacy cargo_test summary is wrong"
grep -qx 'test --workspace' "$root/cargo.log" || fail "cargo test --workspace did not run"

# ---- 7. リリースに同梱した release.sh（scripts/ に平らに置かれ、dev/ が無い）からでも runner が見つかる ----
# 配送の prepare.sh は `current/scripts/release.sh` を起こす。以前は `current/dev/test-parallel.sh` を探して落ちていた。
bundled="$root/state/releases/$c12/scripts"
[ -f "$bundled/release.sh" ] && [ ! -e "$bundled/../dev" ] || fail "unexpected bundled layout under $bundled"
c="$(new_commit)"
c12="${c:0:12}"
: >"$root/cargo.log"
CELERIS_STATE_DIR="$root/state" CELERIS_CONFIG_DIR="$root/config" CELERIS_CONFIG="$root/config/config.toml" \
  SD_REPO="$repo" SD_CELERISCTL="$root/bin/celerisctl" SD_PNPM_SHIM_DIR="$root/bin" \
  CARGO_LOG="$root/cargo.log" FAKE_SCRATCH="$root/scratch" SD_RELEASE_PRUNE=0 \
  PATH="$root/bin:$PATH" bash "$bundled/release.sh" "$c" >"$root/release.out" 2>&1 \
  || fail "release from the bundled release.sh failed"
gate_is "$root/state/releases/$c12/gate.json" "g['ok'] and g['cargo_test']['runner'] == 'nextest'" \
  || fail "bundled release.sh did not run the nextest runner"

echo "release_parallel_test_gate: ok"
