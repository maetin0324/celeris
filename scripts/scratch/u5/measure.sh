#!/usr/bin/env bash
# ADR-0075 U5 の測定（Phase G3 checkpoint 1）。loopback の記録用 WebDAV stub（port 4291）に本物の sccache 0.18
# （server port 4292）を向け、発行されるメソッド・パスと backend 停止時の挙動を記録する。手動。
# usage: measure.sh <case>   case: ok | down-at-start | down-mid | slow | 500
set -u
SP="$(cd "$(dirname "$0")" && pwd)"
CASE="$1"
STUB_PORT=4291
SCC_PORT=4292
SCC=$HOME/.cargo/bin/sccache
OUT="${U5_OUT:-/var/lib/celeris/scratch/targets/agent-g3-u5}/out-$CASE"
rm -rf "$OUT"; mkdir -p "$OUT"
LOG="$OUT/stub.log"; : > "$LOG"
T=/var/lib/celeris/scratch/targets/agent-g3-u5/target
mkdir -p "$(dirname "$T")"

# project
P="$OUT/proj"
mkdir -p "$P/u5lib/src" "$P/u5bin/src"
printf '[workspace]\nmembers = ["u5lib", "u5bin"]\nresolver = "2"\n' > "$P/Cargo.toml"
printf '[package]\nname="u5lib"\nversion="0.1.0"\nedition="2021"\n' > "$P/u5lib/Cargo.toml"
echo 'pub fn f(x: u64) -> u64 { (0..x).map(|i| i * i).sum() }' > "$P/u5lib/src/lib.rs"
printf '[package]\nname="u5bin"\nversion="0.1.0"\nedition="2021"\n[dependencies]\nu5lib={path="../u5lib"}\n' > "$P/u5bin/Cargo.toml"
echo 'fn main() { println!("{}", u5lib::f(10)); }' > "$P/u5bin/src/main.rs"

STUB_PID=""
start_stub() { python3 "$SP/webdav-record-stub.py" $STUB_PORT "$LOG" "${1:-ok}" & STUB_PID=$!; sleep 0.5; }
stop_stub() { [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null; wait "$STUB_PID" 2>/dev/null; STUB_PID=""; }

export SCCACHE_SERVER_PORT=$SCC_PORT
export SCCACHE_WEBDAV_ENDPOINT=http://127.0.0.1:$STUB_PORT
export SCCACHE_WEBDAV_KEY_PREFIX=sccache
export SCCACHE_WEBDAV_TOKEN=u5-secret-token
export SCCACHE_IDLE_TIMEOUT=0
export SCCACHE_LOG=debug
export SCCACHE_ERROR_LOG="$OUT/sccache-server.log"
export SCCACHE_DIR="$OUT/unused-disk"

build() {
  rm -rf "$T"
  ( cd "$P" && CARGO_TARGET_DIR=$T CARGO_INCREMENTAL=0 RUSTC_WRAPPER=$SCC cargo build --offline -q 2>&1 ); echo "build exit=$?"
}

case "$CASE" in
  ok)
    start_stub ok
    $SCC --start-server; echo "start-server exit=$?"
    build; $SCC --show-stats --stats-format=json > "$OUT/stats1.json"
    $SCC --zero-stats >/dev/null
    build; $SCC --show-stats --stats-format=json > "$OUT/stats2.json"
    ;;
  down-at-start)
    $SCC --start-server; echo "start-server exit=$?"
    ;;
  down-mid)
    start_stub ok
    $SCC --start-server; echo "start-server exit=$?"
    stop_stub
    build; $SCC --show-stats --stats-format=json > "$OUT/stats1.json"
    ;;
  slow)
    start_stub slow
    $SCC --start-server; echo "start-server exit=$?"
    s=$(date +%s%3N); build; e=$(date +%s%3N); echo "build ms=$((e-s))"
    $SCC --show-stats --stats-format=json > "$OUT/stats1.json"
    ;;
  500)
    start_stub 500
    $SCC --start-server; echo "start-server exit=$?"
    build; $SCC --show-stats --stats-format=json > "$OUT/stats1.json"
    ;;
esac
$SCC --stop-server >/dev/null 2>&1
stop_stub
echo "--- stub log"
cat "$LOG"
