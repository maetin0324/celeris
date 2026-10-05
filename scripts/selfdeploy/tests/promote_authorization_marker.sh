#!/bin/sh
# scripts/selfdeploy/tests/promote_authorization_marker.sh — ADR-0040 付記 2026-10-02
# 「handoff と migration の認可（昇格していない release）」:
#
#   promote.sh（live / stop-start）と rollback.sh --restore-db が、`celeris@<sha12>` を start する
#   **時点で** `releases/<sha12>/promoting.json`（自分の sha12）を置いていること、終わった後
#   （成功でも失敗でも）に印が残らないことを確かめる。
#
# 本番には触れない: HOME / CELERIS_CONFIG_DIR / CELERIS_STATE_DIR / CELERIS_DB は一時ディレクトリ。
# `systemctl` / `curl` / `sqlite3` / `ss` は PATH の先頭に置いた偽物（`ss` を偽にしないと promote.sh の
# stop-start が本物の :7700 の GUI を見つけて SIGTERM する）。偽 `systemctl start celeris@X` が、
# その瞬間の印と `current` のリンク先を書き留める。
#
# 実行: sh scripts/selfdeploy/tests/promote_authorization_marker.sh
# dash/sh で走る POSIX sh（bash 配列展開は使わない: ${BASH_SOURCE[0]} も ${@:2} も不可）。
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
SD="$(cd "$HERE/.." && pwd)"

FAIL=0
ok() { echo "ok: $*"; }
ng() { echo "FAIL: $*" >&2; FAIL=1; }
assert_eq() {
  local desc="$1" want="$2" got="$3"
  if [ "$want" = "$got" ]; then ok "$desc"; else ng "$desc (want=[$want] got=[$got])"; fi
}
json_get() { python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))[sys.argv[2]])' "$1" "$2"; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

OLD=aaaaaaaaaaaa
NEW=bbbbbbbbbbbb
FAKE_BIN="$WORK/fakebin"
mkdir -p "$FAKE_BIN"

# ---- 偽の道具 ----------------------------------------------------------------

# systemctl: 呼び出しを記録し、`start celeris@X` の瞬間の印と current を残す。
# `$FAKE_STATE/fail-start-<unit>` があればその start は失敗する。
cat >"$FAKE_BIN/systemctl" <<'EOF'
#!/usr/bin/env bash
echo "$*" >>"$FAKE_STATE/systemctl.log"
[ "${1:-}" = --user ] && shift
cmd="${1:-}"; shift || true
case "$cmd" in
  cat) exit 0 ;;
  start)
    unit="$1"
    case "$unit" in
      celeris@*)
        sha="${unit#celeris@}"
        m="$CELERIS_STATE_DIR/releases/$sha/promoting.json"
        if [ -f "$m" ]; then cp "$m" "$FAKE_STATE/at-start-$sha.promoting.json"; fi
        readlink "$CELERIS_STATE_DIR/current" >"$FAKE_STATE/at-start-$sha.current" 2>/dev/null || : >"$FAKE_STATE/at-start-$sha.current"
        ;;
    esac
    [ -e "$FAKE_STATE/fail-start-$unit" ] && exit 1
    touch "$FAKE_STATE/started-$unit"
    exit 0 ;;
  stop) rm -f "$FAKE_STATE/started-$1"; exit 0 ;;
  is-active) exit 1 ;;
  enable | disable | daemon-reload) exit 0 ;;
  show) echo 0; exit 0 ;;
  list-units) exit 0 ;;
  *) exit 0 ;;
esac
EOF

# curl: -o <file> に本文、-w の代わりに http code を標準出力へ。応答は偽 systemd の状態で決める。
cat >"$FAKE_BIN/curl" <<'EOF'
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
    # ADR-0040 D4 付記 2026-10-05: 新が起きたら、旧は draining（handoff 済み）として答える。
    if [ -e "$FAKE_STATE/started-celeris@$FAKE_NEW" ]; then
      body="{\"instances\":[{\"release\":\"$FAKE_NEW\",\"role\":\"active\"},{\"release\":\"$FAKE_OLD\",\"role\":\"draining\"}]}"; code=200
    fi ;;
  *:7700/healthz)
    for sha in $FAKE_NEW $FAKE_OLD; do
      if [ -e "$FAKE_STATE/started-celeris-gui@$sha" ]; then
        body="{\"release\":\"$sha\",\"name\":\"celeris-gui\"}"; code=200; break
      fi
    done ;;
esac
printf '%s' "$body" >"$out"
printf '%s' "$code"
[ "$code" = 000 ] && exit 7
exit 0
EOF

