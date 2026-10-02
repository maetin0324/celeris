#!/bin/sh
# phase3_* (browser control) の e2e 試験を、CPU 負荷と並走する cargo test を
# かけた状態で繰り返し実行し、時刻依存の flaky が無いことを確かめる。
# 引数なしで `sh scripts/dev/stress-e2e-phase3.sh` として呼ぶ。
# 全部通れば exit 0、1 回でも落ちれば exit 1。負荷プロセスは trap で必ず片付ける。
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$ROOT_DIR"

ITERATIONS=${STRESS_E2E_PHASE3_ITERATIONS:-20}
PARALLEL=${STRESS_E2E_PHASE3_PARALLEL:-8}
LOAD_SECONDS=${STRESS_E2E_PHASE3_LOAD_SECONDS:-1800}

WORK_DIR=$(mktemp -d /tmp/stress-e2e-phase3.XXXXXX)
LOAD_PIDS=""

cleanup() {
  set +e
  for pid in $LOAD_PIDS; do
    kill "$pid" >/dev/null 2>&1
  done
  for pid in $LOAD_PIDS; do
    wait "$pid" >/dev/null 2>&1
  done
  rm -rf "$WORK_DIR"
}
trap cleanup EXIT INT TERM

NPROC=$(nproc 2>/dev/null || echo 4)

echo "[stress-e2e-phase3] starting $NPROC CPU-burn load processes (cap ${LOAD_SECONDS}s)"
n=0
while [ "$n" -lt "$NPROC" ]; do
  timeout "$LOAD_SECONDS" sh -c 'while :; do :; done' &
  LOAD_PIDS="$LOAD_PIDS $!"
  n=$((n + 1))
done

echo "[stress-e2e-phase3] starting background cargo test -p task-dispatch --lib load (cap ${LOAD_SECONDS}s)"
timeout "$LOAD_SECONDS" sh -c '
  cd "'"$ROOT_DIR"'"
  while :; do
    cargo test -p task-dispatch --lib >/dev/null 2>&1 || true
  done
' &
LOAD_PIDS="$LOAD_PIDS $!"

echo "[stress-e2e-phase3] building workspace bins (fixture needs target/debug/celeris and celerisctl)"
cargo build --workspace --bins >"$WORK_DIR/build.log" 2>&1 || {
  echo "[stress-e2e-phase3] FAIL: workspace bins build failed"
  cat "$WORK_DIR/build.log"
  exit 1
}

echo "[stress-e2e-phase3] building tests/e2e (phase3_) once before the timed loop"
cargo test -p e2e --test api_scenarios phase3_ --no-run >"$WORK_DIR/build.log" 2>&1 || {
  echo "[stress-e2e-phase3] FAIL: build failed"
  cat "$WORK_DIR/build.log"
  exit 1
}

echo "[stress-e2e-phase3] running $ITERATIONS serial iterations under load"
i=1
while [ "$i" -le "$ITERATIONS" ]; do
  log="$WORK_DIR/serial-$i.log"
  if cargo test -p e2e --test api_scenarios phase3_ >"$log" 2>&1; then
    echo "[stress-e2e-phase3] serial iteration $i/$ITERATIONS: ok"
  else
    echo "[stress-e2e-phase3] FAIL: serial iteration $i/$ITERATIONS"
    cat "$log"
    exit 1
  fi
  i=$((i + 1))
done

echo "[stress-e2e-phase3] running $PARALLEL concurrent processes under load"
j=1
PAR_PIDS=""
while [ "$j" -le "$PARALLEL" ]; do
  (cargo test -p e2e --test api_scenarios phase3_ >"$WORK_DIR/parallel-$j.log" 2>&1) &
  PAR_PIDS="$PAR_PIDS $!"
  j=$((j + 1))
done

fail=0
for pid in $PAR_PIDS; do
  if ! wait "$pid"; then
    fail=1
  fi
done

if [ "$fail" -ne 0 ]; then
  echo "[stress-e2e-phase3] FAIL: one or more of the $PARALLEL concurrent processes failed"
  k=1
  while [ "$k" -le "$PARALLEL" ]; do
    echo "--- parallel-$k.log ---"
    cat "$WORK_DIR/parallel-$k.log" 2>/dev/null || true
    k=$((k + 1))
  done
  exit 1
fi
echo "[stress-e2e-phase3] all $PARALLEL concurrent processes: ok"

echo "[stress-e2e-phase3] OK: $ITERATIONS serial iterations + $PARALLEL concurrent processes all passed under load"
exit 0
