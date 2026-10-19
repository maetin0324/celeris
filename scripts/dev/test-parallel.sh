#!/usr/bin/env bash
# scripts/dev/test-parallel.sh [extra nextest args...] — テストバイナリを並列に回す（Phase SD-2、ADR-0041 §8）。
#
#   0. `cargo nextest list --workspace --list-type binaries-only`（準備: 試験 binary と試験が起動する実 bin を build し、
#      mapping の実 bin が実行できることを確かめる。落ちたら nextest run は走らせず非 0。回帰検査は
#      scripts/dev/test-cli-binary-preparation.sh）
#   1. `cargo nextest run --workspace`（バイナリをまたいで、テストごとに別プロセスで並列。直列が要るテストは
#      `.config/nextest.toml` の test-group で縛る）
#   2. `cargo test --doc --workspace`（nextest は doc-test を回さないので、別に回す）
#
# `cargo test --workspace` と同じ範囲（unit / integration / doc-test）を回す。release.sh の `cargo-test` 段はこれを使う。
# 開発者は従来どおり `cargo test --workspace` でもよい（どちらも通ることが gate の前提。CLAUDE.md は変えない）。
#
# 最後に 1 行 `CELERIS_TEST_SUMMARY {json}` を出す（release.sh が gate.json の `cargo_test` に写す）:
#   runner / nextest_version / jobs / binaries（nextest のバイナリ数 + doc-test の crate 数）/ nextest_binaries /
#   doc_binaries / passed / failed / ignored / prepare_exit / nextest_exit / doctest_exit / nextest_secs / doctest_secs
# どちらかが失敗する・集計が読めない（＝全部走った証拠が無い）なら exit 非 0。
#
# env:
#   CELERIS_TEST_JOBS   同時に走らせるテストの数。既定 min(8, max(2, nproc/3))（このホストには daemon の run が同居する）
#   CELERIS_NEXTEST_ANY_VERSION  1 なら tools/nextest/VERSION と版が違っても続ける（既定は止める）
#   CELERIS_TEST_SKIP_DOC        1 なら doc-test を回さない（開発時の短縮用。release.sh は使わない）
#   CELERIS_USERNS_TESTS / CELERIS_ISOLATION_TESTS  試験側が読む（ADR-0126 B2/B4）。1 / require のとき userns が作れない
#                                host（worker の sandbox など）では隔離の試験が設計どおり**失敗**するので、先頭と末尾に
#                                その旨の診断行を出す（gate の結果は変えない。summary の `userns` に残す）
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

# userns の preflight（ADR-0126 B2/B4）: release gate と同じ env（CELERIS_USERNS_TESTS=1 か CELERIS_ISOLATION_TESTS=require）
# なのに unprivileged user namespace が作れない host では、実 bwrap / unshare / launcher の試験が「Operation not permitted」で
# 設計どおり落ちる。原因が branch でなく環境だと読めるよう、先頭と失敗時の末尾に 1 行出す。gate の結果は変えない。
userns_wanted=false
if [ "${CELERIS_USERNS_TESTS:-0}" = 1 ] || [ "${CELERIS_ISOLATION_TESTS:-}" = require ]; then userns_wanted=true; fi
userns_ok=null
if [ "$userns_wanted" = true ]; then
  if unshare -Ur true >/dev/null 2>&1; then userns_ok=true; else userns_ok=false; fi