# sqlite3: `.backup '<path>'` だけ真似る（本物の DB は開かない）。
cat >"$FAKE_BIN/sqlite3" <<'EOF'
#!/usr/bin/env bash
for a in "$@"; do
  case "$a" in
    .backup*) p="${a#.backup }"; p="${p#\'}"; p="${p%\'}"; echo fake-db >"$p"; exit 0 ;;
  esac
done
echo 36
EOF

# ss: 何も LISTEN していない（本物の :7700 の GUI を見つけさせない）。
cat >"$FAKE_BIN/ss" <<'EOF'
#!/usr/bin/env bash
echo "State Recv-Q Send-Q Local Address:Port Peer Address:Port Process"
EOF
chmod +x "$FAKE_BIN"/*

# ---- 一時の state を作る -------------------------------------------------------

# setup <case> <live_ok>: HOME ごと作り直し、old が current・previous は無し・new は verify 済み。
setup() {
  local name="$1" live_ok="$2" sha
  CASE="$WORK/$name"
  mkdir -p "$CASE/home" "$CASE/fake"
  export HOME="$CASE/home"
  export CELERIS_CONFIG_DIR="$HOME/.config/celeris"
  export CELERIS_STATE_DIR="$HOME/.local/celeris"
  export CELERIS_DB="$CELERIS_STATE_DIR/celeris.sqlite3"
  export FAKE_STATE="$CASE/fake" FAKE_OLD="$OLD" FAKE_NEW="$NEW"
  export SD_REPO="$CASE/no-repo"
  unset CELERIS_CONFIG SD_LOG_FILE
  mkdir -p "$CELERIS_CONFIG_DIR" "$CELERIS_STATE_DIR/releases" "$CELERIS_STATE_DIR/backups"
  printf 'db = "%s"\n' "$CELERIS_DB" >"$CELERIS_CONFIG_DIR/config.toml"
  echo fake-db >"$CELERIS_DB"
  for sha in "$OLD" "$NEW"; do
    mkdir -p "$CELERIS_STATE_DIR/releases/$sha/bin"
    printf '#!/bin/sh\nexit 0\n' >"$CELERIS_STATE_DIR/releases/$sha/bin/celeris"
    chmod +x "$CELERIS_STATE_DIR/releases/$sha/bin/celeris"
    printf '{"schema_version": 36}\n' >"$CELERIS_STATE_DIR/releases/$sha/manifest.json"
    printf '{"ok": true, "live_ok": %s}\n' "$live_ok" >"$CELERIS_STATE_DIR/releases/$sha/verify.json"
  done
  ln -sfn "releases/$OLD" "$CELERIS_STATE_DIR/current"
}

run_script() {
  # run_script <script> [args...] — 偽の PATH で走らせ、exit code を RC に入れる。
  # 呼ばれる promote.sh / rollback.sh 自体は bash 前提なので bash で起動する
  # （このテスト script 自身は sh/dash で走る前提: ${@:2} は使えないので shift で渡す）。
  script="$1"
  shift
  set +e
  PATH="$FAKE_BIN:$PATH" SD_STOP_WAIT=1 bash "$SD/$script" "$@" >"$CASE/out.log" 2>&1
  RC=$?
  set -e
}

# 印の中身・時機・後片付けを確かめる。
check_marker_at_start() {
  local label="$1" sha="$2" script="$3" mode="$4" seen="$FAKE_STATE/at-start-$2.promoting.json"
  if [ -f "$seen" ]; then
    ok "$label: promoting.json existed when systemctl start celeris@$sha ran"
    assert_eq "$label: marker sha12" "$sha" "$(json_get "$seen" sha12)"
    assert_eq "$label: marker script" "$script" "$(json_get "$seen" script)"
    assert_eq "$label: marker mode" "$mode" "$(json_get "$seen" mode)"
    case "$(json_get "$seen" started_at)" in
      [0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T*Z) ok "$label: started_at is RFC 3339 UTC" ;;
      *) ng "$label: started_at is not RFC 3339 UTC: $(json_get "$seen" started_at)" ;;
    esac
  else
    ng "$label: no promoting.json when systemctl start celeris@$sha ran (current -> $(cat "$FAKE_STATE/at-start-$sha.current" 2>/dev/null || echo '?'))"
  fi
}
check_no_marker_left() {
  local label="$1" left
  left="$(find "$CELERIS_STATE_DIR/releases" -name 'promoting.json*' 2>/dev/null)"
  if [ -z "$left" ]; then ok "$label: no promoting.json left behind"; else ng "$label: left behind: $left"; fi
}
dump_on_fail() { if [ "$FAIL" -ne 0 ]; then echo "---- $CASE/out.log" >&2; cat "$CASE/out.log" >&2; fi; }

# ---- ケース 1: promote.sh live ------------------------------------------------
setup live true
touch "$FAKE_STATE/started-celeris@$OLD" "$FAKE_STATE/started-celeris-gui@$OLD"
# 偽 curl は new が start 済みなら new を答える（実機では handoff で旧が listener を閉じる）。
run_script promote.sh "$NEW"
assert_eq "live: promote.sh exit" 0 "$RC"
grep -q "promotion mode: live" "$CASE/out.log" && ok "live: took the live path" || ng "live: did not take the live path"
check_marker_at_start live "$NEW" promote.sh live
check_no_marker_left live
assert_eq "live: current -> new" "releases/$NEW" "$(readlink "$CELERIS_STATE_DIR/current")"
dump_on_fail

# ---- ケース 2: promote.sh stop-start ------------------------------------------
setup stop-start false
run_script promote.sh "$NEW"
assert_eq "stop-start: promote.sh exit" 0 "$RC"
grep -q "promotion mode: stop-start" "$CASE/out.log" && ok "stop-start: took the stop-start path" || ng "stop-start: did not take the stop-start path"
check_marker_at_start stop-start "$NEW" promote.sh stop-start
check_no_marker_left stop-start
assert_eq "stop-start: current -> new" "releases/$NEW" "$(readlink "$CELERIS_STATE_DIR/current")"
[ -f "$CELERIS_STATE_DIR/releases/$NEW/promoted.json" ] && ok "stop-start: promoted.json written" || ng "stop-start: promoted.json missing"
dump_on_fail

# ---- ケース 3: live で start が失敗 → EXIT トラップが印を消す --------------------
setup live-fail true
touch "$FAKE_STATE/started-celeris@$OLD" "$FAKE_STATE/fail-start-celeris@$NEW"
run_script promote.sh "$NEW"
[ "$RC" -ne 0 ] && ok "live-fail: promote.sh failed (exit $RC)" || ng "live-fail: promote.sh should fail"
check_marker_at_start live-fail "$NEW" promote.sh live
check_no_marker_left live-fail
assert_eq "live-fail: current unchanged" "releases/$OLD" "$(readlink "$CELERIS_STATE_DIR/current")"
[ -f "$CELERIS_STATE_DIR/releases/$NEW/promote_failed.json" ] && ok "live-fail: promote_failed.json written" || ng "live-fail: promote_failed.json missing"
dump_on_fail

# ---- ケース 4: stop-start で start が失敗 → 旧を起こし直し、印は残らない ------------
setup stop-start-fail false
touch "$FAKE_STATE/fail-start-celeris@$NEW"
run_script promote.sh "$NEW"
[ "$RC" -ne 0 ] && ok "stop-start-fail: promote.sh failed (exit $RC)" || ng "stop-start-fail: promote.sh should fail"
check_marker_at_start stop-start-fail "$NEW" promote.sh stop-start
check_no_marker_left stop-start-fail
assert_eq "stop-start-fail: current unchanged" "releases/$OLD" "$(readlink "$CELERIS_STATE_DIR/current")"
dump_on_fail

# ---- ケース 5: rollback.sh --restore-db（current はまだ new。previous = old を起こす） --
setup rollback false
ln -sfn "releases/$NEW" "$CELERIS_STATE_DIR/current"
ln -sfn "releases/$OLD" "$CELERIS_STATE_DIR/previous"
echo backup >"$CELERIS_STATE_DIR/backups/20261001-000000-pre-$NEW.sqlite3"
run_script rollback.sh --restore-db
assert_eq "rollback: rollback.sh exit" 0 "$RC"
check_marker_at_start rollback "$OLD" rollback.sh restore-db
check_no_marker_left rollback
assert_eq "rollback: current -> old" "releases/$OLD" "$(readlink "$CELERIS_STATE_DIR/current")"
dump_on_fail

# 偽の systemctl だけが呼ばれたこと（本番 host の systemctl は使わない）の念押し: 記録があること。
[ -s "$WORK/live/fake/systemctl.log" ] && ok "fake systemctl recorded the calls" || ng "fake systemctl was not used"

if [ "$FAIL" -ne 0 ]; then
  echo "promote_authorization_marker: FAILED" >&2
  exit 1
fi
echo "promote_authorization_marker: all ok"
