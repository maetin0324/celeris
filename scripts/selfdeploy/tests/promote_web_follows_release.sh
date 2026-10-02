#!/usr/bin/env bash
# ADR-0081 / web ADR-W3 付記 2026-10-02 (C)(D): promote.sh が昇格後に呼ぶ web-follow.sh は、旧 web が動いているときだけ
# 新 release の web へ追従させる。
#   (a) 旧が active・新が web.ok=true → start/enable celeris-web@new と stop/disable celeris-web@old が記録される
#   (b) web.ok=false → web の操作なし
#   (c) 旧が inactive → 操作なし
#   (d) どの場合も celeris@ への呼び出しが無い。どの場合も exit 0
#   (e) 新の起動に失敗 → 新を stop して旧を start し直す
# 偽の systemctl（呼び出しを記録。is-active は FAKE_ACTIVE で制御）と一時 dir の releases だけを使う。
# 実 systemctl・~/.local・~/.config には触れない。
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT
mkdir -p "$root/bin" "$root/config" "$root/state/releases"

fail() {
  echo "FAIL: $*" >&2
  [ -f "$root/calls.log" ] && cat "$root/calls.log" >&2
  [ -f "$root/out.log" ] && tail -n 20 "$root/out.log" >&2
  exit 1
}

cat >"$root/bin/systemctl" <<'EOF2'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$FAKE_SYSTEMCTL_LOG"
args=("$@")
[ "${args[0]}" = "--user" ] && args=("${args[@]:1}")
verb="${args[0]}"
unit="${args[${#args[@]}-1]}"
case "$verb" in
  is-active)
    for u in ${FAKE_ACTIVE:-}; do [ "$u" = "$unit" ] && exit 0; done
    exit 3
    ;;
  start)
    [ "${FAKE_START_FAIL:-}" = "$unit" ] && exit 1
    exit 0
    ;;
esac
exit 0
EOF2
chmod +x "$root/bin/systemctl"

export PATH="$root/bin:$PATH"
export HOME="$root/home"
export CELERIS_CONFIG_DIR="$root/config"
export CELERIS_STATE_DIR="$root/state"
export FAKE_SYSTEMCTL_LOG="$root/calls.log"

OLD=aaaaaaaaaaaa
NEW=bbbbbbbbbbbb
make_release() { # <sha> <web_ok>
  local d="$root/state/releases/$1"
  mkdir -p "$d/web/app/server"
  : >"$d/web/app/server/index.js"
  printf '{"ok": true, "web": {"ok": %s, "blocking": false}}\n' "$2" >"$d/gate.json"
}

run_follow() {
  : >"$root/calls.log"
  if ! bash "$here/web-follow.sh" "$@" >"$root/out.log" 2>&1; then fail "web-follow.sh exited non-zero ($*)"; fi
}
web_ops() { grep -E '^--user (start|stop|enable|disable|restart) ' "$root/calls.log" || true; }
no_celeris_daemon_calls() {
  if grep -Eq '(^| )celeris@' "$root/calls.log"; then fail "celeris@ was touched: $1"; fi
}

# (a)
make_release "$NEW" true
export FAKE_ACTIVE="celeris-web@$OLD"
run_follow "$NEW" "$OLD"
want="--user start celeris-web@$NEW
--user enable celeris-web@$NEW
--user stop celeris-web@$OLD
--user disable celeris-web@$OLD"
[ "$(web_ops)" = "$want" ] || fail "(a) unexpected operations: $(web_ops)"
no_celeris_daemon_calls "(a)"
echo "ok (a) old active + web.ok=true -> start/enable new, stop/disable old"

# (b)
make_release "$NEW" false
run_follow "$NEW" "$OLD"
[ -z "$(web_ops)" ] || fail "(b) web.ok=false but operations: $(web_ops)"
grep -q 'web.ok=true' "$root/out.log" || fail "(b) no reason logged"
no_celeris_daemon_calls "(b)"
echo "ok (b) web.ok=false -> no web operations"

# (b2) web.ok=true だが server/index.js が無い
make_release "$NEW" true
rm -f "$root/state/releases/$NEW/web/app/server/index.js"
run_follow "$NEW" "$OLD"
[ -z "$(web_ops)" ] || fail "(b2) missing index.js but operations: $(web_ops)"
no_celeris_daemon_calls "(b2)"
echo "ok (b2) missing web/app/server/index.js -> no web operations"

# (c)
make_release "$NEW" true
export FAKE_ACTIVE=""
run_follow "$NEW" "$OLD"
[ -z "$(web_ops)" ] || fail "(c) old inactive but operations: $(web_ops)"
grep -q 'not active' "$root/out.log" || fail "(c) no reason logged"
no_celeris_daemon_calls "(c)"
run_follow "$NEW" ""
[ -z "$(web_ops)" ] || fail "(c) empty old but operations: $(web_ops)"
no_celeris_daemon_calls "(c, empty old)"
echo "ok (c) old inactive / empty old -> no operations"

# (e)
export FAKE_ACTIVE="celeris-web@$OLD"
export FAKE_START_FAIL="celeris-web@$NEW"
run_follow "$NEW" "$OLD"
want="--user start celeris-web@$NEW
--user stop celeris-web@$NEW
--user start celeris-web@$OLD"
[ "$(web_ops)" = "$want" ] || fail "(e) unexpected operations: $(web_ops)"
no_celeris_daemon_calls "(e)"
unset FAKE_START_FAIL
echo "ok (e) start of new failed -> new stopped, old restarted, exit 0"

# promote.sh が昇格後に web-follow.sh を呼ぶ（静的確認）
grep -q 'web-follow.sh' "$here/promote.sh" || fail "promote.sh does not call web-follow.sh"
grep -q '"\$WEB_FOLLOW" "\$SHA12" "\$OLD"' "$here/promote.sh" || fail "promote.sh does not pass SHA12/OLD"
# unit は celeris@ を Wants にしない
unit="$here/../../deploy/systemd/celeris-web@.service"
if grep -q '^Wants=celeris@' "$unit"; then fail "celeris-web@.service still has Wants=celeris@"; fi
echo "ok (d) no celeris@ calls in any case; promote.sh calls web-follow.sh; unit has no Wants=celeris@"
echo "promote_web_follows_release: all ok"
