#!/usr/bin/env bash
# scripts/selfdeploy/promote.sh <sha12> — ADR-0040 D4 の「昇格」段。**人だけが実行する**（D5）。
#
#   verify.json.ok が真でなければ拒否する（`--force` は無い）。DB をバックアップし、
#     - ライブ引き継ぎ（verify.json.live_ok が真で、いま動いている celeris の /health が `role` を持つ）
#     - 停止 → 起動（live_ok が偽、または /health に `role` が無い＝この ADR 以前の版）
#   のどちらかで切り替え、`current` / `previous` を更新する。すべて
#   $SD_LOGS/promote-<ts>.log に残る。
set -euo pipefail

SD_PROG=promote
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

usage() {
  cat >&2 <<'EOF'
usage: promote.sh <sha12> [--pre-start <script>] [--stop-stale]

  verify.sh が `ok` を出したリリースだけを昇格できる。`--force` は無い（ADR-0040 D2）。
  systemd の unit が要る: scripts/selfdeploy/install-units.sh を一度だけ実行しておくこと。

  --stop-stale  昇格の対象（current/new）以外に active な celeris@* unit が残っていたら
                （drain したまま終了しなかった前回昇格の残骸。Phase 119 D3）SIGKILL する。
                既定では一覧を出すだけで、何も止めない（人が判断する）。
EOF
  exit 2
}

# `--pre-start <script>`: 停止→起動のとき、旧が止まり DB のバックアップを取った**後**、新を起こす**前**に 1 度だけ実行する
# （ADR-0046 D7 の `celerisctl org migrate-v2` のように、デーモンが止まっている間に DB を直接触る移行のため）。
# 引数は `<sha12> <release dir> <db path> <config path>`。失敗したら DB をバックアップから戻し、旧 unit を起こし直して止まる。
# ライブ引き継ぎ（live）では実行できない（デーモンが動いている）ので、指定があれば停止→起動に倒す。
PRE_START=""
# Phase 119 D3: 既定は出すだけ。SIGKILL するのは `--stop-stale` を明示したときだけ（人が判断する）。
STOP_STALE=false
SHA12=""
while [ $# -gt 0 ]; do
  case "$1" in
    --pre-start)
      shift
      [ $# -ge 1 ] || usage
      PRE_START="$1"
      [ -x "$PRE_START" ] || { echo "promote.sh: --pre-start script is not executable: $PRE_START" >&2; exit 2; }
      ;;
    --stop-stale) STOP_STALE=true ;;
    -h | --help) usage ;;
    --*) usage ;;
    *)
      [ -z "$SHA12" ] || usage
      SHA12="$1"
      ;;
  esac
  shift
done
[ -n "$SHA12" ] || usage

sd_require_json_tool
TS="$(sd_stamp)"
mkdir -p "$SD_BACKUPS" "$SD_LOGS"
SD_LOG_FILE="$SD_LOGS/promote-$TS.log"
sd_log "promote $SHA12 (log: $SD_LOG_FILE)"

REL="$(sd_release_dir "$SHA12")"

# GUI の昇格ボタンは detached なこのプロセスの成否を直接見られない（celeris は起動できたことしか
# 知らない）。だから **失敗したら `$REL/promote_failed.json` を残す**（`sd_die` に限らず、`set -e` で
# 途中終了した場合も含めて EXIT トラップで拾う）。`promoted.json`（成功）と対になる印で、
# `GET /releases` の `items[].promote_failed` に写り、GUI が赤いバナーで出す。
# 次の昇格の試みが始まるとき（celeris 側の `start_promote`）に消される — 古い失敗を引きずらない。
#
# ADR-0040 付記 2026-10-02: start の直前に置く昇格中の印（`$REL/promoting.json`）も、成功・失敗の
# どちらでもここで消す（`update_links` の直後に消し損ねた場合、`sd_die` / `set -e` / シグナルでの途中終了）。
PROMOTING_WRITTEN=false
record_promote_failure() {
  local ec=$?
  if [ "$PROMOTING_WRITTEN" = true ]; then sd_clear_promoting "$SHA12" || true; fi
  [ "$ec" -eq 0 ] && return
  [ -d "$REL" ] || return
  {
    printf '{\n'
    printf '  "failed_at": %s,\n' "$(sd_json_str "$(sd_ts)")"
    printf '  "error": %s\n' "$(sd_json_str "$(tail -n 20 "$SD_LOG_FILE" 2>/dev/null | tr -d '\r')")"
    printf '}\n'
  } >"$REL/promote_failed.json" 2>/dev/null || true
}
trap record_promote_failure EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

[ -d "$REL" ] || sd_die "no such release: $REL"
[ -x "$REL/bin/celeris" ] || sd_die "missing $REL/bin/celeris"
[ -f "$REL/verify.json" ] || sd_die "$SHA12 has no verify.json — run verify.sh first"
[ "$(sd_json_get "$REL/verify.json" ok 2>/dev/null || echo false)" = true ] \
  || sd_die "verify.json of $SHA12 is not ok — refusing to promote (there is no --force)"

