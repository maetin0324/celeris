# shellcheck shell=bash
# host 共通の重い検査の枠（ADR 2026-11-12-host-check-slots）。daemon の task_worker::check_slots と同じ file を取り合う。
#   . scripts/dev/lib/check-slot.sh
#   check_slot_acquire <label>   空いた枠を取るまで待つ。待つなら理由を stderr と waiting/ に 1 回だけ書く。
#                                取った枠は process が終われば kernel が外す（fd を閉じれば外れる）。
#                                CELERIS_CHECK_SLOT が既にあれば（daemon が取って渡した）取り直さない。
#   取った後は CHECK_SLOT_TEST_THREADS（= max(1, core 数 / 枠の数)）を nextest の --test-threads に渡す。
# 枠の dir: ${CELERIS_CHECK_SLOTS_DIR:-$HOME/.cache/celeris/check-slots}、数: ${CELERIS_CHECK_SLOTS:-max(1, core 数 / 12)}。

check_slot_count() {
  local cores n
  cores="$(nproc 2>/dev/null || echo 1)"
  n="${CELERIS_CHECK_SLOTS:-}"
  case "$n" in
    ''|*[!0-9]*|0) n=$((cores / 12)) ;;
  esac
  [ "$n" -ge 1 ] || n=1
  echo "$n"
}

check_slot_acquire() {
  local label="${1:-check}" dir n cores i waited=0 mark="" holders reason
  n="$(check_slot_count)"
  cores="$(nproc 2>/dev/null || echo 1)"
  CHECK_SLOT_TEST_THREADS=$((cores / n))
  [ "$CHECK_SLOT_TEST_THREADS" -ge 1 ] || CHECK_SLOT_TEST_THREADS=1
  export CHECK_SLOT_TEST_THREADS
  if [ -n "${CELERIS_CHECK_SLOT:-}" ]; then
    echo "check-slot: 枠は呼び出し元が取った（$CELERIS_CHECK_SLOT）。--test-threads $CHECK_SLOT_TEST_THREADS" >&2
    return 0
  fi
  if ! command -v flock >/dev/null 2>&1; then
    echo "check-slot: flock が無いので枠を取れない" >&2
    return 2
  fi
  dir="${CELERIS_CHECK_SLOTS_DIR:-${HOME:?}/.cache/celeris/check-slots}"
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
