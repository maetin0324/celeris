#!/usr/bin/env bash
# scripts/dev/tests/test-runner-tmp-lifecycle.sh — scripts/dev/lib/test-tmp-lifecycle.sh（試験 runner の一時領域の寿命管理）の単体試験。
# 引数なしで呼ぶ。採用設計は agent-docs/adr/2026-11-06-test-runner-tmp-lifecycle.md の D1〜D4。
#
# 実 cargo・LLM・ネットワーク・負荷台本は使わない。runner の代わりに、helper を source する小さな driver と、
# fifo で ready/release を受け渡す偽の子 process を使う。待ちはすべて出来事待ち（fifo の read -t / 期限付きの poll）で、
# 固定 sleep で時間を当てない。時刻は TTL_NOW の注入と fixture の touch -d、別 UID は TTL_UID の注入で代用する。
# SIGKILL は捕捉できないので、直後に dir が消えることは試験しない。次回回収の条件（lock・pid+starttime・session・
# boot_id・/proc 不明・若い dir・owner 不明・並行・symlink・別 UID）だけを固定する。
# 試験が作った dir と process は、成功・失敗・中断のどれでも終了時に回収し、残っていないことを自分で確かめる（残れば exit 1）。
# `sh <script>` で呼ばれても bash で動かす（dash は ${BASH_SOURCE[0]} を構文エラーにする）。
[ -n "${BASH_VERSION:-}" ] || exec bash "$0" "$@"
set -uo pipefail
# fixture の runner が配線済みの検査（test-affected.sh の scripts/dev 変更時の検査）を再び呼ばないための guard。
export CELERIS_RUNNER_TMP_TEST=1

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
LIB="$here/scripts/dev/lib/test-tmp-lifecycle.sh"
[ -f "$LIB" ] || { echo "missing $LIB" >&2; exit 1; }

# Unix socket の path 予算を試すので、試験の根は短い path にする（TMPDIR は長いことがある）。
base="$(mktemp -d /tmp/ttl-t.XXXXXX)" || exit 1
PIDS="$base/pids"
: >"$PIDS"
# 何も書かれない fifo。`read -t` の待ち専用（busy loop にしない）。
mkfifo "$base/idle"
exec {IDLE}<>"$base/idle"
fails=0
checks=0
RENV=()

ok() { checks=$((checks + 1)); }
bad() { checks=$((checks + 1)); fails=$((fails + 1)); echo "FAIL: $*" >&2; }
expect_eq() { if [ "$2" = "$3" ]; then ok; else bad "$1: got '$2' want '$3'"; fi; }
expect() { local msg=$1; shift; if "$@"; then ok; else bad "$msg"; fi; }
expect_not() { local msg=$1; shift; if "$@"; then bad "$msg"; else ok; fi; }

pid_alive() {
  local s
  { IFS= read -r s <"/proc/$1/stat"; } 2>/dev/null || return 1
  s=${s##*) }
  [ "${s%% *}" != Z ]
}

cleanup() {
  local rc=$? p deadline
  trap - EXIT INT TERM HUP
  while read -r p; do
    [ -n "$p" ] || continue
    kill -KILL -- "-$p" 2>/dev/null || true
    kill -KILL "$p" 2>/dev/null || true
  done <"$PIDS"
  deadline=$((SECONDS + 10))
  while read -r p; do
    [ -n "$p" ] || continue
    while pid_alive "$p" && [ "$SECONDS" -lt "$deadline" ]; do read -r -t 0.05 -u "$IDLE" _ || true; done
    if pid_alive "$p"; then echo "FAIL: process $p still alive after cleanup" >&2; rc=1; fi
  done <"$PIDS"
  chmod -R u+w "$base" 2>/dev/null || true
  rm -rf --one-file-system "$base"
  if [ -e "$base" ]; then echo "FAIL: $base still exists after cleanup" >&2; rc=1; fi
  exit "$rc"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP

# ---- fixture: driver と偽の子 ---------------------------------------------------------------------------------
cat >"$base/drv.sh" <<DRV
set -uo pipefail
. "$LIB"
echo \$\$ >>"\$PIDS"
ttl_install_traps
ttl_init drv || exit 70
export TTL_LOCK_FD
echo "\$TTL_DIR" >"\$OUT/dir"
ttl_run bash "$base/child.sh"
exit \$?
DRV
cat >"$base/child.sh" <<'CHILD'
echo $$ >>"$PIDS"
echo $$ >"$OUT/childpid"
case "${CHILD_MODE:-}" in
  closefd) eval "exec ${TTL_LOCK_FD}>&-" ;;
  ignore) trap '' INT TERM HUP ;;
esac
echo ready >"$OUT/ready"
exec 8<>"$OUT/release"
read -r -t 60 -u 8 _ || true
[ "${CHILD_MODE:-}" = exit7 ] && exit 7
exit 0
CHILD