LIVE_OK="$(sd_json_get "$REL/verify.json" live_ok 2>/dev/null || echo false)"
WANT_SCHEMA="$(sd_json_get "$REL/manifest.json" schema_version)"
OLD="$(sd_current_sha)"
sd_log "release=$SHA12 schema_version=$WANT_SCHEMA verify.live_ok=$LIVE_OK current=${OLD:-<none>}"

# ---- 昇格の要約（ADR 2026-10-04-release-notes）------------------------------
#
# `current` から $SHA12 までに入る task・migration・schema を、**`current` を切り替える前に**ログへ出す。
# 道具が無い・失敗したら警告だけで続ける（昇格を止めない）。JSON は promoted.json の `included` に使う。
PREVIEW_JSON="$(mktemp "${TMPDIR:-/tmp}/promote-preview.XXXXXX")" || PREVIEW_JSON=""
PREVIEW_OK=false
PREVIEW_TXT=""
if [ -x "$REL/bin/celerisctl" ] && [ -n "$PREVIEW_JSON" ] \
  && PREVIEW_TXT="$("$REL/bin/celerisctl" release preview "$SHA12" --releases-dir "$SD_RELEASES" 2>/dev/null)" \
  && "$REL/bin/celerisctl" release preview "$SHA12" --releases-dir "$SD_RELEASES" --json >"$PREVIEW_JSON" 2>/dev/null; then
  PREVIEW_OK=true
  printf '%s\n' "$PREVIEW_TXT" | while IFS= read -r line; do sd_log "preview: $line"; done
else
  sd_log "warning: promotion preview is not available (celerisctl release preview failed or is missing); continuing"
fi

# ---- systemd の unit があるか ----------------------------------------------

systemctl --user cat celeris@.service >/dev/null 2>&1 \
  || sd_die "systemd user template celeris@.service is not installed; run scripts/selfdeploy/install-units.sh first"
systemctl --user cat celeris-gui@.service >/dev/null 2>&1 \
  || sd_die "systemd user template celeris-gui@.service is not installed; run scripts/selfdeploy/install-units.sh first"

# ---- いま動いているものを見る（読むだけ） ----------------------------------

PROD_HEALTH="$SD_BACKUPS/promote-$TS.health-before.json"
LIVE_ROLE=""
LIVE_RELEASE=""
if [ "$(sd_http_status "$SD_PROD_API/api/v1/health")" = 200 ]; then
  sd_http_get "$SD_PROD_API/api/v1/health" >"$PROD_HEALTH" || true
  LIVE_ROLE="$(sd_json_get "$PROD_HEALTH" role 2>/dev/null || echo "")"
  LIVE_RELEASE="$(sd_json_get "$PROD_HEALTH" release 2>/dev/null || echo "")"
  sd_log "running celeris: release=${LIVE_RELEASE:-<unknown>} role=${LIVE_ROLE:-<none>} schema_version=$(sd_json_get "$PROD_HEALTH" schema_version || echo '?')"
else
  sd_log "no healthy celeris on $SD_PROD_API right now"
fi

if [ "$LIVE_RELEASE" = "$SHA12" ]; then
  if [ "$OLD" != "$SHA12" ] && [ "$LIVE_OK" = true ] && [ "$LIVE_ROLE" = active ] && [ -z "$PRE_START" ]; then
    sd_log "celeris already took over; resuming the incomplete GUI/link handoff"
  else
    sd_die "the running celeris already reports release=$SHA12; nothing to promote"
  fi
fi

MODE=stop-start
if [ "$LIVE_OK" = true ] && [ -n "$LIVE_ROLE" ] && [ -z "$PRE_START" ]; then MODE=live; fi
if [ -n "$PRE_START" ]; then sd_log "--pre-start given ($PRE_START): forcing stop-start"; fi
sd_log "promotion mode: $MODE"

