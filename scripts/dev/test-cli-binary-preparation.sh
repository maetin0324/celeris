#!/usr/bin/env bash
# scripts/dev/test-cli-binary-preparation.sh — scripts/dev/test-parallel.sh の準備段（試験 binary と実 CLI の build と
# nextest の binary mapping の確認）の回帰検査。引数なしで `bash scripts/dev/test-cli-binary-preparation.sh`。
#
# cargo は偽物（PATH の先頭）に置き換え、呼ばれた順を記録する。cargo も実 CLI も走らせない（決定的・数秒）。
# 偽の CLI は「準備が nextest run より前に終わっているか」を見るための印で、CLI の試験の代わりではない
# （実 CLI の起動は `cargo nextest run -p celerisctl` と test-parallel.sh 本体で確かめる）。
#
# 確かめること:
#   (a) 準備が成功すると `nextest list` が `nextest run` より前に呼ばれ、run の時点で mapping の CLI が実行可能、exit 0
#   (b) 準備の build 失敗（list の exit 101）は nextest run を走らせず exit 101 で伝わる。summary の prepare_exit も 101
#   (c) mapping の CLI が無い・実行できない・mapping が読めないなら nextest run を走らせず exit 4
#   (d) 試験 binary が target の外の CARGO_BIN_EXE_*（seed を作った dir）で build されていれば診断行を出す（exit は 0 のまま）
#   (e) 準備の後の nextest 失敗は従来どおり exit 100、追加引数は nextest run だけに渡る
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
work="$(mktemp -d "${TMPDIR:-/tmp}/test-cli-binary-preparation.XXXXXX")"
trap 'chmod -R u+w "$work" 2>/dev/null || true; rm -rf "$work"' EXIT
mkdir -p "$work/bin"

# 偽の cargo。FAKE_LIST: ok（CLI を作って mapping を出す）/ build-fail / missing / noexec / garbage / stale
cat >"$work/bin/cargo" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$FAKE_CALLS"
tgt="$FAKE_TARGET"
case "$1 $2" in
  "nextest --version") echo "cargo-nextest 0.9.146" ;;
  "nextest list")
    case "$FAKE_LIST" in
      build-fail) echo "error: could not compile \`celerisctl\`" >&2; exit 101 ;;
      garbage) echo "not json"; exit 0 ;;
    esac
    mkdir -p "$tgt/debug/deps"
    if [ "$FAKE_LIST" != missing ]; then
      printf '#!/bin/sh\nexit 0\n' >"$tgt/debug/celerisctl"
      chmod +x "$tgt/debug/celerisctl"
      [ "$FAKE_LIST" = noexec ] && chmod -x "$tgt/debug/celerisctl"
    fi
    embedded="$tgt/debug/celerisctl"
    [ "$FAKE_LIST" = stale ] && embedded="/seeds/.building-gen-x/target/debug/celerisctl"
    printf '# env-dep:CARGO_BIN_EXE_celerisctl=%s\n' "$embedded" >"$tgt/debug/deps/bin_smoke-1.d"
    printf '{"rust-build-meta":{"target-directory":"%s","non-test-binaries":{"celerisctl 0.1.0":[{"name":"celerisctl","kind":"bin-exe","path":"debug/celerisctl"}]}},"rust-binaries":{"celerisctl::bin_smoke":{"binary-path":"%s/debug/deps/bin_smoke-1"}}}\n' "$tgt" "$tgt"
    ;;
  "nextest run")
    if [ -x "$tgt/debug/celerisctl" ]; then echo "run saw-cli=yes" >>"$FAKE_CALLS"; else echo "run saw-cli=no" >>"$FAKE_CALLS"; fi
    printf '    Starting 1 test across 1 binary\n'
    if [ "${FAKE_RUN_FAIL:-0}" = 1 ]; then
      printf '        FAIL [   0.004s] celerisctl::bin_smoke help_exits_zero\n'
      printf '     Summary [   0.010s] 1 test run: 0 passed, 1 failed, 0 skipped\n'
      exit 100
    fi
    printf '        PASS [   0.004s] celerisctl::bin_smoke help_exits_zero\n'
    printf '     Summary [   0.010s] 1 test run: 1 passed, 0 skipped\n'
    ;;
  "test --doc") printf '   Doc-tests x\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n' ;;
  *) echo "fake cargo: unexpected args: $*" >&2; exit 99 ;;
esac
EOF
chmod +x "$work/bin/cargo"

fail() {
  echo "FAIL [$case_name]: $*" >&2
  echo "--- calls ---" >&2; cat "$calls" >&2 || true
  echo "--- stderr ---" >&2; cat "$err" >&2 || true
  exit 1
}

