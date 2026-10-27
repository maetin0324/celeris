#!/usr/bin/env bash
# promote.sh・rollback.sh の credentiald 切り替え（sd_switch_credentiald）を偽の systemctl で確かめる。
#   A 旧を止めてから新を起こす（順序）。reset-failed を挟む。
#   B 旧が active でなければ止めない。
#   C 新の unit が無ければ何もしない。
#   D 新が起動後に active でなければ失敗（1）。
#   E 旧の停止に失敗したら新を起こさず失敗。
#   F promote.sh（live・stop-start）と rollback.sh が呼んでいる。
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
SD="$(cd "$HERE/.." && pwd)"
FAIL=0
ok() { echo "ok: $*"; }
ng() { echo "FAIL: $*" >&2; FAIL=1; }

# shellcheck source=/dev/null
. "$SD/lib.sh"
SD_LOG_FILE=""
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/bin"
cat >"$WORK/bin/systemctl" <<'FAKE'
#!/usr/bin/env bash
echo "$*" >>"$FAKE_LOG"
[ "${1:-}" = --user ] && shift
cmd="${1:-}"; shift || true
quiet=""; [ "${1:-}" = --quiet ] && { quiet=1; shift; }
unit="${1:-}"
case "$cmd" in
  cat) [ "${FAKE_NO_NEW_UNIT:-0}" = 1 ] && exit 1; exit 0 ;;
  is-active)
    case "$unit" in
      *@old*) [ "${FAKE_OLD_ACTIVE:-1}" = 1 ] ;;
      *@new*) [ "${FAKE_NEW_ACTIVE:-1}" = 1 ] ;;
    esac ;;
  stop) [ "${FAKE_STOP_FAILS:-0}" != 1 ] ;;
  *) exit 0 ;;
esac
FAKE
chmod +x "$WORK/bin/systemctl"
export PATH="$WORK/bin:$PATH" FAKE_LOG="$WORK/log" SD_CREDENTIALD_SETTLE=0

run() { : >"$FAKE_LOG"; if (sd_switch_credentiald new "$@") 2>/dev/null; then echo 0; else echo 1; fi; }
line() { grep -n -- "$1" "$FAKE_LOG" | head -1 | cut -d: -f1; }

rc="$(run old)"
[ "$rc" = 0 ] && ok "A: success" || ng "A: rc=$rc"
s="$(line 'stop celeris-credentiald@old')"; r="$(line 'reset-failed celeris-credentiald@new')"; st="$(line 'start celeris-credentiald@new')"
if [ -n "$s" ] && [ -n "$r" ] && [ -n "$st" ] && [ "$s" -lt "$r" ] && [ "$r" -lt "$st" ]; then ok "A: stop old < reset-failed < start new"; else ng "A: order s=$s r=$r st=$st"; fi

FAKE_OLD_ACTIVE=0 rc="$(FAKE_OLD_ACTIVE=0 run old)"
if [ "$rc" = 0 ] && ! grep -q 'stop celeris-credentiald@old' "$FAKE_LOG" && grep -q 'start celeris-credentiald@new' "$FAKE_LOG"; then ok "B: inactive old is not stopped"; else ng "B"; fi

rc="$(FAKE_NO_NEW_UNIT=1 run old)"
if [ "$rc" = 0 ] && ! grep -q 'start\|stop' "$FAKE_LOG"; then ok "C: no unit -> nothing"; else ng "C"; fi

rc="$(FAKE_NEW_ACTIVE=0 run old)"
[ "$rc" = 1 ] && ok "D: new not active -> fail" || ng "D: rc=$rc"

rc="$(FAKE_STOP_FAILS=1 run old)"
if [ "$rc" = 1 ] && ! grep -q 'start celeris-credentiald@new' "$FAKE_LOG"; then ok "E: stop failure -> no start, fail"; else ng "E: rc=$rc"; fi

n="$(grep -c '^  switch_credentiald$' "$SD/promote.sh")"
[ "$n" = 2 ] && ok "F: promote.sh calls it in both modes" || ng "F: promote.sh calls=$n"
grep -q 'sd_switch_credentiald "\$PREV" "\$CUR"' "$SD/rollback.sh" && ok "F: rollback.sh calls it" || ng "F: rollback.sh"

exit "$FAIL"
