#!/bin/sh
# job.sh・expected.sha256・spec.template.json の自己試験（ADR I15 (4)）。
# 使い方: sh scripts/campaign/recovery-smoke/selftest.sh（引数なし。$TMPDIR の下だけに書く）
set -eu

here=$(cd "$(dirname "$0")" && pwd)
base=${TMPDIR:-/tmp}
work=$(mktemp -d "$base/recovery-smoke-selftest.XXXXXX")
trap 'rm -rf "$work"' EXIT INT TERM

fail() { echo "FAIL: $*" >&2; exit 1; }

expected=$(cat "$here/expected.sha256")
in_sha=$(sha256sum < "$here/input.bin" | cut -d ' ' -f 1)

prepare() {
    mkdir -p "$1/release"
    cp "$here/job.sh" "$here/input.bin" "$1/"
}

# (1) release あり: out.bin の sha256 が expected.sha256 と一致
d1=$work/with-release
prepare "$d1"
touch "$d1/release/101.selftest"
RECOVERY_SMOKE_DIR=$d1 PBS_JOBID=101.selftest RECOVERY_SMOKE_RELEASE_TIMEOUT=10 \
    sh "$d1/job.sh" || fail "job.sh exited non-zero with release"
a1=$d1/out/101.selftest
[ -f "$a1/out.bin" ] || fail "out.bin missing with release"
got=$(sha256sum < "$a1/out.bin" | cut -d ' ' -f 1)
[ "$got" = "$expected" ] || fail "out.bin sha256 $got != expected $expected"
cmp -s "$a1/input.copy" "$here/input.bin" || fail "input.copy differs from input.bin"
[ "$(cat "$a1/input.sha256")" = "$in_sha" ] || fail "input.sha256 mismatch"
echo "ok: release present -> out.bin sha256 $got"

# 別 job ID でも out.bin は同じ（job ID・時刻・host を含まない）
touch "$d1/release/102.selftest"
RECOVERY_SMOKE_DIR=$d1 PBS_JOBID=102.selftest sh "$d1/job.sh" || fail "second job failed"
cmp -s "$d1/out/102.selftest/out.bin" "$a1/out.bin" || fail "out.bin depends on job id"
echo "ok: out.bin independent of PBS_JOBID"

# (2) release なし: timeout 2 秒で非 0、out.bin なし
d2=$work/without-release
prepare "$d2"
rc=0
RECOVERY_SMOKE_DIR=$d2 PBS_JOBID=201.selftest RECOVERY_SMOKE_RELEASE_TIMEOUT=2 \
    sh "$d2/job.sh" 2>"$work/no-release.err" || rc=$?
[ "$rc" -ne 0 ] || fail "job.sh exited 0 without release"
[ ! -e "$d2/out/201.selftest/out.bin" ] || fail "out.bin written without release"
[ ! -e "$d2/out/201.selftest/out.bin.partial" ] || fail "partial out.bin left without release"
echo "ok: release absent -> exit $rc, no out.bin"

# (3) spec.template.json が JSON として読める
python3 -I -c 'import json,sys; json.load(open(sys.argv[1]))' "$here/spec.template.json" \
    || fail "spec.template.json is not valid JSON"
echo "ok: spec.template.json parses"

echo "selftest: PASS"
