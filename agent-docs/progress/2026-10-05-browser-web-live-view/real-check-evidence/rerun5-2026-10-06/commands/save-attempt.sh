#!/usr/bin/env bash
# Fable rerun 5: copy one attempt's evidence out of /var/tmp/cb-1001 (no secrets) into attempts/<name>.
# usage: save-attempt.sh <name> <script-output-file> <start-file>
set -euo pipefail
EV=/local/celeris/data/workspaces/01M46W97H391DSFW1XJ745W0G9/repos/agent-platform/agent-docs/progress/2026-10-05-browser-web-live-view/real-check-evidence/rerun5-2026-10-06
E=/var/tmp/cb-1001
A=$EV/attempts/$1
mkdir -p "$A"
for f in checks.json health.json web-health.json launcher.log daemon.log web.log page.log denied-page.log \
         live-upstream.log live-upstream-frames.jsonl allowed-page.png egress-denied.json harness.log page-response.html; do
  [[ -e $E/$f ]] && cp "$E/$f" "$A/$f"
done
cp "$2" "$A/script-output.txt"; cp "$3" "$A/start.txt"
sudo -n find "$E/launcher-state" -printf '%M %u:%g %p\n' > "$A/launcher-state-after-stop.txt"
# run-browser dir (events/policy) of the run, if any
d=$(find "$E/w" -type d -name browser -path '*run*' 2>/dev/null | head -1 || true)
[[ -n $d ]] && mkdir -p "$A/run-browser" && find "$d" -maxdepth 1 -type f \( -name '*.json' -o -name '*.jsonl' \) -exec cp {} "$A/run-browser/" \; || true
T=$(python3 -c 'import json,sys;print(next((r["task_id"] for r in json.load(open(sys.argv[1])) if "task_id" in r),""))' "$E/checks.json" 2>/dev/null || true)
[[ -n $T && -f $E/test.sqlite3 ]] && sqlite3 -json "$E/test.sqlite3" "select * from events where task_id='$T' order by id" > "$A/task-events.json" 2>/dev/null || true
# redaction check
for s in "$(cat $E/api.token)" "$(cat $E/web.password)"; do grep -rlF -- "$s" "$A" && { echo "SECRET LEAK in $A" >&2; exit 1; }; done
grep -rl 'PRIVATE KEY' "$A" && { echo "KEY LEAK" >&2; exit 1; }
ls -la "$A"
