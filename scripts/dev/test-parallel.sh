#!/usr/bin/env bash
# scripts/dev/test-parallel.sh [extra nextest args...] — テストバイナリを並列に回す（Phase SD-2、ADR-0041 §8）。
#
#   1. `cargo nextest run --workspace`（バイナリをまたいで、テストごとに別プロセスで並列。直列が要るテストは
#      `.config/nextest.toml` の test-group で縛る）
#   2. `cargo test --doc --workspace`（nextest は doc-test を回さないので、別に回す）
#
# `cargo test --workspace` と同じ範囲（unit / integration / doc-test）を回す。release.sh の `cargo-test` 段はこれを使う。
# 開発者は従来どおり `cargo test --workspace` でもよい（どちらも通ることが gate の前提。CLAUDE.md は変えない）。
#
# 最後に 1 行 `CELERIS_TEST_SUMMARY {json}` を出す（release.sh が gate.json の `cargo_test` に写す）:
#   runner / nextest_version / jobs / binaries（nextest のバイナリ数 + doc-test の crate 数）/ nextest_binaries /
#   doc_binaries / passed / failed / ignored / nextest_secs / doctest_secs
# どちらかが失敗する・集計が読めない（＝全部走った証拠が無い）なら exit 非 0。
#
# env:
#   CELERIS_TEST_JOBS   同時に走らせるテストの数。既定 min(8, max(2, nproc/3))（このホストには daemon の run が同居する）
#   CELERIS_NEXTEST_ANY_VERSION  1 なら tools/nextest/VERSION と版が違っても続ける（既定は止める）
#   CELERIS_TEST_SKIP_DOC        1 なら doc-test を回さない（開発時の短縮用。release.sh は使わない）
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
pin_file="$here/tools/nextest/VERSION"
pin="$(tr -d ' \n' <"$pin_file" 2>/dev/null || true)"
[ -n "$pin" ] || { echo "test-parallel: cannot read the pinned nextest version from $pin_file" >&2; exit 2; }

if ! have="$(cargo nextest --version 2>/dev/null | head -n 1)" || [ -z "$have" ]; then
  cat >&2 <<EOF