# run_case <name> <FAKE_LIST> [extra args...]: rc / calls / out / err を残す
run_case() {
  case_name="$1"; local list="$2"; shift 2
  local dir="$work/$case_name"
  mkdir -p "$dir/target" "$dir/tmp"
  calls="$dir/calls"; out="$dir/stdout"; err="$dir/stderr"
  : >"$calls"
  rc=0
  PATH="$work/bin:$PATH" TMPDIR="$dir/tmp" FAKE_CALLS="$calls" FAKE_TARGET="$dir/target" FAKE_LIST="$list" \
    CELERIS_USERNS_TESTS=0 CELERIS_ISOLATION_TESTS= \
    bash "$here/scripts/dev/test-parallel.sh" "$@" >"$out" 2>"$err" || rc=$?
}

summary_field() {
  python3 -c 'import json,sys
for l in open(sys.argv[1]):
    if l.startswith("CELERIS_TEST_SUMMARY "):
        print(json.loads(l.split(" ",1)[1])[sys.argv[2]])' "$out" "$1"
}

line_of() { grep -n -- "$1" "$calls" | head -n 1 | cut -d: -f1; }

assert_not_run() {
  if grep -q '^nextest run' "$calls"; then fail "nextest run was started after a failed preparation"; fi
  grep -qx 'test --doc --workspace --color never' "$calls" || fail "doc-tests were not run"
  [ "$(summary_field prepare_exit)" = "$1" ] || fail "summary prepare_exit is not $1"
  [ "$(tail -n 1 "$err")" = "test-parallel: prepare failed (exit $1)" ] || fail "last stderr line is [$(tail -n 1 "$err")]"
}

# (a) 準備 → run の順。run の時点で CLI が実行可能
run_case ok ok
[ "$rc" -eq 0 ] || fail "exit $rc (expected 0)"
l_list="$(line_of '^nextest list --workspace --list-type binaries-only --message-format json')"
l_run="$(line_of '^nextest run --workspace')"
[ -n "$l_list" ] && [ -n "$l_run" ] && [ "$l_list" -lt "$l_run" ] || fail "nextest list did not come before nextest run"
grep -qx 'run saw-cli=yes' "$calls" || fail "the CLI was not executable when nextest run started"
grep -q '^test-parallel: prepare: ok (1 test binaries; CLI celerisctl in ' "$err" || fail "no prepare ok line"
if grep -q 'prepare: warning' "$err"; then fail "unexpected stale warning"; fi
[ "$(summary_field prepare_exit)" = 0 ] || fail "summary prepare_exit is not 0"
[ "$(summary_field passed)" = 2 ] || fail "summary passed is not 2"
[ "$(tail -n 1 "$err")" = "test-parallel: ok" ] || fail "last stderr line is [$(tail -n 1 "$err")]"

# (b) 準備の build 失敗は exit 101 で伝わる
run_case build-fail build-fail
[ "$rc" -eq 101 ] || fail "exit $rc (expected 101)"
assert_not_run 101

# (c) mapping の CLI が無い・実行できない・mapping が読めない
run_case missing missing
[ "$rc" -eq 4 ] || fail "exit $rc (expected 4)"
grep -q '^test-parallel: prepare: CLI binary missing or not executable: celerisctl (' "$err" || fail "missing CLI not named"
assert_not_run 4
run_case noexec noexec
[ "$rc" -eq 4 ] || fail "exit $rc (expected 4)"
grep -q '^test-parallel: prepare: CLI binary missing or not executable: celerisctl (' "$err" || fail "non-executable CLI not named"
assert_not_run 4
run_case garbage garbage
[ "$rc" -eq 4 ] || fail "exit $rc (expected 4)"
grep -q '^test-parallel: prepare: cannot read the nextest binary mapping' "$err" || fail "unreadable mapping not reported"
assert_not_run 4

# (d) target の外の CARGO_BIN_EXE_* は診断行（exit は変えない）
run_case stale stale
[ "$rc" -eq 0 ] || fail "exit $rc (expected 0)"
grep -q '^test-parallel: prepare: warning: 1 test binaries were built with CARGO_BIN_EXE_\* outside this target (e.g. celerisctl::bin_smoke: CARGO_BIN_EXE_celerisctl=/seeds/.building-gen-x/target/debug/celerisctl)' "$err" \
  || fail "stale embedded path not reported"

# (e) 準備の後の nextest 失敗は exit 100。追加引数は run だけ
FAKE_RUN_FAIL=1 run_case run-fail ok --no-capture
[ "$rc" -eq 100 ] || fail "exit $rc (expected 100)"
grep -q '^nextest run --workspace .* --no-capture$' "$calls" || fail "extra args did not reach nextest run"
if grep -q '^nextest list .*--no-capture' "$calls"; then fail "extra args were passed to nextest list"; fi
grep -qx 'test-parallel: failed: celerisctl::bin_smoke help_exits_zero' "$err" || fail "failed test name missing"
[ "$(tail -n 1 "$err")" = "test-parallel: cargo nextest run failed (exit 100)" ] || fail "last stderr line is [$(tail -n 1 "$err")]"
[ "$(summary_field prepare_exit)" = 0 ] || fail "summary prepare_exit is not 0"

echo "test-cli-binary-preparation: all checks passed"