# ---- Phase 119 D3: 他に残っている celeris@* の draining unit を検出する --------------------
#
# 実機 2026-09-24: 過去 4 世代の celeris@<sha> が drain したまま終了せず active running のまま
# 溜まっていた（ADR-0040 追記）。昇格の対象（$OLD / $SHA12）以外に active な unit があれば、
# ここで一覧を出す（既定はそれだけ）。`--stop-stale` が付いていれば SIGKILL する。
STALE_SHAS="$(sd_list_stale_celeris_units "$SHA12" "$OLD" || true)"
if [ -n "$STALE_SHAS" ]; then
  sd_log "stale: drained だが終了していない celeris インスタンスが他にもいる（current=${OLD:-<none>} new=$SHA12 以外に active）:"
  while IFS= read -r stale_sha; do
    [ -n "$stale_sha" ] || continue
    stale_pid="$(systemctl --user show -p MainPID --value "celeris@$stale_sha" 2>/dev/null || echo 0)"
    sd_log "  celeris@$stale_sha pid=${stale_pid:-?}"
    if [ "$STOP_STALE" = true ] && [ -n "$stale_pid" ] && [ "$stale_pid" != 0 ]; then
      sd_log "  --stop-stale: SIGKILL celeris@$stale_sha (pid=$stale_pid)"
      kill -KILL "$stale_pid" 2>/dev/null || true
      systemctl --user stop "celeris@$stale_sha" 2>/dev/null || true
    fi
  done <<<"$STALE_SHAS"
  if [ "$STOP_STALE" != true ]; then
    sd_log "  (not stopping them — pass --stop-stale to SIGKILL, or investigate why they did not exit after drain)"
  fi
else
  sd_log "no stale celeris@* units besides current=${OLD:-<none>} and new=$SHA12"
fi

# ---- 道具 ------------------------------------------------------------------

BACKUP="$SD_BACKUPS/$TS-pre-$SHA12.sqlite3"
backup_db() {
  sd_log "backing up the production DB (read-only) to $BACKUP"
  sqlite3 "file:$SD_DB?mode=ro" ".backup '$BACKUP'" || sd_die "sqlite3 .backup failed"
  sd_log "backup ok: $(du -h "$BACKUP" | cut -f1)"
}

# `poll_release <url> <json-path-of-release> <want> <role-path|-> <want-role|-> <timeout> [samples]` —
# SO_REUSEPORT で新旧が同じポートを共有するので、1 回の応答では足りない。
# 毎秒 5 回サンプルし、**全部**が期待どおりになったら成功（＝旧はもう listener を閉じている）。
poll_release() {
  local url="$1" rel_path="$2" want="$3" role_path="$4" want_role="$5" timeout="$6"
  local samples="${7:-5}"
  local waited=0 i all tmp got_rel got_role
  tmp="$(mktemp)"
  while [ "$waited" -lt "$timeout" ]; do
    all=true
    for ((i=0; i<samples; i++)); do
      if ! sd_http_get "$url" >"$tmp" 2>/dev/null; then all=false; break; fi
      sd_json_valid "$tmp" || { all=false; break; }
      got_rel="$(sd_json_get "$tmp" "$rel_path" 2>/dev/null || echo "")"
      [ "$got_rel" = "$want" ] || { all=false; break; }
      if [ "$role_path" != "-" ]; then
        got_role="$(sd_json_get "$tmp" "$role_path" 2>/dev/null || echo "")"
        [ "$got_role" = "$want_role" ] || { all=false; break; }
      fi
    done
    if [ "$all" = true ]; then
      rm -f "$tmp"
      if [ "$role_path" != "-" ]; then
        sd_log "poll $url: release=$want role=$want_role after ${waited}s ($samples/$samples samples)"
      else
        sd_log "poll $url: release=$want after ${waited}s ($samples/$samples samples)"
      fi
      return 0
    fi
    sleep 1
    waited=$((waited + 1))
  done
  sd_log "poll $url: gave up after ${timeout}s (last release=${got_rel:-?} role=${got_role:-?})"
  rm -f "$tmp"
  return 1
}

