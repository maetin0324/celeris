#!/usr/bin/env bash
# Fable rerun 4, diagnostic attempt 1: same code/binaries/configs/harness as attempt 1.
# Script copy with commands/diag1-script.patch (diagnostic only, NOT a proposed fix as is):
#  - the egress denial collector starts before the API driver and the driver waits for it
#    before opening/denying the decision wait (deny fails the task and cancels the run/session);
#  - the collector dumps all current-session records to collector.log, allows 180 s, and accepts
#    kind ip_literal as well as private_address.
# Plus the read-only root inotify observer of launcher action request/result files.
EV=$(cd "$(dirname "$0")/.." && pwd)
OUT=/var/tmp/cb-1001/action-observer.jsonl
sudo -n python3 "$EV/commands/action-observer.py" "$OUT" &
sleep 1
SCRIPT=/var/tmp/fable-rerun4-src/scripts/dev/browser-web-live-check-diag1.sh bash "$EV/commands/run-check-as-documented.sh"; rc=$?
sudo -n pkill -f action-observer.py 2>/dev/null
sudo -n chown "$(id -u)" "$OUT" 2>/dev/null
exit $rc