test-parallel: cargo-nextest is not installed (\`cargo nextest --version\` failed).
  install the pinned version once (needs the network):
    cargo install cargo-nextest --locked --version $pin
  see docs/ops/nextest.md. As a stop-gap the release gate can use plain \`cargo test --workspace\`
  with SD_GATE_TEST_RUNNER=cargo-test (docs/ops/selfdeploy.md §2).
EOF
  exit 127
fi
have_ver="$(printf '%s' "$have" | awk '{print $2}')"
if [ "$have_ver" != "$pin" ] && [ "${CELERIS_NEXTEST_ANY_VERSION:-0}" != 1 ]; then
  echo "test-parallel: cargo-nextest $have_ver is installed but tools/nextest/VERSION pins $pin" >&2
  echo "  run: cargo install cargo-nextest --locked --version $pin --force   (or CELERIS_NEXTEST_ANY_VERSION=1)" >&2
  exit 2
fi

ncpu="$(nproc 2>/dev/null || echo 4)"
jobs_default=$((ncpu / 3))
[ "$jobs_default" -lt 2 ] && jobs_default=2
[ "$jobs_default" -gt 8 ] && jobs_default=8
jobs="${CELERIS_TEST_JOBS:-$jobs_default}"

logdir="$(mktemp -d "${TMPDIR:-/tmp}/celeris-test-parallel.XXXXXX")"
trap 'rm -rf "$logdir"' EXIT
# 試験の一時 dir は $logdir/tmp に閉じ込め、上の trap で残った物ごと消す（ADR 2026-10-07-build-tmp-hygiene D3）
mkdir -p "$logdir/tmp"
export TMPDIR="$logdir/tmp"

echo "test-parallel: cargo nextest run --workspace (nextest $have_ver, $jobs jobs of $ncpu cpus)" >&2
t0="$(date +%s.%N)"
nrc=0
cargo nextest run --workspace --no-fail-fast --color never --hide-progress-bar \
  --test-threads "$jobs" "$@" 2>&1 | tee "$logdir/nextest.log" || nrc=$?
t1="$(date +%s.%N)"

drc=0
if [ "${CELERIS_TEST_SKIP_DOC:-0}" = 1 ]; then
  : >"$logdir/doc.log"
  drc=skipped
else
  echo "test-parallel: cargo test --doc --workspace" >&2
  cargo test --doc --workspace --color never 2>&1 | tee "$logdir/doc.log" || drc=$?
fi
t2="$(date +%s.%N)"

tmp_leftovers="$(find "$logdir/tmp" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l | tr -d ' ')"
if [ "$tmp_leftovers" -ne 0 ]; then
  echo "test-parallel: warning: tests left $tmp_leftovers entries in TMPDIR (removed on exit)" >&2
fi

python3 - "$logdir/nextest.log" "$logdir/doc.log" "$have_ver" "$jobs" "$nrc" "$drc" "$t0" "$t1" "$t2" "$tmp_leftovers" <<'PY'
import json, re, sys
nlog, dlog, ver, jobs, nrc, drc, t0, t1, t2, tmp_left = sys.argv[1:]
nbin = total = None
npass = nfail = nskip = 0
summary_seen = False
with open(nlog, encoding="utf-8", errors="replace") as fh:
    for line in fh:
        s = line.strip()
        m = re.match(r"Starting (\d+) tests? across (\d+) binar(?:y|ies)", s)
        if m:
            total, nbin = int(m.group(1)), int(m.group(2))
        if s.startswith("Summary [") and re.search(r"\btests? run:", s):
            summary_seen = True
            tail = s.split(":", 1)[1]
            for count, word in re.findall(r"(\d+) ([a-z ]+?)(?:,|$)", tail):
                n = int(count)
                word = word.strip()
                if word == "passed":
                    npass = n
                elif word in ("failed", "timed out", "exec failed"):
                    nfail += n
                elif word == "skipped":
                    nskip = n
dbin = dpass = dfail = dign = 0
with open(dlog, encoding="utf-8", errors="replace") as fh:
    for line in fh:
        s = line.strip()
        if s.startswith("Doc-tests "):
            dbin += 1
        m = re.match(r"test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored", s)
        if m:
            dpass += int(m.group(1)); dfail += int(m.group(2)); dign += int(m.group(3))
ok = summary_seen and nbin is not None
out = {
    "runner": "nextest",
    "nextest_version": ver,
    "jobs": int(jobs),
    "binaries": (nbin or 0) + dbin if ok else None,
    "nextest_binaries": nbin,
    "doc_binaries": dbin,
    "passed": npass + dpass,
    "failed": nfail + dfail,
    "ignored": nskip + dign,
    "nextest_exit": int(nrc),
    "doctest_exit": None if drc == "skipped" else int(drc),
    "nextest_secs": round(float(t1) - float(t0), 1),
    "doctest_secs": round(float(t2) - float(t1), 1),
    "tmp_leftovers": int(tmp_left),
    "summary_parsed": ok,
}
print("CELERIS_TEST_SUMMARY " + json.dumps(out, ensure_ascii=False))
open(nlog + ".ok", "w").write("true" if ok else "false")
PY

if [ "$nrc" -ne 0 ]; then echo "test-parallel: cargo nextest run failed (exit $nrc)" >&2; exit "$nrc"; fi
if [ "$drc" != skipped ] && [ "$drc" -ne 0 ]; then echo "test-parallel: cargo test --doc failed (exit $drc)" >&2; exit "$drc"; fi
if [ "$(cat "$logdir/nextest.log.ok" 2>/dev/null)" != true ]; then
  echo "test-parallel: nextest exited 0 but its Starting/Summary lines were not found; cannot prove every binary ran" >&2
  exit 3
fi
echo "test-parallel: ok" >&2