# `poll_instances_settled <sha12> <timeout>` — `daemon_instances` で、**生きている** active が `sha12` の
# 1 つだけになるまで待つ（ADR-0040 D4 付記 2026-10-05 D4f と付記 2026-10-05b）。
#
# 読む先は `GET /api/v1/releases`（Bearer が要る。`$SD_API_TOKEN_FILE`）。読めないときは DB の
# `daemon_instances` を**読み取り専用**で見る（status.sh と同じ）。生きている active の定義は
# `sd_instances_verdict`（役割 active・drained_at 無し・pid が生きている。draining・終了済みは数えない）。
#
# 本番 2026-10-05 03:44: 旧版はトークン無しで `/api/v1/releases` を読み、401 を「別の active がいる」と
# 記録して、引き継ぎ済みの新を止めた（active が 0 になり 1.5 分停止）。読めないことと二重 active を
# 区別するため、最後の判定を `SETTLE_VERDICT` / `SETTLE_SOURCE` / `SETTLE_SUMMARY` に残す。
SETTLE_VERDICT=unreadable
SETTLE_SOURCE=none
SETTLE_SUMMARY=""
poll_instances_settled() {
  local want="$1" timeout="$2" url="$SD_PROD_API/api/v1/releases"
  local waited=0 tmp verdict source warned_api=false warned_token=false
  tmp="$(mktemp)"
  if [ ! -r "$SD_API_TOKEN_FILE" ]; then
    sd_log "warning: $SD_API_TOKEN_FILE is not readable; GET $url will be tried without a token"
    warned_token=true
  fi
  while [ "$waited" -lt "$timeout" ]; do
    source=none
    if sd_http_get "$url" "$SD_API_TOKEN_FILE" >"$tmp" 2>/dev/null && sd_instances_rows "$tmp" >/dev/null; then
      source=api
    elif sd_instances_from_db "$SD_DB" "$tmp"; then
      source=db
      if [ "$warned_api" = false ]; then
        sd_log "GET $url is not readable (HTTP failure or no JSON; token file: $SD_API_TOKEN_FILE readable=$([ "$warned_token" = true ] && echo no || echo yes)); reading daemon_instances from the DB read-only instead"
        warned_api=true
      fi
    else
      : >"$tmp"
    fi
    verdict="$(sd_instances_verdict "$tmp" "$want")" || true
    SETTLE_VERDICT="$verdict"
    SETTLE_SOURCE="$source"
    SETTLE_SUMMARY="$(sd_instances_summary "$tmp")"
    if [ "$verdict" = settled ]; then
      rm -f "$tmp"
      sd_log "daemon_instances ($source): the only live active is $want after ${waited}s (the old instance is draining, drained or gone): $SETTLE_SUMMARY"
      return 0
    fi
    sleep 1
    waited=$((waited + 1))
  done
  rm -f "$tmp"
  case "$SETTLE_VERDICT" in
    two_active) sd_log "daemon_instances ($SETTLE_SOURCE): two live active instances after ${timeout}s: $SETTLE_SUMMARY" ;;
    new_not_active) sd_log "daemon_instances ($SETTLE_SOURCE): $want is not a live active after ${timeout}s: $SETTLE_SUMMARY" ;;
    no_active) sd_log "daemon_instances ($SETTLE_SOURCE): no live active instance after ${timeout}s: $SETTLE_SUMMARY" ;;
    *) sd_log "daemon_instances: not readable after ${timeout}s (API and DB); cannot tell whether the old instance is still active" ;;
  esac
  return 1
}

# `ensure_an_active_remains <sha12>` — 新を止めた**後**に呼ぶ（ADR-0040 付記 2026-10-05b）。
# 誰も `/api/v1/health` で `role=active` を答えなければ、旧は既に draining か消えている（止めたせいで
# active が 0 になった）ので、新を起こし直す。起こし直したら `RESTARTED_NEW=true`。
RESTARTED_NEW=false
ensure_an_active_remains() {
  local sha12="$1" wait="${SD_ACTIVE_RECHECK_WAIT:-10}" waited=0 tmp role rel
  tmp="$(mktemp)"
  while [ "$waited" -lt "$wait" ]; do
    if sd_http_get "$SD_PROD_API/api/v1/health" >"$tmp" 2>/dev/null; then
      role="$(sd_json_get "$tmp" role 2>/dev/null || echo "")"
      rel="$(sd_json_get "$tmp" release 2>/dev/null || echo "?")"
      if [ "$role" = active ]; then
        rm -f "$tmp"
        sd_log "an active celeris is still serving after stopping celeris@$sha12 (release=$rel)"
        return 0
      fi
    fi
    sleep 1
    waited=$((waited + 1))
  done
  rm -f "$tmp"
  sd_log "no active celeris answered $SD_PROD_API/api/v1/health for ${wait}s after stopping celeris@$sha12 (the old one is draining or gone); starting celeris@$sha12 again so that production is not left without an active instance"
  mark_promoting
  if systemctl --user start "celeris@$sha12"; then
    RESTARTED_NEW=true
    if poll_release "$SD_PROD_API/api/v1/health" release "$sha12" role active 60; then
      sd_log "celeris@$sha12 is active again; the links were not changed — rerun promote.sh $sha12 to finish (it resumes when the new one is already active)"
    else
      sd_log "warning: celeris@$sha12 was started again but did not report role=active within 60s; check it by hand"
    fi
  else
    sd_log "ERROR: could not start celeris@$sha12 again; production has no active celeris — start one by hand"
  fi
  return 1
}

# 0.0.0.0:7700 で LISTEN している node の pid（初回の移行で旧 GUI を止めるため）。
find_old_gui_pid() {
  local pid
  pid="$(ss -ltnp "sport = :$SD_PROD_GUI_PORT" 2>/dev/null | sed -n 's/.*pid=\([0-9]\+\).*/\1/p' | head -n 1)"
  [ -n "$pid" ] || return 1
  printf '%s' "$pid"
}

kill_grace_secs() {
  local v
  v="$(sed -n 's/^[[:space:]]*kill_grace_secs[[:space:]]*=[[:space:]]*\([0-9]\+\).*$/\1/p' "$SD_CONFIG" | head -n 1)"
  printf '%s' "${v:-10}"
}

