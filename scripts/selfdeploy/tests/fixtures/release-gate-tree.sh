# release.sh の gate を偽の道具と一時 dir の偽の木で回す試験の共有の fixture（bash で source する）。
# 使う試験: release_parallel_test_gate.sh・release_record_summary_survives_prune.sh。
# 以前は 2 本が同じ偽の道具と偽の木を写して持ち、release.sh・test-parallel.sh が読む file が増えるたびに片方だけが
# 古くなった（check-slot.sh・cos-skills.txt の不足）。ここに 1 つだけ置く。
#
# source の前に root（一時 dir）を決めておく。source すると次を用意する（本番の path・network には触れない）:
#   $root/bin/{cargo,pnpm,node,celerisctl}  偽の道具
#   $repo（= $root/repo）                     release.sh・test-parallel.sh が読む file を写した git の木（commit はまだ無い）
#   new_commit                               crates/x/src/lib.rs を書き換えて commit し、40 桁の sha を出す
#   SD_SCRIPT_BUNDLE_GATE・SD_WEB_PNPM       TS 制御層の bundle の段と web の release の偽物（export）
# 偽の道具が読む env（release.sh に渡す）:
#   CARGO_LOG（必須）: 偽の cargo が引数を 1 行ずつ追記する file
#   FAKE_SCRATCH: 偽の celerisctl の `scratch lease` が返す target の根
#   FAKE_NEXTEST_FAIL=1 → nextest の試験が 1 つ落ちる（exit 100）、FAKE_DOC_FAIL=1 → doc-test が落ちる（exit 101）、
#   FAKE_NEXTEST_NOSUMMARY=1 → exit 0 だが集計の行を出さない、FAKE_NO_NEXTEST=1 → `cargo nextest` が無い

_rgt_fixtures="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
_rgt_sd="$(cd "$_rgt_fixtures/../.." && pwd)"
_rgt_top="$(cd "$_rgt_sd/../.." && pwd)"
mkdir -p "$root/bin"

# control-ts を持たない偽の木なので、TS 制御層の bundle の段は偽物にする（本物は scripts/dev/test-release-script-gate.sh）。
export SD_SCRIPT_BUNDLE_GATE="$_rgt_fixtures/fake-script-bundle-gate.sh"

# 偽の cargo。nextest は本物の出力の形（Starting / Summary 行）を真似る。
# Summary 行には本物と同じく `passed` の後に括弧の注記（`(1 slow)`・`(1 slow, 1 leaky)`）を付ける。本物の nextest は遅い試験が
# あると `8525 passed (3 slow), 22 skipped` と書くが、以前の fixture は注記の無い `7 passed, 2 skipped` だけを出していたので、
# test-parallel.sh の解析が注記付きの行で `passed` の組を取りこぼし、gate.json の `passed` が doc-test の数だけになる誤りを
# 検出できなかった（ADR 2026-11-20-release-record-summary-log 1.1）。注記付きの行を固定して、この取りこぼしを落とす。
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
          printf '     Summary [   0.020s] 7 tests run: 6 passed (1 slow, 1 leaky), 1 failed, 2 skipped\n'
          printf 'error: test run failed\n' >&2
          exit 100
        fi
        printf '     Summary [   0.020s] 7 tests run: 7 passed (1 slow), 2 skipped\n'
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

# release.sh は自分の NoNewPrivs を読み、1 なら試験 9 を gate から外す（ADR 2026-11-21-release-gate-second-stage-skip G5）。
# 試験の結果がこの worker・CI の NoNewPrivs に左右されないよう、既定は「NoNewPrivs の行が無い」file に固定する
# （release_host_tests_gate.sh だけが値を指定する）。
: >"$root/proc-self-status"
export SD_PROC_SELF_STATUS="${SD_PROC_SELF_STATUS:-$root/proc-self-status}"

# ---- 偽の木（release.sh・test-parallel.sh が読む file を写す） ----
repo="$root/repo"
mkdir -p "$repo/crates/task-core/src/store" "$repo/crates/celeris" "$repo/crates/x/src" "$repo/scripts/selfdeploy"
cp "$_rgt_sd"/*.sh "$_rgt_sd"/*.py "$repo/scripts/selfdeploy/"
# release.sh は cos-skills.txt の CoS skill を梱包する（sd_bundle_cos_skills、ADR 2026-11-11-cos-skill-sync）。偽の木は skill を
# 持たないので空の一覧を置く。
printf '# fixture: no CoS skills\n' >"$repo/scripts/selfdeploy/cos-skills.txt"
chmod +x "$repo/scripts/selfdeploy/"*.sh
# runner はビルドする sha の中のものを使う（release.sh の隣ではない）。
mkdir -p "$repo/scripts/dev/lib" "$repo/scripts/dev/repo-health" "$repo/tools/nextest"
cp "$_rgt_top/scripts/dev/test-parallel.sh" "$_rgt_top/scripts/dev/source-size-report.py" "$repo/scripts/dev/"
# test-parallel.sh が読む helper（check-slot.sh は ADR 2026-11-12-host-check-slots の後に読む）と、release.sh が同梱する
# 検査器（sd_bundle_repo_health は repo_health.py と source-size-report.py の無い build を失敗にする）。
cp "$_rgt_top/scripts/dev/lib/test-tmp-lifecycle.sh" "$_rgt_top/scripts/dev/lib/check-slot.sh" "$repo/scripts/dev/lib/"
cp "$_rgt_top/scripts/dev/repo-health/repo_health.py" "$repo/scripts/dev/repo-health/"
cp "$_rgt_top/tools/nextest/VERSION" "$repo/tools/nextest/"
printf 'pub const SCHEMA_VERSION: u32 = 7;\n' >"$repo/crates/task-core/src/store/migrations.rs"
printf '[package]\nname = "celeris"\nversion = "0.1.0"\n' >"$repo/crates/celeris/Cargo.toml"
. "$_rgt_fixtures/web-release.sh"
git init -q "$repo"
unset _rgt_fixtures _rgt_sd _rgt_top

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
