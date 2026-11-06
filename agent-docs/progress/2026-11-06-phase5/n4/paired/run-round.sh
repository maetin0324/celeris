#!/bin/bash
D=$TMPDIR/n4/$ROUND
mkdir -p "$D"; rm -f "$D"/*
date -u +%FT%TZ > "$D/start.txt"
( while true; do echo "$(date -u +%FT%TZ) $(cat /proc/loadavg)" >> "$D/loadavg.log"; sleep 30; done ) &
LP=$!
pids=()
for arm in before after; do for cond in off shadow; do
  TICK_SCALE_BLOCKED=${SIZES:-0,600,2000,10000} TICK_SCALE_TICKS=5 TICK_SCALE_DISPATCHES=5 TICK_SCALE_COND=$cond \
  CELERIS_SCRIPT_REQUIRE_BUNDLE=1 TICK_SCALE_RAW="$D/$arm-$cond.jsonl" \
  "$TMPDIR/n4/$arm.bin" --exact tick_scale_baseline --nocapture > "$D/$arm-$cond.log" 2>&1 &
  pids+=($!); echo "$arm-$cond $!" >> "$D/pids"
done; done
rc=0
for p in "${pids[@]}"; do wait "$p"; e=$?; echo "pid $p exit=$e" >> "$D/exits"; [ $e -ne 0 ] && rc=1; done
kill $LP
date -u +%FT%TZ > "$D/end.txt"
exit $rc