update_links() {
  if [ -n "$OLD" ] && [ -d "$(sd_release_dir "$OLD")" ]; then
    sd_set_link "$SD_PREVIOUS" "$OLD"
    sd_log "previous -> releases/$OLD"
  fi
  sd_set_link "$SD_CURRENT" "$SHA12"
  sd_log "current  -> releases/$SHA12"
  # `current` が新を指したので、以後の起動は印なしで認可される（ADR-0040 付記 規則 1）。
  sd_clear_promoting "$SHA12"
  PROMOTING_WRITTEN=false
}

# `celeris@$SHA12` を start する直前に呼ぶ（ADR-0040 付記: 未昇格の release は handoff も migrate もしない）。
mark_promoting() {
  sd_write_promoting "$SHA12" promote.sh "$MODE"
  PROMOTING_WRITTEN=true
}

# ---- ライブ引き継ぎ --------------------------------------------------------

promote_live() {
  backup_db
  mark_promoting
  sd_log "systemctl --user start celeris@$SHA12"
  systemctl --user start "celeris@$SHA12" || sd_die "failed to start celeris@$SHA12"
  if ! poll_release "$SD_PROD_API/api/v1/health" release "$SHA12" role active "${SD_HANDOFF_WAIT:-60}"; then
    sd_log "handoff did not complete within ${SD_HANDOFF_WAIT:-60}s; stopping celeris@$SHA12 and leaving the old one alone"
    systemctl --user stop "celeris@$SHA12" || true
    # 付記 2026-10-05b: 止めた後に active が 0 なら新を起こし直す（旧が先に draining へ移っていた場合）。
    if ensure_an_active_remains "$SHA12"; then
      sd_die "live handoff failed: the new celeris did not become active (the old celeris is still serving; nothing was changed)"
    fi
    sd_die "live handoff failed: the new celeris did not become active in time, and no active celeris was left after stopping it; celeris@$SHA12 was started again (restarted=$RESTARTED_NEW). Check /api/v1/health and daemon_instances, then rerun promote.sh $SHA12"
  fi
  # ADR-0040 D4 付記 2026-10-05: 新が active でも、旧が draining（または消えた）ことを daemon_instances で
  # 確かめてから「handoff done」にする。
  # 付記 2026-10-05b: 新を止めるのは、**旧が生きた active のまま、新も active**（二重 active）と読めたときだけ。
  # 旧が draining に移った後に新を止めると active が 0 になる（本番 2026-10-05 03:44 の停止）。読めない・
  # 判断できないときは新を残し、人に知らせる（exit 1 → promote_failed.json → GUI のバナー）。
  if ! poll_instances_settled "$SHA12" "${SD_SETTLE_WAIT:-60}"; then
    case "$SETTLE_VERDICT" in
      two_active)
        sd_log "the old celeris is still a live active next to the new one ($SETTLE_SUMMARY); stopping celeris@$SHA12 so that the old one keeps serving"
        systemctl --user stop "celeris@$SHA12" || true
        if ensure_an_active_remains "$SHA12"; then
          sd_die "live handoff failed: two active instances (the old celeris was still active next to the new one; the new celeris@$SHA12 was stopped; the old one keeps serving; links unchanged)"
        fi
        sd_die "live handoff failed: two active instances were read, but after stopping celeris@$SHA12 no active celeris answered; celeris@$SHA12 was started again (restarted=$RESTARTED_NEW). Check daemon_instances and rerun promote.sh $SHA12"
        ;;
      *)
        sd_log "cannot confirm that the old celeris released active (verdict=$SETTLE_VERDICT source=$SETTLE_SOURCE: ${SETTLE_SUMMARY:-?}); leaving celeris@$SHA12 running (stopping it could leave production without an active instance)"
        case "$SETTLE_VERDICT" in
          unreadable) sd_die "live handoff not confirmed: daemon_instances could not be read (GET /api/v1/releases with $SD_API_TOKEN_FILE and the DB $SD_DB). The new celeris@$SHA12 is active per /api/v1/health and was left running; the links were not changed. Check with status.sh, then rerun promote.sh $SHA12 (it resumes when the new one is already active)" ;;
          no_active) sd_die "live handoff not confirmed: /api/v1/health says $SHA12 is active but daemon_instances shows no live active ($SETTLE_SUMMARY). The new celeris@$SHA12 was left running; the links were not changed. Check daemon_instances, then rerun promote.sh $SHA12" ;;
          *) sd_die "live handoff not confirmed: daemon_instances shows the old celeris still active and $SHA12 not active ($SETTLE_SUMMARY) although /api/v1/health answered as $SHA12. The new celeris@$SHA12 was left running; the links were not changed. Check daemon_instances, then rerun promote.sh $SHA12" ;;
        esac
        ;;
    esac
  fi
  sd_log "handoff done: the new celeris is the only live active and the old one is draining (or gone)"

  systemctl --user enable "celeris@$SHA12" || sd_log "warning: enable celeris@$SHA12 failed"
  if [ -n "$OLD" ]; then
    systemctl --user disable "celeris@$OLD" || sd_log "warning: disable celeris@$OLD failed"
  fi

  sd_log "systemctl --user start celeris-gui@$SHA12"
  systemctl --user start "celeris-gui@$SHA12" || sd_die "failed to start celeris-gui@$SHA12 (celeris is already the new one)"
  # GUIは旧も応答し続ける。新からの応答を確認 → 旧を止める → 全応答が新か確認、の順。
  if ! poll_release "http://127.0.0.1:$SD_PROD_GUI_PORT/healthz" release "$SHA12" - - 60 1; then
    systemctl --user stop "celeris-gui@$SHA12" || true
    sd_die "the new GUI did not take over :$SD_PROD_GUI_PORT within 60s (celeris is already the new one; fix the GUI by hand)"
  fi
  if [ -n "$OLD" ] && systemctl --user is-active --quiet "celeris-gui@$OLD"; then
    sd_log "systemctl --user stop celeris-gui@$OLD"
    systemctl --user stop "celeris-gui@$OLD" || sd_log "warning: stop celeris-gui@$OLD failed"
  fi
  if ! poll_release "http://127.0.0.1:$SD_PROD_GUI_PORT/healthz" release "$SHA12" - - 60; then
    if [ -n "$OLD" ]; then systemctl --user start "celeris-gui@$OLD" || true; fi
    systemctl --user stop "celeris-gui@$SHA12" || true
    sd_die "GUI handoff could not be confirmed; restored the old GUI"
  fi
  systemctl --user enable "celeris-gui@$SHA12" || sd_log "warning: enable celeris-gui@$SHA12 failed"
  if [ -n "$OLD" ]; then
    systemctl --user disable "celeris-gui@$OLD" || sd_log "warning: disable celeris-gui@$OLD failed"
  fi

  update_links
  sd_log "live handoff complete. The old celeris keeps draining its runs; watch it with status.sh."
}

