#!/bin/sh
# scripts/dev/test-parallel.sh が nextest 失敗時に落ちた test 名を stderr の最終行より前に出すことを確かめる。
# cargo は偽物（PATH の先頭に置く）に置き換え、nextest の出力だけを与える。cargo は一切走らせない。
# 確かめること: (a) FAIL / TIMEOUT / SIG* の結果行から test 名が重複なしで出る、(b) 最終行は
# `cargo nextest run failed (exit N)` のまま、(c) CELERIS_TEST_SUMMARY の行が出る。
set -eu

here="$(cd "$(dirname "$0")/../../.." && pwd)"
work="$(mktemp -d "${TMPDIR:-/tmp}/test-parallel-fail-names.XXXXXX")"
trap 'chmod -R u+w "$work" 2>/dev/null || true; rm -rf "$work"' EXIT
fakebin="$work/bin"
mkdir -p "$fakebin"

cat >"$work/nextest.log" <<'EOF'
    Starting 5 tests across 3 binaries
        PASS [   0.010s] task-core tests::ok
        FAIL [   0.104s] task-worker tests::broken_one
     TIMEOUT [  60.000s] (1/5) task-dispatch slow::test
      SIGABRT [   0.002s] celeris crash::test
        FAIL [   0.104s] task-worker tests::broken_one
     Summary [   0.200s] 5 tests run: 2 passed, 3 failed, 0 skipped
EOF

# 偽の cargo: nextest --version は pin を返し、nextest run は偽のログを出して 100 で終わる。doc-test は成功。
cat >"$fakebin/cargo" <<EOF
#!/bin/sh
case "\$1 \$2" in
  "nextest --version") echo "cargo-nextest 0.9.146" ;;
  "nextest run") cat "$work/nextest.log"; exit 100 ;;
  "test --doc") exit 0 ;;
  *) echo "fake cargo: unexpected args: \$*" >&2; exit 99 ;;
esac
EOF
chmod +x "$fakebin/cargo"

out="$work/stdout"
err="$work/stderr"
rc=0
PATH="$fakebin:$PATH" TMPDIR="$work" bash "$here/scripts/dev/test-parallel.sh" >"$out" 2>"$err" || rc=$?

fail() {
  echo "FAIL: $*" >&2
  echo "--- stderr ---" >&2
  cat "$err" >&2
  exit 1
}

[ "$rc" -eq 100 ] || fail "exit code $rc (expected 100)"

expected="test-parallel: failed: task-worker tests::broken_one
test-parallel: failed: task-dispatch slow::test
test-parallel: failed: celeris crash::test"
got="$(grep '^test-parallel: failed: ' "$err")"
[ "$got" = "$expected" ] || fail "failed-test lines differ: got [$got]"

last="$(tail -n 1 "$err")"
[ "$last" = "test-parallel: cargo nextest run failed (exit 100)" ] || fail "last stderr line is [$last]"

grep -q '^test-parallel: failed: ' "$err" || fail "no failed-test line"
# 失敗名は最終行より前に出ること
names_line="$(grep -n '^test-parallel: failed: ' "$err" | tail -n 1 | cut -d: -f1)"
last_line="$(wc -l <"$err" | tr -d ' ')"
[ "$names_line" -lt "$last_line" ] || fail "failed-test lines are not before the last line"

grep -q '^CELERIS_TEST_SUMMARY {' "$out" || fail "CELERIS_TEST_SUMMARY line missing from stdout"

echo "test-parallel-fail-names: all checks passed"
