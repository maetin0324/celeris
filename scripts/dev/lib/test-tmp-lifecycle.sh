# shellcheck shell=bash
# scripts/dev/lib/test-tmp-lifecycle.sh — 試験 runner の一時領域の寿命管理（source して使う 1 file、bash 専用）。
# 採用設計: agent-docs/adr/2026-11-06-test-runner-tmp-lifecycle.md の D1〜D4。この file は set -e/-u/-o pipefail を
# 変えない。関数は ttl_ （公開）と ttl__ （内部）で始まる。
#
# 公開関数:
#   ttl_init <runner名>   root を選び（D1）、終わった実行の dir を回収し（D3）、自分の実行 dir を作る（D2）。
#                         TTL_ROOT・TTL_DIR・TTL_TMPDIR・TTL_LOCK_FD を設定し、TMPDIR を export する。
#   ttl_sweep [root]      終わった実行の dir を回収する（D3）。結果は TTL_N_SWEPT/ALIVE/FOREIGN/UNKNOWN と TTL_SWEEP_SKIPPED。
#   ttl_run <cmd...>      cmd を setsid の別 session で起こし（stdin は引き継ぐ）、終了を待つ。rc は TTL_CHILD_RC と戻り値。
#   ttl_install_traps     TERM・INT・HUP（D4）と EXIT（後始末）の trap を置く。
#   ttl_finish            EXIT の後始末。子の session が空で、lock を他に持つ process が無いときだけ dir を消す。
#
# 環境変数（試験・運用で注入できる。既定は括弧内）:
#   CELERIS_TEST_TMP_ROOT  明示 root（候補 1）
#   TTL_SCRATCH_PARENT     候補 2 の親（/local/celeris/data/scratch）。root は <親>/tr-<uid>
#   TTL_FALLBACK_PARENT    候補 3 の親（/tmp）。root は <親>/celeris-tr-<uid>
#   TTL_BUDGET             TMPDIR の長さ上限 byte（47）。TMPDIR は <root>/XXXXXX/t（root + 9 byte）
#   TTL_SIGNAL_WAIT        signal 後に子の終わりを待つ秒数（30、整数）
#   TTL_NOW                現在時刻の epoch 秒（date +%s）
#   TTL_UID                「自分の UID」（id -u）。別 UID の fixture に使う
#   TTL_PROC               /proc の場所（/proc）。読めない場合の「残す」判定の試験に使う
#
# 判定の補足（ADR との差）:
#   - 子の session は ttl_run が <dir>/sids に記録する。D3 の 3 は owner の sid と sids の全部を走査する。
#     ただし回収側自身の session は除く（同じ端末から起こした別の実行を永久に保護しないため。その実行の保護は lock が担う）。
#   - owner の boot_id が今と違うときは、旧 boot の process は存在しないので D3 の 3 を省く（sid の数が偶然一致するため）。
#   - bash は job control 無しの非同期 command の SIGINT を無視にする。INT を転送するとき、子が INT を無視していれば TERM を送る。

: "${TTL_BUDGET:=47}"
TTL_ROOT=""
TTL_DIR=""
TTL_TMPDIR=""
TTL_LOCK_FD=""
TTL_CHILD=""
TTL_CHILD_RC=""
TTL_N_SWEPT=0
TTL_N_ALIVE=0
TTL_N_FOREIGN=0
TTL_N_UNKNOWN=0
TTL_SWEEP_SKIPPED=0

ttl__uid() { printf '%s' "${TTL_UID:-$(id -u)}"; }
ttl__now() { printf '%s' "${TTL_NOW:-$(date +%s)}"; }
ttl__proc() { printf '%s' "${TTL_PROC:-/proc}"; }
ttl__log() { printf 'test-tmp-lifecycle: %s\n' "$*" >&2; }