# ---- 停止 → 起動（初回の移行はこれ） --------------------------------------

promote_stop_start() {
  local old_pid grace waited old_gui_pid
  grace="$(kill_grace_secs)"

  if old_pid="$(sd_resolve_old_daemon_pid "$OLD")"; then
    sd_log "old celeris pid=$old_pid (systemd MainPID of celeris@${OLD:-<none>}, or an exact /proc match on: celeris --config $SD_CONFIG if systemd does not know this release yet; Phase 119 D3)"
    # 実機 2026-09-19 22:42: SIGTERM の後、旧 celeris が ext4 のジャーナル待ち（jbd2_log_wait_commit、D 状態）で
    # 2 分半かかり、20 秒で諦めた promote.sh が「何も変えていない」と言って止まった。しかし SIGTERM は届いていて
    # API は閉じていたので、本番は新も旧も無い状態で 3 分止まった。SIGTERM を送ったら**戻れない**ので、
    # 長く待ち（SD_STOP_WAIT、既定 300 秒）、それでも生きていて API だけ閉じているなら警告して先へ進む
    # （DB は SQLite のロックで守られる。旧は書き終えて flush しているだけ）。
    stop_wait="${SD_STOP_WAIT:-300}"
    sd_log "SIGTERM $old_pid; waiting up to ${stop_wait}s (kill_grace_secs=$grace)"
    kill -TERM "$old_pid"
    waited=0
    while kill -0 "$old_pid" 2>/dev/null && [ "$waited" -lt "$stop_wait" ]; do
      sleep 1
      waited=$((waited + 1))
      if [ $((waited % 15)) -eq 0 ]; then
        sd_log "still exiting after ${waited}s: state=$(awk '{print $3}' "/proc/$old_pid/stat" 2>/dev/null || echo '?') wchan=$(cat "/proc/$old_pid/wchan" 2>/dev/null || echo '?')"
      fi
    done
    if kill -0 "$old_pid" 2>/dev/null; then
      if [ "$(sd_http_status "$SD_PROD_API/api/v1/health")" = 200 ]; then
        sd_die "old celeris (pid $old_pid) is still serving after ${stop_wait}s; refusing to start a second daemon (SIGTERM was sent — watch it and rerun)"
      fi
      sd_log "warning: old celeris (pid $old_pid) is still exiting after ${stop_wait}s but its API is closed; continuing (SQLite locking protects the DB)"
    else
      sd_log "old celeris exited after ${waited}s"
    fi
  elif [ -n "$OLD" ] && systemctl --user is-active --quiet "celeris@$OLD"; then
    sd_log "systemctl --user stop celeris@$OLD"
    systemctl --user stop "celeris@$OLD" || sd_die "failed to stop celeris@$OLD"
  else
    sd_log "no running celeris found; starting the new one on a cold DB"
  fi

  # 旧が止まってからバックアップを取る（ADR-0040 D4 の順。引き継ぎ中の仕事も入る）。
  backup_db

  if [ -n "$PRE_START" ]; then
    sd_log "pre-start hook: $PRE_START $SHA12 $REL $SD_DB $SD_CONFIG"
    if ! "$PRE_START" "$SHA12" "$REL" "$SD_DB" "$SD_CONFIG" >>"$SD_LOG_FILE" 2>&1; then
      sd_log "pre-start hook failed; restoring the DB from $BACKUP"
      rm -f "$SD_DB-wal" "$SD_DB-shm"
      cp -p "$BACKUP" "$SD_DB" || sd_log "warning: could not restore $SD_DB from $BACKUP"
      restore_old_daemon
      sd_die "pre-start hook failed (see $SD_LOG_FILE). The DB was restored from the pre-promotion backup and the old unit was restarted"
    fi
    sd_log "pre-start hook ok"
  fi

  mark_promoting
  sd_log "systemctl --user start celeris@$SHA12"
  if ! systemctl --user start "celeris@$SHA12"; then
    sd_log "start celeris@$SHA12 failed"
    restore_old_daemon
    sd_die "could not start celeris@$SHA12"
  fi
  if ! sd_wait_http_200 "$SD_PROD_API/api/v1/health" 60; then
    sd_log "no 200 from $SD_PROD_API/api/v1/health within 60s"
    systemctl --user stop "celeris@$SHA12" || true
    restore_old_daemon
    sd_die "the new celeris did not become healthy. The DB was NOT restored (the schema may have moved forward): use rollback.sh --restore-db if you need the pre-promotion DB ($BACKUP)"
  fi
  sd_http_get "$SD_PROD_API/api/v1/health" >"$SD_BACKUPS/promote-$TS.health-after.json" || true
  GOT_SCHEMA="$(sd_json_get "$SD_BACKUPS/promote-$TS.health-after.json" schema_version || echo "?")"
  if [ "$GOT_SCHEMA" != "$WANT_SCHEMA" ]; then
    sd_log "health.schema_version=$GOT_SCHEMA but the release says $WANT_SCHEMA"
    systemctl --user stop "celeris@$SHA12" || true
    restore_old_daemon
    sd_die "schema_version mismatch after start. The DB was NOT restored ($BACKUP is the pre-promotion copy)"
  fi
  sd_log "new celeris is healthy: schema_version=$GOT_SCHEMA"
  systemctl --user enable "celeris@$SHA12" || sd_log "warning: enable celeris@$SHA12 failed"
  if [ -n "$OLD" ]; then
    systemctl --user disable "celeris@$OLD" || sd_log "warning: disable celeris@$OLD failed"
  fi

  # GUI: 初回の移行では systemd の外で動いている `node server.js` を止める。
  if old_gui_pid="$(find_old_gui_pid)"; then
    sd_log "old GUI pid=$old_gui_pid on :$SD_PROD_GUI_PORT; SIGTERM"
    kill -TERM "$old_gui_pid" || true
    waited=0
    while kill -0 "$old_gui_pid" 2>/dev/null && [ "$waited" -lt 20 ]; do
      sleep 1
      waited=$((waited + 1))
    done
    if kill -0 "$old_gui_pid" 2>/dev/null; then sd_log "warning: old GUI pid $old_gui_pid is still alive"; fi
  else
    sd_log "nothing is listening on :$SD_PROD_GUI_PORT"
  fi
  sd_log "systemctl --user start celeris-gui@$SHA12"
  systemctl --user start "celeris-gui@$SHA12" || sd_die "celeris is the new one, but celeris-gui@$SHA12 did not start"
  if ! sd_wait_http_200 "http://127.0.0.1:$SD_PROD_GUI_PORT/healthz" 60; then
    sd_die "celeris is the new one, but the GUI did not answer /healthz on :$SD_PROD_GUI_PORT within 60s"
  fi
  systemctl --user enable "celeris-gui@$SHA12" || sd_log "warning: enable celeris-gui@$SHA12 failed"
  if [ -n "$OLD" ]; then
    systemctl --user disable "celeris-gui@$OLD" || sd_log "warning: disable celeris-gui@$OLD failed"
  fi

  update_links
  sd_log "stop-start promotion complete"
}

