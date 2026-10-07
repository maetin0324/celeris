#!/usr/bin/env bash
# Fable rerun 3, diagnostic attempt 2: unpatched script and configs, launcher patched (commands/diag2-launcher.patch), plus a
# read-only root inotify observer that records the launcher action requests/results (agent-browser
# JSON replies) so the navigate failure has a cause. Only the launcher binary differs.
EV=$(cd "$(dirname "$0")/.." && pwd)
OUT=/var/tmp/cb-1001/action-observer.jsonl
sudo -n python3 "$EV/commands/action-observer.py" "$OUT" &
OBS=$!
sleep 1
bash "$EV/commands/run-check-diag2-inner.sh"; rc=$?
sudo -n kill $OBS 2>/dev/null; sudo -n pkill -f action-observer.py 2>/dev/null
sudo -n chown "$(id -u)" "$OUT" 2>/dev/null
exit $rc