# --- /proc の読み取り ------------------------------------------------------------------------------------------
# ttl__pstat <pid> → TTL_P_SID, TTL_P_START。戻り値 0=読めた 1=process が無い 2=判定できない
ttl__pstat() {
  local proc f s r
  proc="$(ttl__proc)"
  f="$proc/$1/stat"
  if ! { IFS= read -r s <"$f"; } 2>/dev/null; then
    [ -r "$proc/self/stat" ] || return 2
    [ -e "$proc/$1" ] && return 2
    return 1
  fi
  r=${s##*) }
  # shellcheck disable=SC2086
  set -- $r
  [ "$#" -ge 20 ] || return 2
  TTL_P_SID=$4
  TTL_P_START=${20}
  return 0
}

# ttl__sessions_scan <alive|signal> <sig> <sid...>: sid の session に属する process を探す。
# alive: 戻り値 0=居る 1=居ない 2=判定できない。signal: 見つけた全員に <sig> を送る。
ttl__sessions_scan() {
  local mode=$1 sig=$2 proc f s r p any=0 found=1
  shift 2
  [ "$#" -gt 0 ] || return 1
  local want=" $* "
  proc="$(ttl__proc)"
  for f in "$proc"/[0-9]*/stat; do
    [ -e "$f" ] || continue
    any=1
    { IFS= read -r s <"$f"; } 2>/dev/null || continue
    r=${s##*) }
    # shellcheck disable=SC2086
    set -- $r
    [ "$#" -ge 4 ] || return 2
    # 終わって wait されていない zombie は数えない（init が回収しない環境でも回収が止まらないように）
    [ "$1" = Z ] && continue
    case "$want" in
      *" $4 "*)
        if [ "$mode" = alive ]; then return 0; fi
        p=${f%/stat}
        p=${p##*/}
        kill -s "$sig" "$p" 2>/dev/null || true
        found=0
        ;;
    esac
  done
  [ "$mode" = alive ] || return "$found"
  [ "$any" = 1 ] || return 2
  return 1
}
ttl__sid_alive() { ttl__sessions_scan alive "" "$@"; }

ttl__own_sid() {
  ttl__pstat "$$" || return 1
  printf '%s' "$TTL_P_SID"
}

# --- D1: root の選択 --------------------------------------------------------------------------------------------
# ttl__root_ok <root>: lstat 検査。実 dir・自分の UID・0700 のときだけ 0。理由は stderr。
ttl__root_ok() {
  local r=$1 info u m f
  info="$(stat -c '%u %a %F' -- "$r" 2>/dev/null)" || { ttl__log "root $r: stat failed"; return 1; }
  u=${info%% *}
  info=${info#* }
  m=${info%% *}
  f=${info#* }
  if [ "$f" != directory ]; then ttl__log "root $r: not a real directory ($f); not used"; return 1; fi
  if [ "$u" != "$(ttl__uid)" ]; then ttl__log "root $r: owned by uid $u; not used"; return 1; fi
  if [ "$m" != 700 ]; then ttl__log "root $r: mode $m (need 700); not used"; return 1; fi
  return 0
}

# ttl__candidate <root>: 予算・存在・所有者を確かめ、無ければ 0700 で作る。root の作り直し・削除はしない。
ttl__candidate() {
  local r=$1 parent
  case "$r" in /*) ;; *) ttl__log "root $r: not an absolute path; not used"; return 1 ;; esac
  # TMPDIR は <root>/XXXXXX/t = root + 9 byte
  if [ $((${#r} + 9)) -gt "$TTL_BUDGET" ]; then
    ttl__log "root $r: TMPDIR would be $((${#r} + 9)) bytes (budget $TTL_BUDGET); not used"
    return 1
  fi
  if [ ! -e "$r" ] && [ ! -L "$r" ]; then
    parent="${r%/*}"
    [ -n "$parent" ] || parent=/
    if [ -d "$parent" ] && [ ! -L "$parent" ] && [ -w "$parent" ]; then
      mkdir -m 0700 -- "$r" 2>/dev/null || true
    else
      return 1
    fi
  fi
  ttl__root_ok "$r"
}

# ttl_select_root: TTL_ROOT を設定する。候補を上から順に試す。
ttl_select_root() {
  local uid scratch fallback
  uid="$(ttl__uid)"
  scratch="${TTL_SCRATCH_PARENT:-/local/celeris/data/scratch}"
  fallback="${TTL_FALLBACK_PARENT:-/tmp}"
  TTL_ROOT=""
  if [ -n "${CELERIS_TEST_TMP_ROOT:-}" ] && ttl__candidate "$CELERIS_TEST_TMP_ROOT"; then
    TTL_ROOT="$CELERIS_TEST_TMP_ROOT"
    return 0
  fi
  if [ -d "$scratch" ] && [ -w "$scratch" ] && ttl__candidate "$scratch/tr-$uid"; then
    TTL_ROOT="$scratch/tr-$uid"
    return 0
  fi
  if ttl__candidate "$fallback/celeris-tr-$uid"; then
    ttl__log "using fallback root $fallback/celeris-tr-$uid"
    TTL_ROOT="$fallback/celeris-tr-$uid"
    return 0
  fi
  ttl__log "no usable root for the test temporary directory"
  return 1
}

# --- D3: 回収 ---------------------------------------------------------------------------------------------------
# ttl__trash_rm <root> <dir>: dir を root 内の .trash-XXXXXX へ rename してから消す。
ttl__trash_rm() {
  local root=$1 d=$2 t
  t="$(mktemp -d -p "$root" .trash-XXXXXX 2>/dev/null)" || return 1
  if ! mv -T -- "$d" "$t" 2>/dev/null; then
    rmdir "$t" 2>/dev/null || true
    return 1
  fi
  chmod -R u+w "$t" 2>/dev/null || true
  rm -rf --one-file-system -- "$t"
}

# ttl__read_owner <file>: O_uid O_pid O_start O_sid O_boot を設定。欠け・形式違いは 1。
ttl__read_owner() {
  local k v
  O_uid="" O_pid="" O_start="" O_sid="" O_boot=""
  { [ -f "$1" ] && [ ! -L "$1" ]; } || return 1
  while IFS='=' read -r k v; do
    case "$k" in
      uid) O_uid=$v ;; pid) O_pid=$v ;; starttime) O_start=$v ;; sid) O_sid=$v ;; boot_id) O_boot=$v ;;
    esac
  done <"$1"
  case "$O_uid$O_pid$O_start$O_sid" in *[!0-9]*) return 1 ;; esac
  [ -n "$O_uid" ] && [ -n "$O_pid" ] && [ -n "$O_start" ] && [ -n "$O_sid" ] && [ -n "$O_boot" ]
}

# ttl__reap_dir <root> <dir>: 実行 dir 1 つを判定して回収する。判定できなければ残す。
ttl__reap_dir() {
  local root=$1 d=$2 lfd boot own sids line rc
  if ! ttl__read_owner "$d/owner"; then
    TTL_N_UNKNOWN=$((TTL_N_UNKNOWN + 1))
    return 0
  fi
  # 条件 1: lock を取れる
  if ! { [ -f "$d/lock" ] && [ ! -L "$d/lock" ]; }; then
    TTL_N_UNKNOWN=$((TTL_N_UNKNOWN + 1))
    return 0
  fi
  { exec {lfd}<"$d/lock"; } 2>/dev/null || { TTL_N_UNKNOWN=$((TTL_N_UNKNOWN + 1)); return 0; }
  if ! flock -n "$lfd"; then
    exec {lfd}<&-
    TTL_N_ALIVE=$((TTL_N_ALIVE + 1))
    return 0
  fi
  # 条件 2: boot_id が違う、または pid が無い・starttime が違う
  local boot_diff=0
  if ! { IFS= read -r boot <"$(ttl__proc)/sys/kernel/random/boot_id"; } 2>/dev/null; then
    exec {lfd}<&-
    TTL_N_UNKNOWN=$((TTL_N_UNKNOWN + 1))
    return 0
  fi
  if [ "$boot" != "$O_boot" ]; then
    boot_diff=1
  else
    rc=0
    ttl__pstat "$O_pid" || rc=$?
    if [ "$rc" = 2 ]; then
      exec {lfd}<&-
      TTL_N_UNKNOWN=$((TTL_N_UNKNOWN + 1))
      return 0
    fi
    if [ "$rc" = 0 ] && [ "$TTL_P_START" = "$O_start" ]; then
      exec {lfd}<&-
      TTL_N_ALIVE=$((TTL_N_ALIVE + 1))
      return 0
    fi
  fi
  # 条件 3: owner と sids の session に process が居ない
  if [ "$boot_diff" = 0 ]; then
    own="$(ttl__own_sid)" || own=""
    sids=""
    [ "$O_sid" = "$own" ] || sids="$O_sid"
    if [ -f "$d/sids" ] && [ ! -L "$d/sids" ]; then
      while IFS='=' read -r line rc; do
        case "$line" in sid) case "$rc" in '' | *[!0-9]*) ;; *) [ "$rc" = "$own" ] || sids="$sids $rc" ;; esac ;; esac
      done <"$d/sids"
    fi
    rc=0
    # shellcheck disable=SC2086
    ttl__sid_alive $sids || rc=$?
    if [ "$rc" != 1 ]; then
      exec {lfd}<&-
      if [ "$rc" = 0 ]; then TTL_N_ALIVE=$((TTL_N_ALIVE + 1)); else TTL_N_UNKNOWN=$((TTL_N_UNKNOWN + 1)); fi
      return 0
    fi
  fi
  # lock を持ったまま rename し、その後で消す
  if ttl__trash_rm "$root" "$d"; then
    TTL_N_SWEPT=$((TTL_N_SWEPT + 1))
  else
    TTL_N_UNKNOWN=$((TTL_N_UNKNOWN + 1))
  fi
  exec {lfd}<&-
  return 0
}

ttl_sweep() {
  local root=${1:-$TTL_ROOT} slfd d name uid info u f now mtime entries
  TTL_N_SWEPT=0 TTL_N_ALIVE=0 TTL_N_FOREIGN=0 TTL_N_UNKNOWN=0 TTL_SWEEP_SKIPPED=0
  { [ -n "$root" ] && [ -d "$root" ] && [ ! -L "$root" ]; } || return 0
  uid="$(ttl__uid)"
  { exec {slfd}>>"$root/.sweep.lock"; } 2>/dev/null || return 0
  if ! flock -n "$slfd"; then
    exec {slfd}>&-
    TTL_SWEEP_SKIPPED=1
    return 0
  fi
  now="$(ttl__now)"
  for d in "$root"/* "$root"/.[!.]*; do
    { [ -e "$d" ] || [ -L "$d" ]; } || continue
    name=${d##*/}
    [ "$name" = .sweep.lock ] && continue
    info="$(stat -c '%u %a %F' -- "$d" 2>/dev/null)" || continue
    u=${info%% *}
    f=${info#* * }
    # 実 dir で自分の UID の物だけを対象にする。それ以外は数えるだけで触らない。
    if [ "$f" != directory ] || [ "$u" != "$uid" ]; then
      TTL_N_FOREIGN=$((TTL_N_FOREIGN + 1))
      continue
    fi
    case "$name" in
      .trash-*)
        ttl__trash_rm "$root" "$d" && TTL_N_SWEPT=$((TTL_N_SWEPT + 1))
        ;;
      .init-*)
        if [ -e "$d/owner" ] || [ -L "$d/owner" ]; then
          TTL_N_ALIVE=$((TTL_N_ALIVE + 1))
          continue
        fi
        mtime="$(stat -c %Y -- "$d" 2>/dev/null)" || continue
        entries="$(ls -A -- "$d" 2>/dev/null)" || continue
        if [ $((now - mtime)) -ge 3600 ] && { [ -z "$entries" ] || [ "$entries" = owner.tmp ]; }; then
          if ttl__trash_rm "$root" "$d"; then TTL_N_SWEPT=$((TTL_N_SWEPT + 1)); else TTL_N_UNKNOWN=$((TTL_N_UNKNOWN + 1)); fi
        else
          TTL_N_UNKNOWN=$((TTL_N_UNKNOWN + 1))
        fi
        ;;
      .*) TTL_N_FOREIGN=$((TTL_N_FOREIGN + 1)) ;;
      *) ttl__reap_dir "$root" "$d" ;;
    esac
  done
  exec {slfd}>&-
  if [ $((TTL_N_SWEPT + TTL_N_FOREIGN + TTL_N_UNKNOWN)) -gt 0 ]; then
    ttl__log "sweep $root: removed $TTL_N_SWEPT, in use $TTL_N_ALIVE, left alone (foreign/unknown) $((TTL_N_FOREIGN + TTL_N_UNKNOWN))"
  fi
  return 0
}