# 失敗したときに旧 unit を起こし直す（unit が無い＝初回の移行なら、人に任せる）。
restore_old_daemon() {
  if [ -n "$OLD" ] && systemctl --user cat "celeris@$OLD" >/dev/null 2>&1; then
    sd_log "restarting the old unit: systemctl --user start celeris@$OLD"
    systemctl --user start "celeris@$OLD" || sd_log "warning: could not restart celeris@$OLD"
  else
    sd_log "there is no old systemd unit to restart (this was the first migration)."
    sd_log "the old binary is at ${SD_REPO}/target/debug/celeris — a human decides what to start."
  fi
}

case "$MODE" in
  live) promote_live ;;
  stop-start) promote_stop_start ;;
esac

# ---- web/ の追従（ADR-0081 / web ADR-W3 付記 2026-10-02 (C)）----------------
#
# celeris と gui の切替が済んだ後に、旧 release の celeris-web@ が動いていれば新 release へ移す。
# web-follow.sh は常に exit 0 の設計だが、万一失敗しても警告だけにして昇格は失敗にしない。
# celeris@ の unit には触れない（web は daemon の handoff を起こさない）。
WEB_FOLLOW="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/web-follow.sh"
if [ -x "$WEB_FOLLOW" ]; then
  "$WEB_FOLLOW" "$SHA12" "$OLD" 2>&1 | tee -a "$SD_LOG_FILE" >&2 \
    || sd_log "warning: web-follow.sh $SHA12 ${OLD:-<none>} failed (promotion is not affected)"
