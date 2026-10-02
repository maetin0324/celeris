#!/usr/bin/env bash
# scripts/selfdeploy/web-follow.sh <new_sha12> <old_sha12> — 昇格の後に web/ の gateway を新 release へ追従させる
# （ADR-0081 / web ADR-W3 の付記 2026-10-02 (C)(D)）。promote.sh が昇格に成功した後に呼ぶ。人が直接呼んでもよい。
#
#   celeris-web@<old> が active のときだけ動く。新 release の gate.json の `web.ok` が true で
#   `web/app/server/index.js` があれば、celeris-web@<new> を start → enable、旧を stop → disable する。
#   新の起動に失敗したら新を stop して旧を start し直す。条件を満たさなければ何もせず理由をログに出す。
#   どの場合も exit 0（web の追従の失敗で昇格を失敗にしない。D3/H7 の非 blocking）。
#   celeris@ の unit には一切触れない（web は daemon の handoff を起こさない）。
set -uo pipefail

SD_PROG=web-follow
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

NEW="${1:-}"
OLD="${2:-}"
releases="$SD_RELEASES"

if [ -z "$NEW" ]; then
  sd_log "usage: web-follow.sh <new_sha12> <old_sha12>; no new sha given — nothing to do"
  exit 0
fi
if [ -z "$OLD" ]; then
  sd_log "no previous release (old sha is empty); web is not following — start celeris-web@$NEW by hand if wanted"
  exit 0
fi
if [ "$OLD" = "$NEW" ]; then
  sd_log "old and new are the same release ($NEW); nothing to do for web"
  exit 0
fi
if ! systemctl --user is-active --quiet "celeris-web@$OLD"; then
  sd_log "celeris-web@$OLD is not active; web is not following (nothing changed)"
  exit 0
fi

rel="$releases/$NEW"
web_ok="$(python3 - "$rel/gate.json" <<'PY' 2>/dev/null || echo false
import json, sys
try:
    with open(sys.argv[1], encoding="utf-8") as fh:
        gate = json.load(fh)
except Exception:
    print("false")
    sys.exit(0)
web = gate.get("web") if isinstance(gate, dict) else None
print("true" if isinstance(web, dict) and web.get("ok") is True else "false")
PY
)"
if [ "$web_ok" != true ]; then
  sd_log "gate.json of $NEW does not have web.ok=true ($rel/gate.json); keeping celeris-web@$OLD (nothing changed)"
  exit 0
fi
if [ ! -f "$rel/web/app/server/index.js" ]; then
  sd_log "$rel/web/app/server/index.js is missing; keeping celeris-web@$OLD (nothing changed)"
  exit 0
fi

sd_log "systemctl --user start celeris-web@$NEW"
if ! systemctl --user start "celeris-web@$NEW"; then
  sd_log "warning: start celeris-web@$NEW failed; stopping it and restarting celeris-web@$OLD"
  systemctl --user stop "celeris-web@$NEW" || sd_log "warning: stop celeris-web@$NEW failed"
  systemctl --user start "celeris-web@$OLD" || sd_log "warning: restart celeris-web@$OLD failed"
  exit 0
fi
systemctl --user enable "celeris-web@$NEW" || sd_log "warning: enable celeris-web@$NEW failed"
sd_log "systemctl --user stop celeris-web@$OLD"
systemctl --user stop "celeris-web@$OLD" || sd_log "warning: stop celeris-web@$OLD failed"
systemctl --user disable "celeris-web@$OLD" || sd_log "warning: disable celeris-web@$OLD failed"
sd_log "web follows the release: celeris-web@$NEW (was celeris-web@$OLD)"
exit 0