# --- D2: 作成 ---------------------------------------------------------------------------------------------------
ttl_init() {
  local runner=${1:-runner} init id boot start sid
  ttl_select_root || return 1
  ttl_sweep "$TTL_ROOT"
  init="$(mktemp -d -p "$TTL_ROOT" .init-XXXXXX)" || { ttl__log "cannot create an init dir under $TTL_ROOT"; return 1; }
  id=${init##*/.init-}
  boot=""
  { IFS= read -r boot <"$(ttl__proc)/sys/kernel/random/boot_id"; } 2>/dev/null || true
  start="" sid=""
  if ttl__pstat "$$"; then start=$TTL_P_START sid=$TTL_P_SID; fi
  {
    printf 'uid=%s\npid=%s\nstarttime=%s\nsid=%s\nboot_id=%s\nrunner=%s\ncreated=%s\n' \
      "$(ttl__uid)" "$$" "$start" "$sid" "$boot" "$runner" "$(ttl__now)"
  } >"$init/owner.tmp" && mv -f -- "$init/owner.tmp" "$init/owner" || return 1
  mkdir -m 0700 -- "$init/t" || return 1
  mkfifo -m 0600 -- "$init/tick" || return 1
  exec {TTL_LOCK_FD}>"$init/lock" || return 1
  flock -n "$TTL_LOCK_FD" || return 1
  if [ -e "$TTL_ROOT/$id" ] || [ -L "$TTL_ROOT/$id" ]; then
    ttl__log "$TTL_ROOT/$id already exists"
    return 1
  fi
  mv -T -- "$init" "$TTL_ROOT/$id" || return 1
  TTL_DIR="$TTL_ROOT/$id"
  TTL_TMPDIR="$TTL_DIR/t"
  export TMPDIR="$TTL_TMPDIR"
  return 0
}

# --- D4: 子の起動・signal・後始末 -----------------------------------------------------------------------------------
ttl_run() {
  TTL_CHILD_RC=""
  setsid "$@" <&0 &
  TTL_CHILD=$!
  # setsid は fork せず exec するので、$! が新しい session の id になる
  [ -n "$TTL_DIR" ] && printf 'sid=%s\n' "$TTL_CHILD" >>"$TTL_DIR/sids"
  local rc=0
  wait "$TTL_CHILD" || rc=$?
  TTL_CHILD_RC=$rc
  return "$rc"
}

# ttl__wait_session_gone <sid> <seconds>: session が空になるか期限まで待つ。出来事待ち（tick fifo の read -t）。
ttl__wait_session_gone() {
  local sid=$1 limit=$2 deadline tfd rc
  deadline=$((SECONDS + limit))
  { exec {tfd}<>"$TTL_DIR/tick"; } 2>/dev/null || tfd=""
  while :; do
    rc=0
    ttl__sid_alive "$sid" || rc=$?
    [ "$rc" = 0 ] || break
    [ "$SECONDS" -lt "$deadline" ] || break
    if [ -n "$tfd" ]; then read -r -t 0.05 -u "$tfd" _ || true; else read -r -t 0.05 _ </dev/null || true; fi
  done
  [ -z "$tfd" ] || exec {tfd}<&-
  return "$rc"
}

# ttl__on_signal <名前> <番号>: 子の session へ転送し、最大 TTL_SIGNAL_WAIT 秒待ち、128+番号で終わる。
ttl__on_signal() {
  local name=$1 num=$2 sig=$1 ign
  trap '' INT TERM HUP
  if [ -n "$TTL_CHILD" ]; then
    if [ "$name" = INT ] && [ -r "$(ttl__proc)/$TTL_CHILD/status" ]; then
      ign="$(sed -n 's/^SigIgn:[[:space:]]*//p' "$(ttl__proc)/$TTL_CHILD/status" 2>/dev/null)"
      # SIGINT = 2 → bit 1
      if [ -n "$ign" ] && [ $(((0x$ign >> 1) & 1)) = 1 ]; then sig=TERM; fi
    fi
    ttl__sessions_scan signal "$sig" "$TTL_CHILD" || true
    ttl__wait_session_gone "$TTL_CHILD" "${TTL_SIGNAL_WAIT:-30}" || true
  fi
  exit $((128 + num))
}

# ttl_finish: 後始末。子の session が空で、lock を他の process が持っていないときだけ消す（D3 と同じ保護）。
ttl_finish() {
  local rc=$? sids line v rc2 lfd
  trap '' INT TERM HUP
  if [ -n "$TTL_DIR" ] && [ -d "$TTL_DIR" ]; then
    sids=""
    if [ -f "$TTL_DIR/sids" ]; then
      while IFS='=' read -r line v; do
        [ "$line" = sid ] && sids="$sids $v"
      done <"$TTL_DIR/sids"
    fi
    rc2=0
    # shellcheck disable=SC2086
    ttl__sid_alive $sids || rc2=$?
    if [ "$rc2" = 1 ]; then
      [ -z "$TTL_LOCK_FD" ] || exec {TTL_LOCK_FD}>&-
      TTL_LOCK_FD=""
      if { exec {lfd}<"$TTL_DIR/lock"; } 2>/dev/null && flock -n "$lfd"; then
        ttl__trash_rm "$TTL_ROOT" "$TTL_DIR" || ttl__log "could not remove $TTL_DIR"
      else
        ttl__log "kept $TTL_DIR (still referenced)"
      fi
      { exec {lfd}<&-; } 2>/dev/null || true
    else
      ttl__log "kept $TTL_DIR (child still running or unknown)"
    fi
  fi
  [ -z "$TTL_ROOT" ] || ttl_sweep "$TTL_ROOT"
  return "$rc"
}

ttl_install_traps() {
  trap 'ttl__on_signal INT 2' INT
  trap 'ttl__on_signal TERM 15' TERM
  trap 'ttl__on_signal HUP 1' HUP
  trap 'ttl_finish' EXIT
}