fi
userns_note() {
  cat >&2 <<EOF
test-parallel: warning: CELERIS_USERNS_TESTS=${CELERIS_USERNS_TESTS:-} CELERIS_ISOLATION_TESTS=${CELERIS_ISOLATION_TESTS:-} but this host cannot create an unprivileged user namespace (\`unshare -Ur true\` failed)
test-parallel:   the isolation tests (bwrap / unshare / launcher / db guard) will fail here by design (ADR-0126 B4); this is the environment, not the branch.
test-parallel:   run this gate on a host with user namespaces (the release host), or outside the gate use CELERIS_USERNS_TESTS=0 CELERIS_ISOLATION_TESTS=skip.
EOF
}
[ "$userns_ok" = false ] && userns_note

# Use a short path: Unix-domain sockets have a small fixed sun_path limit, and
# many integration fixtures put socket names below tempfile::tempdir(). A
# caller-provided TMPDIR can already be deep, so nesting another test directory
# there makes otherwise valid credential/browser IPC sockets unbindable.
logdir="$(mktemp -d "/tmp/celeris-test-parallel.XXXXXX")"
# 読み取り専用にした試験の残骸でも消せるよう権限を戻してから消す。後片付けの失敗は警告だけで exit code を変えない。
cleanup_logdir() {
  chmod -R u+w "$logdir" 2>/dev/null || true
  rm -rf "$logdir" 2>/dev/null || echo "test-parallel: warning: could not remove $logdir" >&2
  return 0
}
trap cleanup_logdir EXIT
# 試験の一時 dir は $logdir/tmp に閉じ込め、上の trap で残った物ごと消す（ADR 2026-10-07-build-tmp-hygiene D3）
mkdir -p "$logdir/tmp"
export TMPDIR="$logdir/tmp"

# 準備（build）: nextest run と同じ workspace の試験 binary と、試験が起動する実 bin（celerisctl など）を先に build し、
# nextest の binary mapping（list --list-type binaries-only）を読む。build が落ちる・mapping が読めない・実 bin が無いか
# 実行できないなら nextest は走らせず、doc-test と summary の後で準備の exit code（mapping の不備は 4）で終わる。
# 準備は追加引数に関わらず workspace 全体（追加引数は nextest run だけに渡す）。
# 再利用 target（seed の写し）では cargo が作り直さない試験 binary に、build 時の CARGO_BIN_EXE_<bin>（seed を作った
# 別 dir の絶対 path）が残ることがある（integrate-external の celerisctl NotFound）。その数を診断行に出す（exit は変えない）。
echo "test-parallel: prepare: cargo nextest list --workspace --list-type binaries-only" >&2
prc=0
cargo nextest list --workspace --list-type binaries-only --message-format json --color never \
  >"$logdir/prepare.json" || prc=$?
if [ "$prc" -eq 0 ]; then
  python3 - "$logdir/prepare.json" <<'PY' || prc=$?
import json, os, re, sys
try:
    with open(sys.argv[1], encoding="utf-8") as fh:
        meta = json.load(fh)
    build = meta["rust-build-meta"]
    target = build["target-directory"]
    suites = meta["rust-binaries"]
    non_test = build.get("non-test-binaries") or {}
except (OSError, ValueError, KeyError, TypeError) as e:
    print(f"test-parallel: prepare: cannot read the nextest binary mapping: {e}", file=sys.stderr)
    sys.exit(4)
if not suites:
    print("test-parallel: prepare: the nextest binary mapping lists no test binaries", file=sys.stderr)
    sys.exit(4)
bins = {}
missing = []
for entries in non_test.values():
    for b in entries:
        if b.get("kind") != "bin-exe":
            continue
        path = os.path.join(target, b["path"])
        bins[b["name"]] = path
        if not (os.path.isfile(path) and os.access(path, os.X_OK)):
            missing.append(f"{b['name']} ({path})")
for m in missing:
    print(f"test-parallel: prepare: CLI binary missing or not executable: {m}", file=sys.stderr)
if missing:
    sys.exit(4)
stale = []
for name, suite in sorted(suites.items()):
    try:
        with open(suite["binary-path"] + ".d", encoding="utf-8", errors="replace") as fh:
            deps = fh.read()
    except OSError:
        continue
    for bin_name, embedded in re.findall(r"^# env-dep:CARGO_BIN_EXE_([^=\s]+)=(.*)$", deps, re.M):
        if bin_name in bins and embedded != bins[bin_name]:
            stale.append((name, bin_name, embedded))
if stale:
    name, bin_name, embedded = stale[0]
    print(f"test-parallel: prepare: warning: {len(stale)} test binaries were built with CARGO_BIN_EXE_* outside this "
          f"target (e.g. {name}: CARGO_BIN_EXE_{bin_name}={embedded}); env!() in those tests still names that path",
          file=sys.stderr)
print(f"test-parallel: prepare: ok ({len(suites)} test binaries; CLI {', '.join(sorted(bins)) or '-'} in {target})",
      file=sys.stderr)
PY
fi
[ "$prc" -eq 0 ] || echo "test-parallel: prepare failed (exit $prc); cargo nextest run was not started" >&2

t0="$(date +%s.%N)"
nrc=0
if [ "$prc" -eq 0 ]; then
  echo "test-parallel: cargo nextest run --workspace (nextest $have_ver, $jobs jobs of $ncpu cpus)" >&2
  cargo nextest run --workspace --no-fail-fast --color never --hide-progress-bar \
    --test-threads "$jobs" "$@" 2>&1 | tee "$logdir/nextest.log" || nrc=$?
else
  : >"$logdir/nextest.log"
fi
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

python3 - "$logdir/nextest.log" "$logdir/doc.log" "$have_ver" "$jobs" "$nrc" "$drc" "$t0" "$t1" "$t2" "$tmp_leftovers" "$userns_ok" "$prc" <<'PY'
import json, re, sys
nlog, dlog, ver, jobs, nrc, drc, t0, t1, t2, tmp_left, userns_ok, prc = sys.argv[1:]
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
    "prepare_exit": int(prc),
    "nextest_exit": int(nrc),
    "doctest_exit": None if drc == "skipped" else int(drc),
    "nextest_secs": round(float(t1) - float(t0), 1),
    "doctest_secs": round(float(t2) - float(t1), 1),
    "tmp_leftovers": int(tmp_left),
    "summary_parsed": ok,
    # userns の preflight（CELERIS_USERNS_TESTS=1 / CELERIS_ISOLATION_TESTS=require のときだけ測る。それ以外は null）
    "userns": {"true": True, "false": False}.get(userns_ok),
}
print("CELERIS_TEST_SUMMARY " + json.dumps(out, ensure_ascii=False))
open(nlog + ".ok", "w").write("true" if ok else "false")
PY

if [ "$prc" -ne 0 ]; then echo "test-parallel: prepare failed (exit $prc)" >&2; exit "$prc"; fi
if [ "$nrc" -ne 0 ]; then
  # 落ちた test 名を最終行より前に出す（nextest の結果行 FAIL / TIMEOUT / SIG* [時間] 名前。重複を除き最大 30 行）
  grep -E '^[[:space:]]*(FAIL|TIMEOUT|SIG[A-Z]+)[[:space:]]+\[' "$logdir/nextest.log" 2>/dev/null \
    | sed -E 's/^[[:space:]]*[A-Z]+[[:space:]]+\[[^]]*\][[:space:]]*//; s/^\([0-9]+\/[0-9]+\)[[:space:]]*//' \
    | awk '!seen[$0]++' | head -n 30 | sed 's/^/test-parallel: failed: /' >&2 || true
  [ "$userns_ok" = false ] && userns_note
  echo "test-parallel: cargo nextest run failed (exit $nrc)" >&2; exit "$nrc"
fi
if [ "$drc" != skipped ] && [ "$drc" -ne 0 ]; then echo "test-parallel: cargo test --doc failed (exit $drc)" >&2; exit "$drc"; fi
if [ "$(cat "$logdir/nextest.log.ok" 2>/dev/null)" != true ]; then
  echo "test-parallel: nextest exited 0 but its Starting/Summary lines were not found; cannot prove every binary ran" >&2
  exit 3
fi
echo "test-parallel: ok" >&2
