#!/usr/bin/env bash
# promote から web-follow への配線と、ADR-0135 の追従条件を検証する。
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
bash "$here/tests/web_follow_health_gate.sh"
grep -q 'web-follow.sh' "$here/promote.sh"
grep -q '"\$WEB_FOLLOW" "\$SHA12" "\$OLD"' "$here/promote.sh"
unit="$here/../../deploy/systemd/celeris-web@.service"
! grep -q '^Wants=celeris@' "$unit"
echo 'promote_web_follows_release: all ok'
