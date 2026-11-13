# shellcheck shell=bash
# host 共通の重い検査の枠（ADR 2026-11-12-host-check-slots）。daemon の task_worker::check_slots と同じ file を取り合う。
#   . scripts/dev/lib/check-slot.sh
#   check_slot_acquire <label>   空いた枠を取るまで待つ。待つなら理由を stderr と waiting/ に 1 回だけ書く。
#                                取った枠は process が終われば kernel が外す（fd を閉じれば外れる）。
#                                CELERIS_CHECK_SLOT が既にあれば（daemon が取って渡した）取り直さない。
#   取った後は CHECK_SLOT_TEST_THREADS（= check_slot_test_threads）を nextest の --test-threads に渡す。
#   子を起こす呼び出しには `{CHECK_SLOT_FD}>&-` を付け、lock の fd を子孫に継がせない（bash の {var}<> は CLOEXEC を
#   付けない。継ぐと、試験の孤児〈2026-11-10 の release gate の sleep 3600 など〉が script の終了後も枠を握る）。
#   関数の呼び出しに付けた close は関数の間だけ効き、shell 自身は lock を持ち続ける。CHECK_SLOT_FD は常に設定する
#   （枠を取らなかったときは /dev/null。未設定だと `{CHECK_SLOT_FD}>&-` が ambiguous redirect になる）。
# 枠の dir・数・並列度は下の 3 関数が決める。Rust の task_worker::check_slots（slots_dir・slot_count・test_threads）と
# 同じ規則で、check_slots の試験が bash にこの file を読ませて一致を確かめる。

# D1: 枠の dir。CELERIS_CHECK_SLOTS_DIR（host の local の dir）か $XDG_RUNTIME_DIR/celeris/check-slots。
# どちらも無ければ理由を stderr に出して 2 を返す（別の dir に落ちない）。
check_slot_dir() {
  if [ -n "${CELERIS_CHECK_SLOTS_DIR:-}" ]; then
    printf '%s\n' "$CELERIS_CHECK_SLOTS_DIR"
  elif [ -n "${XDG_RUNTIME_DIR:-}" ]; then
    printf '%s\n' "$XDG_RUNTIME_DIR/celeris/check-slots"
  else
    echo "check-slot: CELERIS_CHECK_SLOTS_DIR も XDG_RUNTIME_DIR も無いので、host の枠の dir を決められない" >&2
    return 2
  fi
}

# D2: 枠の数。CELERIS_CHECK_SLOTS が 10 進の数字だけで 1 以上ならそれ、ほかは max(1, core 数 / 12)。
check_slot_count() {
  local cores n
  cores="$(nproc 2>/dev/null || echo 1)"
  n="${CELERIS_CHECK_SLOTS:-}"
  case "$n" in
    ''|*[!0-9]*) n=0 ;;
    *) n=$((10#$n)) ;;
  esac
  if [ "$n" -lt 1 ]; then
    n=$((cores / 12))
    [ "$n" -ge 1 ] || n=1
  fi
  echo "$n"
}

# D2: 枠を取った試験の nextest の並列度。max(1, core 数 / 枠の数)。
check_slot_test_threads() {
  local cores n t
  cores="$(nproc 2>/dev/null || echo 1)"
  n="$(check_slot_count)"
  t=$((cores / n))
  [ "$t" -ge 1 ] || t=1
  echo "$t"
}

check_slot_acquire() {
  local label="${1:-check}" dir n i waited=0 mark="" holders reason
  n="$(check_slot_count)"
  CHECK_SLOT_TEST_THREADS="$(check_slot_test_threads)"
  export CHECK_SLOT_TEST_THREADS
  if [ -n "${CELERIS_CHECK_SLOT:-}" ]; then
    exec {CHECK_SLOT_FD}</dev/null
    echo "check-slot: 枠は呼び出し元が取った（$CELERIS_CHECK_SLOT）。--test-threads $CHECK_SLOT_TEST_THREADS" >&2
    return 0
  fi
  if ! command -v flock >/dev/null 2>&1; then
    echo "check-slot: flock が無いので枠を取れない" >&2
    return 2
  fi
  dir="$(check_slot_dir)" || return 2
  mkdir -p "$dir/waiting" || return 2
  while :; do
    i=0
    while [ "$i" -lt "$n" ]; do
      exec {CHECK_SLOT_FD}<>"$dir/slot-$i.lock" || return 2
      if flock -n "$CHECK_SLOT_FD"; then
        printf 'pid=%s\nlabel=%s\nsince=%s\n' "$$" "$label" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >"$dir/slot-$i.holder"
        CELERIS_CHECK_SLOT="$dir/slot-$i.lock"
        export CELERIS_CHECK_SLOT
        [ -z "$mark" ] || rm -f "$mark"
        if [ "$waited" -eq 1 ]; then
          echo "check-slot: 枠 $i を取った（$label）" >&2
        fi
        echo "check-slot: 枠 $i/$n を使う。--test-threads $CHECK_SLOT_TEST_THREADS" >&2
        return 0
      fi
      exec {CHECK_SLOT_FD}>&-
      i=$((i + 1))
    done
    if [ "$waited" -eq 0 ]; then
      waited=1
      holders="$(cat "$dir"/slot-*.holder 2>/dev/null | sed -n 's/^label=//p' | paste -sd '、' -)"
      reason="host の検査の枠 $n 本がすべて使用中（${holders:-使用中の検査は不明}）"
      mark="$dir/waiting/$$-sh"
      printf 'pid=%s\nlabel=%s\nsince=%s\nreason=%s\n' "$$" "$label" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$reason" >"$mark"
      echo "check-slot: 枠を待つ（理由: $reason）" >&2
    fi
    sleep 10
  done
}