else
  sd_log "warning: $WEB_FOLLOW not found; web was not moved to $SHA12"
fi

# ---- promoted.json（ADR-0041 D3）------------------------------------------
#
# 「このリリースが、いつ、どの版から、どうやって昇格したか」を**リリースの中に**残す。
# `GET /releases` の `promoted_at` はこれを読むだけ（Phase 48 の逸脱 2「どこにも書かれていない」の解消）。
# **git リポジトリには触れない**（人のチェックアウトを機械が fast-forward しない。ADR-0041 D3）。
# `main` に反映されているかは `GET /releases` の `on_main` が読み取りで見せる。
# preview の JSON から `{"releases": [...], "tasks": [{task_id,title}], "complete": bool}`。無ければ null。
promoted_included_json() {
  if [ "$PREVIEW_OK" != true ] || ! command -v python3 >/dev/null 2>&1; then printf 'null'; return 0; fi
  python3 - "$PREVIEW_JSON" <<'PY' 2>/dev/null || printf 'null'
import json, sys
p = json.load(open(sys.argv[1], encoding="utf-8"))
print(json.dumps({
    "releases": [r["sha12"] for r in p.get("releases", [])],
    "tasks": [{"task_id": t["task_id"], "title": t.get("title")} for t in p.get("tasks", [])],
    "complete": bool(p.get("complete")),
}, ensure_ascii=False))
PY
}
# ADR 2026-10-08-browser-prod-enablement D1.3: browser の適合台帳は無くても昇格を止めない。release の
# `browser/ledger-status.json` の {ok, code} を写す（無ければ ok=false, code=missing）。
BROWSER_LEDGER_JSON='{"ok": false, "code": "missing"}'
if [ -f "$REL/browser/ledger-status.json" ] && command -v python3 >/dev/null 2>&1; then
  BROWSER_LEDGER_JSON="$(python3 - "$REL/browser/ledger-status.json" <<'PY' 2>/dev/null || printf '{"ok": false, "code": "invalid"}'
import json, sys
d = json.load(open(sys.argv[1], encoding="utf-8"))
print(json.dumps({"ok": bool(d.get("ok")), "code": str(d.get("code") or "unknown")}))
PY
)"
fi
sd_log "browser_ledger: $BROWSER_LEDGER_JSON (not required to promote)"
{
  printf '{\n'
  printf '  "promoted_at": %s,\n' "$(sd_json_str "$(sd_ts)")"
  printf '  "mode": %s,\n' "$(sd_json_str "$MODE")"
  if [ -n "$OLD" ]; then
    printf '  "from": %s,\n' "$(sd_json_str "$OLD")"
  else
    printf '  "from": null,\n'
  fi
  printf '  "browser_ledger": %s,\n' "$BROWSER_LEDGER_JSON"
  printf '  "included": %s\n' "$(promoted_included_json)"
  printf '}\n'
} >"$REL/promoted.json"
rm -f "$PREVIEW_JSON" 2>/dev/null || true
sd_log "promoted.json: $REL/promoted.json (mode=$MODE from=${OLD:-<none>})"

sd_log "promoted $SHA12 (mode=$MODE). backup: $BACKUP"
# 昇格前・rollback 前 backup の保持（ADR 2026-10-10-local-disk-growth-paths D4）。失敗しても昇格は失敗にしない。
PRUNE_BACKUPS="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/prune-backups.sh"
if sh "$PRUNE_BACKUPS" "$SD_BACKUPS" >>"$SD_LOG_FILE" 2>&1; then
  sd_log "prune-backups: ok (see $SD_LOG_FILE)"
else
  sd_log "warning: prune-backups.sh $SD_BACKUPS failed (exit $?); no backup was pruned or only some were — see $SD_LOG_FILE"
fi
sd_log "the working checkout was NOT touched. If main is behind this release, a human runs: git -C $SD_REPO merge --ff-only $SHA12"
