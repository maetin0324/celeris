#!/usr/bin/env bash
# scripts/selfdeploy/tests/release_lock_bundled_scripts.sh — ADR 2026-10-16-release-lock-bundled-scripts:
#
#   release に同梱した scripts（<release>/scripts/release.sh。delivery の prepare.sh が current/scripts から起こす）が
#   持つ lock を、sd_lock_or_tempfail が「漏れた fd」と誤判定して退けないこと。退けると同じ sha の 2 本目が
#   同時に走り、<sha12>.partial を互いに消し合う（2026-10-16 04:17Z / 05:42Z の web-bundle 失敗の原因）。
#   また、selfdeploy の script ではない持ち主（漏れた fd）の lock は従来どおり退けること。
#
# 待ちは fifo の read（CPU を使わない）。時計・負荷に依らず決定的。
# 実行: bash scripts/selfdeploy/tests/release_lock_bundled_scripts.sh
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(mktemp -d)"
holder_pid=""
cleanup() {
  [ -n "$holder_pid" ] && kill "$holder_pid" 2>/dev/null || true
  rm -rf "$root"
}
trap cleanup EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }

# release に同梱された形（平ら）: <release>/scripts/{lib.sh,release.sh}
mkdir -p "$root/rel/scripts" "$root/state"
cp "$HERE/../lib.sh" "$root/rel/scripts/lib.sh"
export CELERIS_STATE_DIR="$root/state" CELERIS_CONFIG_DIR="$root/cfg"
lock="$root/state/lock"

# release.sh の代役: lock を取り、取れたら ready を書いて fifo を読んで待つ。
cat >"$root/rel/scripts/release.sh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
SD_PROG=release-stub
sd_lock_or_tempfail 8 "$1" 0 "stub release"
: >"$2"
read -r _ <"$3"
SH

mkfifo "$root/go"
bash "$root/rel/scripts/release.sh" "$lock" "$root/ready" "$root/go" 8>&- 2>"$root/holder.err" &
holder_pid=$!
for _ in $(seq 1 200); do [ -e "$root/ready" ] && break; sleep 0.05; done
[ -e "$root/ready" ] || { cat "$root/holder.err" >&2; fail "holder did not take the lock"; }

# 2 本目（リポジトリ側の起動と同じ lib を使う）: 同梱 script が持つ lock は退けず、exit 75 で諦める。
set +e
bash -c '. "$1"; SD_PROG=second; sd_lock_or_tempfail 8 "$2" 0 "stub release"' _ "$HERE/../lib.sh" "$lock" 2>"$root/second.err"
rc=$?
set -e
[ "$rc" = 75 ] || { cat "$root/second.err" >&2; fail "second release got rc=$rc, want 75 (EX_TEMPFAIL)"; }
ls "$root/state"/lock.leaked-* >/dev/null 2>&1 && fail "a live bundled release.sh's lock was moved aside as leaked"
echo "ok: lock held by <release>/scripts/release.sh is not treated as leaked"

# 持ち主が終わる（fifo に書く）。
echo go >"$root/go"
wait "$holder_pid" || true
holder_pid=""

# 漏れた fd: selfdeploy の script ではない process が lock を握っているとき（従来の挙動）は退けて取り直す。
lock2="$root/state/lock2"
mkfifo "$root/go2"
bash -c 'exec 7>>"$1"; flock -n 7 || exit 1; : >"$3"; read -r _ <"$2"' _ "$lock2" "$root/go2" "$root/ready2" &
holder_pid=$!
for _ in $(seq 1 200); do [ -e "$root/ready2" ] && break; sleep 0.05; done
[ -e "$root/ready2" ] || fail "test setup: non-script holder did not take the lock"
set +e
bash -c '. "$1"; SD_PROG=third; sd_lock_or_tempfail 8 "$2" 0 "stub"' _ "$HERE/../lib.sh" "$lock2" 2>"$root/third.err"
rc=$?
set -e
echo go >"$root/go2"
wait "$holder_pid" || true
holder_pid=""
[ "$rc" = 0 ] || { cat "$root/third.err" >&2; fail "leaked (non-script) holder was not moved aside: rc=$rc"; }
ls "$root/state"/lock2.leaked-* >/dev/null 2>&1 || fail "no .leaked- file for a non-script holder"
echo "ok: a lock held only by a non-selfdeploy process is still moved aside"
echo "release_lock_bundled_scripts: ok"