# start_runner <case> <child mode>: driver を専用 session で起こし、子の ready を待つ。OUT ROOT RPID DIR CPID を設定。
start_runner() {
  OUT="$base/o-$1"
  ROOT="$base/r-$1"
  mkdir -p "$OUT"
  mkfifo "$OUT/ready" "$OUT/release"
  exec {RFD}<>"$OUT/ready"
  exec {XFD}<>"$OUT/release"
  # job control を一時的に有効にして起こす。無いと非同期 command の SIGINT は無視のまま継承され、INT の試験にならない。
  # driver は自分の process group の先頭になる（pgid = RPID）。子の session は ttl_run が setsid で別に作る。
  set -m
  env CHILD_MODE="$2" OUT="$OUT" PIDS="$PIDS" CELERIS_TEST_TMP_ROOT="$ROOT" "${RENV[@]}" \
    bash "$base/drv.sh" 2>"$OUT/err" &
  RPID=$!
  set +m
  echo "$RPID" >>"$PIDS"
  if ! read -r -t 60 -u "$RFD" _; then bad "$1: child never became ready"; fi
  DIR="$(cat "$OUT/dir" 2>/dev/null)"
  CPID="$(cat "$OUT/childpid" 2>/dev/null)"
}
end_runner() { exec {RFD}<&- {XFD}<&-; }
release_child() { echo go >&"$XFD"; }
# wait_gone <pid>: 出来事待ち（期限 60 秒）
wait_gone() {
  local deadline=$((SECONDS + 60))
  while pid_alive "$1" && [ "$SECONDS" -lt "$deadline" ]; do read -r -t 0.05 -u "$IDLE" _ || true; done
  ! pid_alive "$1"
}
runner_rc() { wait "$RPID"; RC=$?; }

