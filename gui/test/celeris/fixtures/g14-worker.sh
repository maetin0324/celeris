#!/bin/sh
# gui/scripts/browser-live-e2e.sh 用の fake ワーカー（gui/e2e/g14-browser-live.spec.ts）。LLM は呼ばない。
# - 起動ごとに `markers/<task_id>.<pid>` を作る（spec が run の回数 = 再開を数える）
# - task を Running に留めるため、5 秒ごとに progress を出しながら最大 G14_WORKER_SECS 秒（既定 600）眠る。
#   spec が wait を開くと task は Blocked になり、dispatcher がこの run を止める（止めなければ最後に done）。
set -u
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
RUN=$(mktemp)
cat >"$RUN"
eval "$(node "$HERE/read-run-request.mjs" <"$RUN")"
rm -f "$RUN"
mkdir -p "$HERE/markers"
date -u +%Y-%m-%dT%H:%M:%SZ > "$HERE/markers/$TASK_ID.$$"
echo '{"type":"progress","msg":"g14 fake worker started"}'
secs=${G14_WORKER_SECS:-600}
i=0
while [ "$i" -lt "$secs" ]; do
  sleep 5
  i=$((i + 5))
  echo '{"type":"progress","msg":"g14 fake worker waiting"}'
done
echo '{"type":"done","summary":"g14 fake done","evidence":[]}'
