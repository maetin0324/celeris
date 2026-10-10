#!/bin/sh
# scripts/dev/test-parallel.sh の userns preflight（ADR-0126 B4）を確かめる。cargo と unshare は偽物（PATH の先頭）。
# 確かめること:
#   (a) CELERIS_ISOLATION_TESTS=require で unshare -Ur が失敗する host: 診断行が先頭（nextest の前）と失敗時の末尾
#       （最終行 `cargo nextest run failed (exit N)` の直前）に出る。exit code は変わらない（nextest の 100 のまま）。
#       CELERIS_TEST_SUMMARY の userns は false。
#   (b) 同じ env で unshare -Ur が成功する host: 診断行は出ない。userns は true。
#   (c) env が無いとき: unshare を呼ばない（偽 unshare は呼ばれたら印を残す）。userns は null。診断行は出ない。
set -eu

here="$(cd "$(dirname "$0")/../../.." && pwd)"
work="$(mktemp -d "${TMPDIR:-/tmp}/test-parallel-userns.XXXXXX")"
trap 'chmod -R u+w "$work" 2>/dev/null || true; rm -rf "$work"' EXIT
fakebin="$work/bin"
mkdir -p "$fakebin"

cat >"$work/nextest-fail.log" <<'EOF2'
    Starting 2 tests across 1 binary
        PASS [   0.010s] task-core tests::ok
        FAIL [   0.104s] task-worker isolation::bwrap_operation_not_permitted
     Summary [   0.200s] 2 tests run: 1 passed, 1 failed, 0 skipped
EOF2
cat >"$work/nextest-ok.log" <<'EOF2'
    Starting 2 tests across 1 binary
        PASS [   0.010s] task-core tests::ok
        PASS [   0.010s] task-worker isolation::real_bwrap
     Summary [   0.200s] 2 tests run: 2 passed, 0 failed, 0 skipped
EOF2

# 偽の cargo: nextest run は $FAKE_NEXTEST_LOG を出し $FAKE_NEXTEST_RC で終わる。doc-test は成功。
cat >"$fakebin/cargo" <<EOF2
#!/bin/sh
case "\$1 \$2" in
  "nextest --version") echo "cargo-nextest 0.9.146" ;;
  "nextest run") cat "\$FAKE_NEXTEST_LOG"; exit "\$FAKE_NEXTEST_RC" ;;
  "test --doc") exit 0 ;;
  *) echo "fake cargo: unexpected args: \$*" >&2; exit 99 ;;
esac
EOF2
# 偽の unshare: 呼ばれた印を残し、$FAKE_UNSHARE_RC で終わる。
cat >"$fakebin/unshare" <<EOF2
#!/bin/sh
echo "\$*" >>"$work/unshare.calls"
exit "\${FAKE_UNSHARE_RC:-0}"
EOF2
chmod +x "$fakebin/cargo" "$fakebin/unshare"

fail() {
  echo "FAIL: $*" >&2
  echo "--- stderr ---" >&2
  cat "$work/stderr" >&2
  exit 1
}

run() {
  # run <env...> ; 出力は $work/stdout, $work/stderr。exit code は $rc。
  rc=0
  rm -f "$work/unshare.calls"
  env PATH="$fakebin:$PATH" TMPDIR="$work" "$@" bash "$here/scripts/dev/test-parallel.sh" >"$work/stdout" 2>"$work/stderr" || rc=$?
}
summary_userns() {
  grep '^CELERIS_TEST_SUMMARY ' "$work/stdout" | sed 's/^CELERIS_TEST_SUMMARY //' \
    | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin).get("userns", "missing")))'
}
warn='^test-parallel: warning: CELERIS_USERNS_TESTS=.* cannot create an unprivileged user namespace'

# (a) require + unshare が失敗 + nextest が失敗
run CELERIS_ISOLATION_TESTS=require FAKE_UNSHARE_RC=1 FAKE_NEXTEST_LOG="$work/nextest-fail.log" FAKE_NEXTEST_RC=100
[ "$rc" -eq 100 ] || fail "(a) exit code $rc (expected 100: the preflight must not change the gate result)"
grep -q -- '-Ur true' "$work/unshare.calls" 2>/dev/null || fail "(a) unshare -Ur true was not probed"
n="$(grep -c "$warn" "$work/stderr" || true)"
[ "$n" -eq 2 ] || fail "(a) expected the userns warning twice (head and tail), got $n"
first_warn="$(grep -n "$warn" "$work/stderr" | head -n 1 | cut -d: -f1)"
nextest_line="$(grep -n '^test-parallel: cargo nextest run --workspace' "$work/stderr" | head -n 1 | cut -d: -f1)"
[ "$first_warn" -lt "$nextest_line" ] || fail "(a) the first warning is not before the nextest line"
grep -q 'ADR-0126 B4' "$work/stderr" || fail "(a) the warning does not cite ADR-0126 B4"
last="$(tail -n 1 "$work/stderr")"
[ "$last" = "test-parallel: cargo nextest run failed (exit 100)" ] || fail "(a) last stderr line is [$last]"
last_warn="$(grep -n "$warn" "$work/stderr" | tail -n 1 | cut -d: -f1)"
last_line="$(wc -l <"$work/stderr" | tr -d ' ')"
[ "$last_warn" -lt "$last_line" ] && [ "$((last_line - last_warn))" -le 3 ] || fail "(a) the second warning is not right before the last line"
grep -q '^test-parallel: failed: task-worker isolation::bwrap_operation_not_permitted' "$work/stderr" || fail "(a) failed-test name missing"
[ "$(summary_userns)" = false ] || fail "(a) summary userns is $(summary_userns) (expected false)"

# (b) USERNS_TESTS=1 + unshare が成功 + nextest が成功
run CELERIS_USERNS_TESTS=1 FAKE_UNSHARE_RC=0 FAKE_NEXTEST_LOG="$work/nextest-ok.log" FAKE_NEXTEST_RC=0
[ "$rc" -eq 0 ] || fail "(b) exit code $rc (expected 0)"
grep -q -- '-Ur true' "$work/unshare.calls" 2>/dev/null || fail "(b) unshare -Ur true was not probed"
if grep -q "$warn" "$work/stderr"; then fail "(b) warning printed although userns is available"; fi
[ "$(summary_userns)" = true ] || fail "(b) summary userns is $(summary_userns) (expected true)"
[ "$(tail -n 1 "$work/stderr")" = "test-parallel: ok" ] || fail "(b) last stderr line is not ok"

# (c) env なし: unshare を呼ばない、userns は null
run CELERIS_USERNS_TESTS=0 CELERIS_ISOLATION_TESTS= FAKE_UNSHARE_RC=1 FAKE_NEXTEST_LOG="$work/nextest-ok.log" FAKE_NEXTEST_RC=0
[ "$rc" -eq 0 ] || fail "(c) exit code $rc (expected 0)"
[ ! -e "$work/unshare.calls" ] || fail "(c) unshare was probed without the gate env"
if grep -q "$warn" "$work/stderr"; then fail "(c) warning printed without the gate env"; fi
[ "$(summary_userns)" = null ] || fail "(c) summary userns is $(summary_userns) (expected null)"

echo "test-parallel-userns-preflight: all checks passed"
