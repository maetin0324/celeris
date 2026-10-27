#!/usr/bin/env bash
# promote.sh・rollback.sh の credentiald 切り替え（sd_switch_credentiald）を偽の systemctl で確かめる。
#   A 旧を止めてから新を起こす（順序）。reset-failed を挟む。
#   B 旧が active でなければ止めない。
#   C 新の unit が無ければ何もしない。
#   D 新が起動後に active でなければ失敗（1）。
#   E 旧の停止に失敗したら新を起こさず失敗。
#   G〜K promote.sh・rollback.sh を偽の systemctl / curl / sqlite3 / ss で最後まで走らせる:
#     G live: link の付け替え・promoted.json の後に旧を止めて新を起こす（exit 0）
#     H stop-start で新が active にならない: exit 3、current は新、promoted.json あり、promote_failed.json なし、
#       credentiald_switch_failed.json あり
#     I 引き継ぎの再開（新の celeris が既に active）でも切り替える
#     J rollback.sh --restore-db で新が active にならない: exit 3、current は previous、promoting.json なし、
#       credentiald_switch_failed.json あり
#     K rollback.sh --restore-db の成功: link の付け替えの後に切り替える（exit 0）
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

# ---- G〜K: promote.sh・rollback.sh を最後まで走らせる ----------------------------------
# 本番には触れない: HOME / CELERIS_* は一時 dir、systemctl・curl・sqlite3・ss は偽物。
mkdir -p "$WORK/scripts" "$WORK/fakebin"
cp "$SD"/*.sh "$WORK/scripts/"
printf '#!/bin/sh\nexit 0\n' >"$WORK/scripts/web-follow.sh"
chmod +x "$WORK/scripts/web-follow.sh"
OLD=aaaaaaaaaaaa NEW=bbbbbbbbbbbb
FB="$WORK/fakebin"
# systemctl: 呼び出しと、その時の current のリンク先を記録する。celeris@ の is-active は偽（promote.sh の
# 他の判定を変えない）。credentiald は started-<unit> の有無で答え、fail-active-<unit> があれば起動後も非 active。
cat >"$FB/systemctl" <<'EOF'
#!/usr/bin/env bash
echo "$* current=$(readlink "$CELERIS_STATE_DIR/current" 2>/dev/null)" >>"$FAKE_STATE/systemctl.log"
[ "${1:-}" = --user ] && shift
cmd="${1:-}"; shift || true
[ "${1:-}" = --quiet ] && shift
unit="${1:-}"
case "$cmd" in
  start) [ -e "$FAKE_STATE/fail-active-$unit" ] || touch "$FAKE_STATE/started-$unit"; exit 0 ;;
  stop) rm -f "$FAKE_STATE/started-$unit"; exit 0 ;;
  is-active)
    case "$unit" in celeris-credentiald@*) [ -e "$FAKE_STATE/started-$unit" ] ;; *) exit 1 ;; esac ;;
  show) echo 0 ;;
  *) exit 0 ;;
esac
EOF
cat >"$FB/curl" <<'EOF'
#!/usr/bin/env bash
out=/dev/null url=""
while [ $# -gt 0 ]; do
  case "$1" in
    -o) out="$2"; shift ;;
    -w | -m | -H | -X | --data-binary | --max-redirs) shift ;;
    -*) ;;
    *) url="$1" ;;
  esac
  shift
done
body="" code=000
case "$url" in
  *:7710/api/v1/health)
    for sha in $FAKE_NEW $FAKE_OLD; do
      if [ -e "$FAKE_STATE/started-celeris@$sha" ]; then
        body="{\"release\":\"$sha\",\"role\":\"active\",\"schema_version\":36}"; code=200; break
      fi
    done ;;
  *:7710/api/v1/releases)
    if [ -e "$FAKE_STATE/started-celeris@$FAKE_NEW" ]; then
      body="{\"instances\":[{\"release\":\"$FAKE_NEW\",\"role\":\"active\"},{\"release\":\"$FAKE_OLD\",\"role\":\"draining\"}]}"; code=200
    fi ;;
esac
printf '%s' "$body" >"$out"
printf '%s' "$code"
[ "$code" = 000 ] && exit 7
exit 0
EOF
cat >"$FB/sqlite3" <<'EOF'
#!/usr/bin/env bash
for a in "$@"; do
  case "$a" in .backup*) p="${a#.backup }"; p="${p#\'}"; p="${p%\'}"; echo fake-db >"$p"; exit 0 ;; esac
done
echo 36
EOF
printf '#!/bin/sh\necho "State Recv-Q Send-Q Local Address:Port Peer Address:Port Process"\n' >"$FB/ss"
chmod +x "$FB"/*

setup() {
  CASE="$WORK/$1"
  mkdir -p "$CASE/home" "$CASE/fake"
  export HOME="$CASE/home" FAKE_STATE="$CASE/fake" FAKE_OLD="$OLD" FAKE_NEW="$NEW"
  export CELERIS_CONFIG_DIR="$HOME/.config/celeris" CELERIS_STATE_DIR="$HOME/.local/celeris"
  export CELERIS_DB="$CELERIS_STATE_DIR/celeris.sqlite3" SD_REPO="$CASE/no-repo"
  unset CELERIS_CONFIG SD_LOG_FILE CELERIS_BACKUPS_DIR CELERIS_LOGS_DIR
  mkdir -p "$CELERIS_CONFIG_DIR" "$CELERIS_STATE_DIR/releases" "$CELERIS_STATE_DIR/backups"
  printf 'db = "%s"\n' "$CELERIS_DB" >"$CELERIS_CONFIG_DIR/config.toml"
  echo fake-db >"$CELERIS_DB"
  for sha in "$OLD" "$NEW"; do
    mkdir -p "$CELERIS_STATE_DIR/releases/$sha/bin"
    printf '#!/bin/sh\nexit 0\n' >"$CELERIS_STATE_DIR/releases/$sha/bin/celeris"
    chmod +x "$CELERIS_STATE_DIR/releases/$sha/bin/celeris"
    printf '{"schema_version": 36}\n' >"$CELERIS_STATE_DIR/releases/$sha/manifest.json"
    printf '{"ok": true, "live_ok": %s}\n' "$2" >"$CELERIS_STATE_DIR/releases/$sha/verify.json"
  done
  ln -sfn "releases/$OLD" "$CELERIS_STATE_DIR/current"
  : >"$FAKE_STATE/systemctl.log"
}
run_script() {
  local script="$1"; shift
  set +e
  PATH="$FB:$PATH" SD_STOP_WAIT=1 bash "$WORK/scripts/$script" "$@" >"$CASE/out.log" 2>&1
  RC=$?
  set -e
}
cred() { echo "celeris-credentiald@$1.service"; }
# 偽 systemctl の記録で、旧の stop < 新の start で、どちらも current が want を指した後か。
switched_after_link() {
  local want="$1" from="$2" to="$3" log="$FAKE_STATE/systemctl.log" s st
  s="$(grep -n -- "stop $(cred "$from") " "$log" | head -1 | cut -d: -f1)"
  st="$(grep -n -- "start $(cred "$to") " "$log" | head -1 | cut -d: -f1)"
  [ -n "$s" ] && [ -n "$st" ] && [ "$s" -lt "$st" ] \
    && grep -q -- "stop $(cred "$from") current=releases/$want\$" "$log" \
    && grep -q -- "start $(cred "$to") current=releases/$want\$" "$log"
}
rel() { echo "$CELERIS_STATE_DIR/releases/$1"; }
check() { if eval "$2"; then ok "$1"; else ng "$1"; echo "---- $CASE/out.log" >&2; cat "$CASE/out.log" >&2; fi; }

# G live
setup live true
touch "$FAKE_STATE/started-celeris@$OLD" "$FAKE_STATE/started-$(cred "$OLD")"
run_script promote.sh "$NEW"
check "G: live exit 0 (rc=$RC)" '[ "$RC" = 0 ] && grep -q "promotion mode: live" "$CASE/out.log"'
check "G: old stopped before new started, after current -> new" 'switched_after_link "$NEW" "$OLD" "$NEW"'
check "G: promoted.json, no failure files" '[ -f "$(rel "$NEW")/promoted.json" ] && [ ! -e "$(rel "$NEW")/promote_failed.json" ] && [ ! -e "$(rel "$NEW")/credentiald_switch_failed.json" ]'

# H stop-start、新が active にならない
setup stop-start-fail false
touch "$FAKE_STATE/started-$(cred "$OLD")" "$FAKE_STATE/fail-active-$(cred "$NEW")"
run_script promote.sh "$NEW"
check "H: stop-start exit 3 (rc=$RC)" '[ "$RC" = 3 ] && grep -q "promotion mode: stop-start" "$CASE/out.log"'
check "H: current -> new, promoted.json kept" '[ "$(readlink "$CELERIS_STATE_DIR/current")" = "releases/$NEW" ] && [ -f "$(rel "$NEW")/promoted.json" ]'
check "H: no promote_failed.json, credentiald_switch_failed.json written" '[ ! -e "$(rel "$NEW")/promote_failed.json" ] && python3 -c "import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if d[\"from\"]==sys.argv[2] and d[\"failed_at\"] else 1)" "$(rel "$NEW")/credentiald_switch_failed.json" "$OLD"'
check "H: no promoting.json left" '[ -z "$(find "$CELERIS_STATE_DIR/releases" -name "promoting.json*")" ]'

# I 引き継ぎの再開（stop-start。新の celeris は既に active、link はまだ旧）
setup resume false
touch "$FAKE_STATE/started-celeris@$NEW" "$FAKE_STATE/started-$(cred "$OLD")"
echo old-failure >"$(rel "$NEW")/credentiald_switch_failed.json"
run_script promote.sh "$NEW"
check "I: resume exit 0 (rc=$RC)" '[ "$RC" = 0 ] && grep -q "resuming the incomplete" "$CASE/out.log" && ! grep -q "start celeris@" "$FAKE_STATE/systemctl.log"'
check "I: switched after current -> new" 'switched_after_link "$NEW" "$OLD" "$NEW"'
check "I: an old credentiald_switch_failed.json is removed on success" '[ ! -e "$(rel "$NEW")/credentiald_switch_failed.json" ]'

# J rollback --restore-db、previous の credentiald が active にならない
setup rollback-fail false
ln -sfn "releases/$NEW" "$CELERIS_STATE_DIR/current"
ln -sfn "releases/$OLD" "$CELERIS_STATE_DIR/previous"
echo backup >"$CELERIS_STATE_DIR/backups/20261001-000000-pre-$NEW.sqlite3"
touch "$FAKE_STATE/started-$(cred "$NEW")" "$FAKE_STATE/fail-active-$(cred "$OLD")"
run_script rollback.sh --restore-db
check "J: rollback exit 3 (rc=$RC)" '[ "$RC" = 3 ]'
check "J: current -> previous, previous -> rolled-back release" '[ "$(readlink "$CELERIS_STATE_DIR/current")" = "releases/$OLD" ] && [ "$(readlink "$CELERIS_STATE_DIR/previous")" = "releases/$NEW" ]'
check "J: no promoting.json left, credentiald_switch_failed.json written" '[ -z "$(find "$CELERIS_STATE_DIR/releases" -name "promoting.json*")" ] && [ -f "$(rel "$OLD")/credentiald_switch_failed.json" ]'
check "J: switch ran after the links were updated" 'grep -q -- "start $(cred "$OLD") current=releases/$OLD\$" "$FAKE_STATE/systemctl.log"'

# K rollback --restore-db の成功
setup rollback false
ln -sfn "releases/$NEW" "$CELERIS_STATE_DIR/current"
ln -sfn "releases/$OLD" "$CELERIS_STATE_DIR/previous"
echo backup >"$CELERIS_STATE_DIR/backups/20261001-000000-pre-$NEW.sqlite3"
touch "$FAKE_STATE/started-$(cred "$NEW")"
run_script rollback.sh --restore-db
check "K: rollback exit 0 (rc=$RC)" '[ "$RC" = 0 ]'
check "K: new stopped before previous started, after current -> previous" 'switched_after_link "$OLD" "$NEW" "$OLD"'

exit "$FAIL"