# run_sweep <root>: 別 process で ttl_sweep を 1 回走らせ、数を 1 行で返す。環境変数は呼び出し側の前置で注入する。
run_sweep() {
  bash -c '. "$1"; ttl_sweep "$2"; echo "swept=$TTL_N_SWEPT alive=$TTL_N_ALIVE foreign=$TTL_N_FOREIGN unknown=$TTL_N_UNKNOWN skipped=$TTL_SWEEP_SKIPPED"' _ "$LIB" "$1" 2>"$base/sweep.err"
}
field() { local s=$1 k=$2; s=" $s"; s=${s#* $k=}; echo "${s%% *}"; }

BOOT="$(cat /proc/sys/kernel/random/boot_id)"
NOW=2000000000
dead_pid() { bash -c 'echo $$'; }
DEADPID="$(dead_pid)"
starttime_of() { local s; s="$(cat "/proc/$1/stat")"; s=${s##*) }; set -- $s; echo "${20}"; }
sid_of() { local s; s="$(cat "/proc/$1/stat")"; s=${s##*) }; set -- $s; echo "$4"; }
mkroot() { mkdir -p -m 0700 "$1"; }
# mk_run <root> <name> <pid> <starttime> <boot> <sid>: 公開済みの実行 dir fixture
mk_run() {
  local d="$1/$2"
  mkdir -p -m 0700 "$d/t"
  : >"$d/lock"
  printf 'uid=%s\npid=%s\nstarttime=%s\nsid=%s\nboot_id=%s\nrunner=fx\ncreated=1\n' "$(id -u)" "$3" "$4" "$6" "$5" >"$d/owner"
  echo data >"$d/t/f"
}
mk_dead() { mk_run "$1" "$2" "$DEADPID" 1 "$BOOT" "$DEADPID"; }

# ===================================================================================================================
# 1. 通常終了・非 0 終了
start_runner normal none
expect "normal: DIR is under the root" test "$DIR" != "" -a -d "$DIR"
expect_eq "normal: TMPDIR is DIR/t (owner published with every key)" "$(grep -c -E '^(uid|pid|starttime|sid|boot_id|runner|created)=' "$DIR/owner")" 7
expect_eq "normal: TMPDIR length within budget" "$([ $((${#DIR} + 2)) -le 47 ] && echo y)" y
expect_eq "normal: DIR/t exists" "$([ -d "$DIR/t" ] && echo y)" y
expect "normal: lock is held while running" bash -c '! flock -n "$1"' _ "$DIR/lock" 2>/dev/null
expect_eq "normal: root mode" "$(stat -c %a "$ROOT")" 700
release_child
runner_rc; expect_eq "normal: rc" "$RC" 0
expect_not "normal: dir removed" test -e "$DIR"
expect "normal: root kept" test -d "$ROOT"
expect_eq "normal: no .init/.trash left" "$(ls -A "$ROOT" | grep -v '^.sweep.lock$' | wc -l)" 0
end_runner

start_runner exit7 exit7
release_child
runner_rc; expect_eq "nonzero: rc is the child's rc" "$RC" 7
expect_not "nonzero: dir removed" test -e "$DIR"
end_runner

# 2. 捕捉できる signal: 親だけへ送ると、子の session へ転送され、子の終了後に dir が消えて 128+N で終わる
for spec in INT:130 TERM:143 HUP:129; do
  sig=${spec%%:*} want=${spec##*:}
  start_runner "sig-$sig" none
  kill -s "$sig" "$RPID"
  runner_rc; expect_eq "$sig: rc" "$RC" "$want"
  expect_not "$sig: child process ended (signal forwarded)" pid_alive "$CPID"
  expect_not "$sig: dir removed after the child ended" test -e "$DIR"
  end_runner
done

# 3. 子が signal を無視する: 待ちの上限（注入）で 128+N で終わり、dir は残る。子が終われば次回回収で消える
RENV=(TTL_SIGNAL_WAIT=1)
start_runner ignore ignore
kill -TERM "$RPID"
runner_rc; expect_eq "ignore: rc" "$RC" 143
expect "ignore: child still alive" pid_alive "$CPID"
expect "ignore: dir kept while the child runs" test -d "$DIR"
sw="$(run_sweep "$ROOT")"
expect_eq "ignore: sweep keeps the dir (lock/session)" "$(field "$sw" swept)/$(field "$sw" alive)" 0/1
expect "ignore: dir still there after sweep" test -d "$DIR"
release_child
wait_gone "$CPID"
sw="$(run_sweep "$ROOT")"
expect_eq "ignore: next sweep reclaims after the child ended" "$(field "$sw" swept)" 1
expect_not "ignore: dir removed by the sweep" test -e "$DIR"
end_runner
RENV=()

# 4. 親だけ KILL: 孤児の子が lock を持つ間は残し、子が終われば次回回収
start_runner kill-parent none
kill -KILL "$RPID"
wait_gone "$RPID"; wait "$RPID" 2>/dev/null
sw="$(run_sweep "$ROOT")"
expect_eq "kill-parent: orphan child still holds the lock → kept" "$(field "$sw" swept)/$(field "$sw" alive)" 0/1
release_child
wait_gone "$CPID"
sw="$(run_sweep "$ROOT")"
expect_eq "kill-parent: reclaimed once the child is gone" "$(field "$sw" swept)" 1
expect_not "kill-parent: dir removed" test -e "$DIR"
end_runner

# 5. group KILL（runner の group と子の session の両方）: 次の runner の起動時回収で消える
start_runner kill-group none
kill -KILL -- "-$RPID" 2>/dev/null
kill -KILL -- "-$CPID" 2>/dev/null
wait_gone "$RPID"; wait "$RPID" 2>/dev/null
wait_gone "$CPID"
dirsave="$DIR"
# 次の runner の起動（ttl_init）が、同じ root の終わった dir を回収する
b="$(CELERIS_TEST_TMP_ROOT="$ROOT" bash -c '. "$1"; ttl_install_traps; ttl_init next && echo "$TTL_DIR"' _ "$LIB" 2>"$base/next.err")"
expect "kill-group: next run got its own dir" test -n "$b"
expect_not "kill-group: init of the next run reclaimed the finished dir" test -e "$dirsave"
expect_not "kill-group: next run's own dir removed on its exit" test -e "$b"
end_runner

# 6. 子が lock の fd を閉じる: lock だけでは守れないが、session の走査で守る
start_runner closefd closefd
expect "closefd: lock is NOT held by the child's closed fd (sanity: only runner holds it)" test -d "$DIR"
kill -KILL "$RPID"
wait_gone "$RPID"; wait "$RPID" 2>/dev/null
expect "closefd: sanity — lock is free after the runner died" flock -n "$DIR/lock" true
sw="$(run_sweep "$ROOT")"
expect_eq "closefd: kept by the session scan" "$(field "$sw" swept)/$(field "$sw" alive)" 0/1
release_child
wait_gone "$CPID"
sw="$(run_sweep "$ROOT")"
expect_eq "closefd: reclaimed after the child ended" "$(field "$sw" swept)" 1
end_runner

# 7. PID 再利用相当・boot_id
r="$base/r-pid"
mkroot "$r"
me=$$
st="$(starttime_of "$me")"
mk_run "$r" reuse "$me" "$((st + 1))" "$BOOT" "$DEADPID"
mk_run "$r" same "$me" "$st" "$BOOT" "$DEADPID"
mk_run "$r" newboot "$me" "$st" "00000000-0000-0000-0000-000000000000" "$DEADPID"
mk_dead "$r" gone
sw="$(run_sweep "$r")"
expect_eq "pid: swept (reuse, new boot, dead pid)" "$(field "$sw" swept)" 3
expect_not "pid: reused pid (starttime differs) removed" test -e "$r/reuse"
expect_not "pid: other boot removed" test -e "$r/newboot"
expect_not "pid: dead pid removed" test -e "$r/gone"
expect "pid: live owner with matching starttime kept" test -d "$r/same"
expect_eq "pid: alive count" "$(field "$sw" alive)" 1

# 8. 若い dir・初期化途中
r="$base/r-init"
mkroot "$r"
mkdir -m 0700 "$r/.init-young"; : >"$r/.init-young/owner.tmp"; touch -d "@$((NOW - 10))" "$r/.init-young"
mkdir -m 0700 "$r/.init-oldpart"; : >"$r/.init-oldpart/owner.tmp"; touch -d "@$((NOW - 7200))" "$r/.init-oldpart"
mkdir -m 0700 "$r/.init-oldempty"; touch -d "@$((NOW - 7200))" "$r/.init-oldempty"
mkdir -m 0700 "$r/.init-oldother"; : >"$r/.init-oldother/lock"; touch -d "@$((NOW - 7200))" "$r/.init-oldother"
mkdir -m 0700 "$r/.init-oldowner"; : >"$r/.init-oldowner/owner"; touch -d "@$((NOW - 7200))" "$r/.init-oldowner"
mkdir -m 0700 "$r/.init-edge"; touch -d "@$((NOW - 3599))" "$r/.init-edge"
mkdir -m 0700 "$r/.trash-crashed"; echo x >"$r/.trash-crashed/f"
sw="$(TTL_NOW=$NOW run_sweep "$r")"
expect_not "init: old, owner.tmp only → removed" test -e "$r/.init-oldpart"
expect_not "init: old, empty → removed" test -e "$r/.init-oldempty"
expect "init: young (10 s) kept" test -d "$r/.init-young"
expect "init: 3599 s kept" test -d "$r/.init-edge"
expect "init: old but other contents kept" test -d "$r/.init-oldother"
expect "init: old but owner present kept" test -d "$r/.init-oldowner"
expect_not "init: leftover .trash removed" test -e "$r/.trash-crashed"
expect_eq "init: swept count" "$(field "$sw" swept)" 3
# 若い published dir（owner の pid が生きている）も残る
mk_run "$r" young "$me" "$st" "$BOOT" "$DEADPID"
TTL_NOW=$NOW run_sweep "$r" >/dev/null
expect "init: young published dir kept" test -d "$r/young"

# 9. owner 不明
r="$base/r-owner"
mkroot "$r"
mkdir -m 0700 "$r/noowner"; : >"$r/noowner/lock"
mk_dead "$r" garbled; echo 'pid=abc' >"$r/garbled/owner"
mk_dead "$r" missingkey; printf 'uid=1\n' >"$r/missingkey/owner"
mk_dead "$r" nolock; rm "$r/nolock/lock"
mk_dead "$r" ownerlink; rm "$r/ownerlink/owner"; ln -s /etc/hostname "$r/ownerlink/owner"
sw="$(run_sweep "$r")"
expect_eq "owner: nothing swept" "$(field "$sw" swept)" 0
expect_eq "owner: unknown count" "$(field "$sw" unknown)" 5
for n in noowner garbled missingkey nolock ownerlink; do expect "owner: $n kept" test -d "$r/$n"; done

# 10. /proc が読めない
r="$base/r-proc"
mkroot "$r"
mk_dead "$r" d1
mkdir "$base/emptyproc"
sw="$(TTL_PROC="$base/nonexistent" run_sweep "$r")"
expect_eq "proc: missing /proc → kept" "$(field "$sw" swept)/$(field "$sw" unknown)" 0/1
sw="$(TTL_PROC="$base/emptyproc" run_sweep "$r")"
expect_eq "proc: empty /proc (no self/stat) → kept" "$(field "$sw" swept)/$(field "$sw" unknown)" 0/1
expect "proc: dir kept" test -d "$r/d1"
sw="$(run_sweep "$r")"
expect_eq "proc: readable /proc → swept" "$(field "$sw" swept)" 1

# 11. 並行回収
r="$base/r-conc"
mkroot "$r"
mk_dead "$r" held
mkfifo "$base/hold-ready" "$base/hold-go"
exec {HR}<>"$base/hold-ready"
exec {HG}<>"$base/hold-go"
(
  exec {l}>>"$r/.sweep.lock"
  flock -n "$l" || exit 9
  echo held >&"$HR"
  read -r -t 60 -u "$HG" _
) &
echo $! >>"$PIDS"
holder=$!
read -r -t 60 -u "$HR" _ || bad "conc: holder not ready"
sw="$(run_sweep "$r")"
expect_eq "conc: sweep skipped while another holds .sweep.lock" "$(field "$sw" skipped)/$(field "$sw" swept)" 1/0
expect "conc: dir kept" test -d "$r/held"
echo go >&"$HG"
wait "$holder"
exec {HR}<&- {HG}<&-
sw="$(run_sweep "$r")"
expect_eq "conc: sweep after release" "$(field "$sw" swept)" 1
for i in $(seq 1 12); do mk_dead "$r" "c$i"; done
for i in 1 2 3 4 5 6; do run_sweep "$r" >"$base/conc-$i" & done
wait
total=0
for i in 1 2 3 4 5 6; do total=$((total + $(field "$(cat "$base/conc-$i")" swept))); done
expect_eq "conc: 12 dirs reclaimed exactly once across 6 sweepers" "$total" 12
expect_eq "conc: nothing left" "$(ls -A "$r" | grep -v '^.sweep.lock$' | wc -l)" 0

# 12. symlink・別 UID・root 外
r="$base/r-link"
mkroot "$r"
mkdir "$base/outside"; echo keep >"$base/outside/sentinel"
ln -s "$base/outside" "$r/linkdir"
mk_dead "$r" inner
ln -s "$base/outside" "$r/inner/t/escape"
ln -s "$base/outside/sentinel" "$r/filelink"
sw="$(run_sweep "$r")"
expect "symlink: link to outside kept" test -L "$r/linkdir"
expect "symlink: file link kept" test -L "$r/filelink"
expect_eq "symlink: counted as foreign" "$(field "$sw" foreign)" 2
expect_not "symlink: finished dir containing a link is removed" test -e "$r/inner"
expect "symlink: outside target untouched" test -f "$base/outside/sentinel"
r="$base/r-uid"
mkroot "$r"
mk_dead "$r" other
sw="$(TTL_UID=4242 run_sweep "$r")"
expect_eq "uid: other-UID entries untouched" "$(field "$sw" swept)/$(field "$sw" foreign)" 0/1
expect "uid: dir kept" test -d "$r/other"
# 走査 root 外: sweep は root 直下しか見ない
mkroot "$base/r-nested"
mk_dead "$base/r-nested" a
mkdir -p "$base/r-nested/a/t/deep"
mk_dead "$base/r-nested/a/t/deep" nested
run_sweep "$base/r-nested" >/dev/null
expect_not "scope: finished dir removed with its contents" test -e "$base/r-nested/a"
mkroot "$base/r-sib"
mk_dead "$base" siblingdir
run_sweep "$base/r-sib" >/dev/null
expect "scope: sibling of the root untouched" test -d "$base/siblingdir"

# 13. root の選択（D1・D2）
select_root() { bash -c '. "$1"; ttl_select_root 2>"$2"; echo "root=$TTL_ROOT"' _ "$LIB" "$base/sel.err"; }
uid="$(id -u)"
mkdir "$base/sc" "$base/fb"
out="$(env -u CELERIS_TEST_TMP_ROOT TTL_SCRATCH_PARENT="$base/sc" TTL_FALLBACK_PARENT="$base/fb" bash -c '. "$1"; ttl_select_root 2>/dev/null; echo "root=$TTL_ROOT"' _ "$LIB")"
expect_eq "select: scratch preferred" "$out" "root=$base/sc/tr-$uid"
expect_eq "select: scratch root is 0700" "$(stat -c %a "$base/sc/tr-$uid")" 700
out="$(env -u CELERIS_TEST_TMP_ROOT TTL_SCRATCH_PARENT="$base/nope" TTL_FALLBACK_PARENT="$base/fb" bash -c '. "$1"; ttl_select_root 2>"$2"; echo "root=$TTL_ROOT"' _ "$LIB" "$base/sel.err")"
expect_eq "select: falls back to /tmp-like parent" "$out" "root=$base/fb/celeris-tr-$uid"
expect "select: fallback is logged" grep -q 'fallback' "$base/sel.err"
out="$(CELERIS_TEST_TMP_ROOT="$base/explicit" TTL_SCRATCH_PARENT="$base/sc" TTL_FALLBACK_PARENT="$base/fb" bash -c '. "$1"; ttl_select_root 2>/dev/null; echo "root=$TTL_ROOT"' _ "$LIB")"
expect_eq "select: explicit root first" "$out" "root=$base/explicit"
# 予算: root + 9 byte <= 47（root <= 38 byte）
pad() { local n=$1 s=""; while [ "${#s}" -lt "$n" ]; do s="${s}p"; done; echo "$s"; }
r38="$base/$(pad $((38 - ${#base} - 1)))"
r39="$base/$(pad $((39 - ${#base} - 1)))"
out="$(CELERIS_TEST_TMP_ROOT="$r38" TTL_SCRATCH_PARENT="$base/nope" TTL_FALLBACK_PARENT="$base/nope" bash -c '. "$1"; ttl_select_root 2>/dev/null; echo "root=$TTL_ROOT"' _ "$LIB")"
expect_eq "budget: 38-byte root accepted (TMPDIR 47 bytes)" "$out" "root=$r38"
out="$(CELERIS_TEST_TMP_ROOT="$r39" TTL_SCRATCH_PARENT="$base/nope" TTL_FALLBACK_PARENT="$base/nope" bash -c '. "$1"; ttl_select_root 2>"$2"; echo "root=$TTL_ROOT"' _ "$LIB" "$base/sel.err")"
expect_eq "budget: 39-byte root rejected" "$out" "root="
expect "budget: reason logged" grep -q 'budget' "$base/sel.err"
expect_not "budget: rejected root not created" test -e "$r39"
# lstat 検査: symlink・0755・別 UID は使わず、作り直しも消しもしない
ln -s "$base/outside" "$base/rootlink"
chmod 755 "$base/outside"
out="$(CELERIS_TEST_TMP_ROOT="$base/rootlink" TTL_SCRATCH_PARENT="$base/nope" TTL_FALLBACK_PARENT="$base/fb" bash -c '. "$1"; ttl_select_root 2>/dev/null; echo "root=$TTL_ROOT"' _ "$LIB")"
expect_eq "root: symlink root skipped, next candidate used" "$out" "root=$base/fb/celeris-tr-$uid"
expect "root: symlink untouched" test -L "$base/rootlink"
mkdir -m 0755 "$base/open755"
out="$(CELERIS_TEST_TMP_ROOT="$base/open755" TTL_SCRATCH_PARENT="$base/nope" TTL_FALLBACK_PARENT="$base/fb" bash -c '. "$1"; ttl_select_root 2>"$2"; echo "root=$TTL_ROOT"' _ "$LIB" "$base/sel.err")"
expect_eq "root: mode 0755 root skipped" "$out" "root=$base/fb/celeris-tr-$uid"
expect_eq "root: mode left as is" "$(stat -c %a "$base/open755")" 755
out="$(CELERIS_TEST_TMP_ROOT="$base/explicit" TTL_UID=4242 TTL_SCRATCH_PARENT="$base/nope" TTL_FALLBACK_PARENT="$base/fb2" bash -c '. "$1"; ttl_select_root 2>"$2"; echo "root=$TTL_ROOT"' _ "$LIB" "$base/sel.err")"
expect_eq "root: root owned by another UID is not used" "$out" "root="
expect "root: foreign owner logged" grep -q 'owned by uid' "$base/sel.err"
expect "root: foreign root kept" test -d "$base/explicit"

# 14. 短い path での実 Unix socket の bind（sun_path 107 byte）
mkroot "$base/r-sock"
out="$(CELERIS_TEST_TMP_ROOT="$base/r-sock" bash -c '
  . "$1"
  ttl_install_traps
  ttl_init sock || exit 70
  [ "${#TMPDIR}" -le 47 ] || { echo "TMPDIR too long: ${#TMPDIR}"; exit 71; }
  n=$((107 - ${#TMPDIR} - 1))
  name=$(printf "s%.0s" $(seq 1 "$n"))
  python3 -I -c "
import socket, sys
s = socket.socket(socket.AF_UNIX); s.bind(sys.argv[1]); s.close()
try:
    t = socket.socket(socket.AF_UNIX); t.bind(sys.argv[1] + \"x\")
    sys.exit(3)
except OSError:
    pass
" "$TMPDIR/$name" || exit 72
  echo "bound ${#TMPDIR}"
' _ "$LIB" 2>"$base/sock.err")"
rc=$?
expect_eq "socket: 107-byte path binds under TMPDIR; 108 bytes fails" "$rc/${out%% *}" "0/bound"
expect_eq "socket: dir removed on exit" "$(ls -A "$base/r-sock" | grep -v '^.sweep.lock$' | wc -l)" 0

# ==== 15. 実 runner（test-affected.sh・test-parallel.sh）への適用 ====================================================
# 小さな git fixture に runner の写しと helper を置き、PATH の先頭の偽 cargo で動かす。実 cargo・全体試験は走らせない。
# 偽 cargo は cargo nextest list/run・cargo metadata・cargo test --doc に最小の応答を返し、nextest run の中で TMPDIR に
# dir と実 Unix socket を作り、FIFO で ready を通知して release まで止まれる（待ちは出来事待ち）。
fx="$base/fx"
fakebin="$base/fakebin"
mkdir -p "$fx/scripts/dev/lib" "$fx/crates/foo/src" "$fx/tools/nextest" "$fakebin"
cp "$here/scripts/dev/test-affected.sh" "$here/scripts/dev/test-parallel.sh" "$fx/scripts/dev/"
cp "$LIB" "$fx/scripts/dev/lib/"
cp "$here/tools/nextest/VERSION" "$fx/tools/nextest/VERSION"
printf '#!/bin/sh\nexit 0\n' >"$fx/scripts/dev/check-health.sh"
printf '[package]\nname = "foo"\n' >"$fx/crates/foo/Cargo.toml"
echo '// v1' >"$fx/crates/foo/src/lib.rs"
git -C "$fx" init -q -b main
git -C "$fx" -c user.name=t -c user.email=t@example.invalid add -A
git -C "$fx" -c user.name=t -c user.email=t@example.invalid commit -q -m fixture
echo '// v2' >"$fx/crates/foo/src/lib.rs" # test-affected.sh が foo を対象に選ぶ変更
PIN="$(tr -d ' \n' <"$fx/tools/nextest/VERSION")"
cat >"$fakebin/cargo" <<'FAKE'
#!/bin/sh
case "$1 $2" in
  "nextest --version") echo "cargo-nextest $FAKE_PIN"; exit 0 ;;
  "nextest list") [ -z "${FAKE_LIST_RC:-}" ] || exit "$FAKE_LIST_RC"; echo '{"rust-binaries":{}}'; exit 0 ;;
  "metadata "*)
    targets='[]'
    [ -z "${FAKE_META_BIN:-}" ] || targets='[{"name":"foo-cli","kind":["bin"]},{"name":"foo-it","kind":["test"]}]'
    printf '{"workspace_root":"%s","target_directory":"%s/target","workspace_members":["foo"],"packages":[{"id":"foo","name":"foo","manifest_path":"%s/crates/foo/Cargo.toml","dependencies":[],"targets":%s}]}\n' \
      "$FAKE_FX" "$FAKE_STATE" "$FAKE_FX" "$targets"
    exit 0 ;;
  "nextest run")
    echo "$TMPDIR" >"$FAKE_STATE/tmpdir"
    echo x >"$TMPDIR/fake-test-file"
    [ -z "${FAKE_LEFT:-}" ] || mkdir "$TMPDIR/left-dir"
    if [ -n "${FAKE_SOCK:-}" ]; then
      python3 -I -c '
import socket, sys
d = sys.argv[1]
name = "s" * (107 - len(d) - 1)
s = socket.socket(socket.AF_UNIX)
s.bind(d + "/" + name)
open(sys.argv[2], "w").write("bound %d\n" % len(d + "/" + name))
' "$TMPDIR" "$FAKE_STATE/sock" || exit 9
    fi
    if [ -n "${FAKE_HOLD:-}" ]; then
      echo "$$ $TMPDIR" >"$FAKE_READY"     # ready 通知（controller が読むまで書き込みは FIFO に残る）
      read -r _ <"$FAKE_RELEASE"            # release まで止まる
      if echo y >"$TMPDIR/after-release" 2>/dev/null; then echo ok; else echo fail; fi >"$FAKE_STATE/child_tmp"
    fi
    echo "Starting 1 test across 1 binary"
    if [ "${FAKE_RUN_RC:-0}" = 0 ]; then
      echo "Summary [   0.001s] 1 test run: 1 passed, 0 skipped"
    else
      echo "        FAIL [   0.001s] foo tests::broken"
      echo "Summary [   0.001s] 1 test run: 0 passed, 1 failed, 0 skipped"
    fi
    exit "${FAKE_RUN_RC:-0}" ;;
  "test --doc") echo "test result: ok. 0 passed; 0 failed; 0 ignored"; exit 0 ;;
esac
echo "fake cargo: unexpected args: $*" >&2
exit 97
FAKE
chmod +x "$fakebin/cargo"

# rr_launch <case> <runner> [VAR=value...]: runner を専用 session（pgid = sid = RPID）で起こす。OUT ROOT RPID を設定。
# python が SIGINT 等を既定に戻す（bash の非同期 command は INT を無視で継承するため）。
rr_launch() {
  local name=$1 runner=$2
  shift 2
  OUT="$base/o-$name"
  ROOT="$base/r-$name"
  mkdir -p "$OUT"
  (
    cd "$fx" || exit 99
    exec env PATH="$fakebin:$PATH" CELERIS_WU_BASE=main CELERIS_TEST_TMP_ROOT="$ROOT" TMPDIR="$OUT" \
      FAKE_PIN="$PIN" FAKE_FX="$fx" FAKE_STATE="$OUT" FAKE_READY="$OUT/ready" FAKE_RELEASE="$OUT/release" "$@" \
      python3 -I -c 'import os,signal,sys
os.setsid()
for s in (signal.SIGINT, signal.SIGQUIT, signal.SIGTERM, signal.SIGHUP, signal.SIGPIPE):
    signal.signal(s, signal.SIG_DFL)
os.execvp(sys.argv[1], sys.argv[1:])' bash "scripts/dev/$runner.sh"
  ) >"$OUT/out" 2>&1 </dev/null &
  RPID=$!
  echo "$RPID" >>"$PIDS"
}
# rr_hold <case> <runner> [VAR=value...]: FAKE_HOLD で起こし、偽 cargo の ready を待つ。RDIR（runner の dir）CPID を設定。
rr_hold() {
  local name=$1 line
  mkdir -p "$base/o-$name"
  mkfifo "$base/o-$name/ready" "$base/o-$name/release"
  exec {RFD}<>"$base/o-$name/ready"
  exec {XFD}<>"$base/o-$name/release"
  rr_launch "$@" FAKE_HOLD=1
  RDIR="" CPID=""
  if read -r -t 60 -u "$RFD" line; then
    CPID=${line%% *}
    RDIR="$(dirname "${line#* }")"
    echo "$CPID" >>"$PIDS"
  else
    bad "$name: fake cargo never became ready"
  fi
}
rr_close() { exec {RFD}<&- {XFD}<&-; }
rr_wait() { wait "$RPID"; RC=$?; }
rr_release() { echo go >&"$XFD"; }
# rr_run <case> <runner> [VAR=value...]: 止めずに最後まで走らせ、RC を設定
rr_run() { rr_launch "$@"; rr_wait; }
# rr_entries <root>: root の実行 dir（.sweep.lock を除く）の数
rr_entries() { ls -A "$1" 2>/dev/null | grep -vc '^\.sweep\.lock$'; }

for runner in test-affected test-parallel; do
  t=${runner#test-}
  t=${t:0:2}   # af / pa
  # (a) 通常終了: exit 0 で dir が消える
  rr_run "$t-ok" "$runner"
  expect_eq "$runner normal: exit 0" "$RC" 0
  expect_eq "$runner normal: run dir removed" "$(rr_entries "$ROOT")" 0
  expect "$runner normal: TMPDIR was under the short root" grep -q "^$ROOT/" "$OUT/tmpdir"
  # (b) 非 0 終了: nextest の exit code が runner の exit code になり、dir は消える
  rr_run "$t-fail" "$runner" FAKE_RUN_RC=100
  expect_eq "$runner nonzero: exit 100" "$RC" 100
  expect_eq "$runner nonzero: run dir removed" "$(rr_entries "$ROOT")" 0
  # (c) 捕捉できる signal: 子へ転送し、子の終了を待ってから dir を消し、128+N で終わる
  for sg in INT:2 TERM:15 HUP:1; do
    sn=${sg%%:*}; sv=${sg##*:}
    rr_hold "$t-$sn" "$runner"
    d_before="$RDIR"
    expect "$runner $sn: run dir exists before the signal" test -d "$d_before"
    kill -s "$sn" "$RPID"
    rr_wait
    expect_eq "$runner $sn: exit 128+N" "$RC" $((128 + sv))
    wait_gone "$CPID"; expect "$runner $sn: fake cargo got the forwarded signal and ended" test "$?" = 0
    expect_not "$runner $sn: run dir removed" test -e "$d_before"
    expect_eq "$runner $sn: nothing left in root" "$(rr_entries "$ROOT")" 0
    rr_close
  done
  # (d) 稼働中の別 runner の dir を保持する: A が子を走らせている間に B が起動・終了しても A の dir は消えない
  rr_hold "$t-live" "$runner"
  A_DIR="$RDIR"; A_RPID=$RPID; A_ROOT="$ROOT"; A_CPID=$CPID; A_OUT="$OUT"
  # B は A と同じ root で通常終了まで走る（B の起動時と終了時の回収が A を見る）
  OUT="$base/o-$t-liveB"; mkdir -p "$OUT"
  (cd "$fx" && exec env PATH="$fakebin:$PATH" CELERIS_WU_BASE=main CELERIS_TEST_TMP_ROOT="$A_ROOT" TMPDIR="$OUT" FAKE_PIN="$PIN" FAKE_FX="$fx" FAKE_STATE="$OUT" bash "scripts/dev/$runner.sh") >"$OUT/out" 2>&1 </dev/null
  expect_eq "$runner live: second runner exit 0" "$?" 0
  expect "$runner live: running runner's dir kept" test -d "$A_DIR/t"
  RPID=$A_RPID; OUT="$A_OUT"
  rr_release; rr_wait
  expect_eq "$runner live: first runner exit 0 after release" "$RC" 0
  expect_eq "$runner live: child saw its TMPDIR until the end" "$(cat "$A_OUT/child_tmp" 2>/dev/null)" ok
  expect_not "$runner live: dir removed at the end" test -e "$A_DIR"
  rr_close
  # (e) 親だけ KILL: dir は残る（SIGKILL 直後の削除は約束しない）。孤児の子が使う間は次の runner も消さず、
  #     子が終わった後の次回起動で回収される
  rr_hold "$t-kp" "$runner"
  K_DIR="$RDIR"; K_ROOT="$ROOT"; K_CPID=$CPID; K_OUT="$OUT"; K_RPID=$RPID
  kill -KILL "$RPID"; rr_wait
  expect_eq "$runner KILL parent: exit 137" "$RC" 137
  expect "$runner KILL parent: run dir left behind" test -d "$K_DIR/t"
  expect "$runner KILL parent: orphan child still alive" pid_alive "$K_CPID"
  OUT="$base/o-$t-kpB"; mkdir -p "$OUT"
  (cd "$fx" && exec env PATH="$fakebin:$PATH" CELERIS_WU_BASE=main CELERIS_TEST_TMP_ROOT="$K_ROOT" TMPDIR="$OUT" FAKE_PIN="$PIN" FAKE_FX="$fx" FAKE_STATE="$OUT" bash "scripts/dev/$runner.sh") >"$OUT/out" 2>&1 </dev/null
  expect "$runner KILL parent: dir used by the orphan child is kept by the next runner" test -d "$K_DIR/t"
  OUT="$K_OUT"; rr_release
  wait_gone "$K_CPID"
  expect_eq "$runner KILL parent: orphan child used its TMPDIR to the end" "$(cat "$K_OUT/child_tmp" 2>/dev/null)" ok
  rr_close
  OUT="$base/o-$t-kpC"; mkdir -p "$OUT"
  (cd "$fx" && exec env PATH="$fakebin:$PATH" CELERIS_WU_BASE=main CELERIS_TEST_TMP_ROOT="$K_ROOT" TMPDIR="$OUT" FAKE_PIN="$PIN" FAKE_FX="$fx" FAKE_STATE="$OUT" bash "scripts/dev/$runner.sh") >"$OUT/out" 2>&1 </dev/null
  expect_not "$runner KILL parent: collected by the next start after the child ended" test -e "$K_DIR"
  # (f) group KILL（runner の group と子の session の両方）: dir は残り、次回起動で回収される
  rr_hold "$t-kg" "$runner"
  G_DIR="$RDIR"; G_ROOT="$ROOT"; G_CPID=$CPID
  kill -KILL -- "-$RPID" "-$CPID" 2>/dev/null
  rr_wait
  expect_eq "$runner KILL group: exit 137" "$RC" 137
  wait_gone "$G_CPID"
  expect "$runner KILL group: run dir left behind" test -d "$G_DIR"
  OUT="$base/o-$t-kgB"; mkdir -p "$OUT"
  (cd "$fx" && exec env PATH="$fakebin:$PATH" CELERIS_WU_BASE=main CELERIS_TEST_TMP_ROOT="$G_ROOT" TMPDIR="$OUT" FAKE_PIN="$PIN" FAKE_FX="$fx" FAKE_STATE="$OUT" bash "scripts/dev/$runner.sh") >"$OUT/out" 2>&1 </dev/null
  expect_not "$runner KILL group: collected by the next start" test -e "$G_DIR"
  rr_close
  # (g) runner 内の短い path で実 Unix socket（sun_path 107 byte）が bind できる
  rr_run "$t-sock" "$runner" FAKE_SOCK=1
  expect_eq "$runner socket: exit 0" "$RC" 0
  expect_eq "$runner socket: bound at 107 bytes" "$(cat "$OUT/sock" 2>/dev/null)" "bound 107"
  expect "$runner socket: TMPDIR within the 47-byte budget" test "$(wc -c <"$OUT/tmpdir")" -le 48
done

# (h) test-parallel の失敗時の exit code・summary・leftovers 診断（偽 cargo だけ。全体試験は走らせない）
rr_run pa-prep test-parallel FAKE_LIST_RC=1
expect_eq "test-parallel prepare failure: exit 1" "$RC" 1
expect "test-parallel prepare failure: nextest run not started" grep -q 'cargo nextest run was not started' "$OUT/out"
expect "test-parallel prepare failure: summary prepare_exit 1" grep -q '"prepare_exit": 1' "$OUT/out"
expect_eq "test-parallel prepare failure: run dir removed" "$(rr_entries "$ROOT")" 0
rr_run pa-meta test-parallel FAKE_META_BIN=1
expect_eq "test-parallel missing CLI binary: exit 4" "$RC" 4
expect "test-parallel missing CLI binary: diagnostic" grep -q 'CLI binary missing or not executable: foo-cli' "$OUT/out"
expect "test-parallel missing CLI binary: summary prepare_exit 4" grep -q '"prepare_exit": 4' "$OUT/out"
rr_run pa-nfail test-parallel FAKE_RUN_RC=100
expect_eq "test-parallel nextest failure: exit 100" "$RC" 100
expect "test-parallel nextest failure: failed test name before the last line" grep -q '^test-parallel: failed: foo tests::broken$' "$OUT/out"
expect_eq "test-parallel nextest failure: last line" "$(tail -n 1 "$OUT/out")" "test-parallel: cargo nextest run failed (exit 100)"
expect "test-parallel nextest failure: summary nextest_exit 100" grep -q '"nextest_exit": 100' "$OUT/out"
rr_run pa-left test-parallel FAKE_LEFT=1
expect_eq "test-parallel leftovers: exit 0" "$RC" 0
expect "test-parallel leftovers: warning kept" grep -q 'test-parallel: warning: tests left 2 entries in TMPDIR (removed on exit)' "$OUT/out"
expect "test-parallel leftovers: summary tmp_leftovers 2" grep -q '"tmp_leftovers": 2' "$OUT/out"
expect_eq "test-parallel leftovers: dir removed after the warning" "$(rr_entries "$ROOT")" 0
expect_eq "test-parallel leftovers: last line ok" "$(tail -n 1 "$OUT/out")" "test-parallel: ok"
# 旧 mktemp -d /tmp/celeris-test-* と旧 trap は両 runner に無い
expect_not "old mktemp /tmp/celeris-test-* removed from test-affected.sh" grep -q 'celeris-test-affected\.' "$here/scripts/dev/test-affected.sh"
expect_not "old mktemp /tmp/celeris-test-* removed from test-parallel.sh" grep -q 'celeris-test-parallel\.\|cleanup_logdir' "$here/scripts/dev/test-parallel.sh"

# ---- 後始末の確認は EXIT の cleanup が行う（process・dir が残れば exit 1） -----------------------------------------
if [ "$fails" -ne 0 ]; then
  echo "runner-tmp-lifecycle: $fails of $checks checks FAILED" >&2
  exit 1
fi
echo "runner-tmp-lifecycle: ok（$checks checks）"
