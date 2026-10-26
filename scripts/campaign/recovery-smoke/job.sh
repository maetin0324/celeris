#!/bin/sh
# Recovery smoke payload (ADR 2026-10-25-recovery-scheduler-backend I15 (4)).
# 引数なし。環境: RECOVERY_SMOKE_DIR（測定 dir の絶対 path）、PBS_JOBID、
# RECOVERY_SMOKE_RELEASE_TIMEOUT（release を待つ上限秒。既定 240）。
# 書くのは "$RECOVERY_SMOKE_DIR/out/$PBS_JOBID/" の下だけ。PBS 呼び出し・CPU 負荷・network なし。
set -eu

dir=${RECOVERY_SMOKE_DIR:?RECOVERY_SMOKE_DIR is not set}
job=${PBS_JOBID:?PBS_JOBID is not set}
timeout=${RECOVERY_SMOKE_RELEASE_TIMEOUT:-240}

case $dir in
/*) ;;
*) echo "job.sh: RECOVERY_SMOKE_DIR must be absolute: $dir" >&2; exit 2 ;;
esac
case $dir in
*[!A-Za-z0-9._/-]* | */../* | */.. | *//*) echo "job.sh: bad RECOVERY_SMOKE_DIR: $dir" >&2; exit 2 ;;
esac
case $job in
'' | *[!A-Za-z0-9.@_-]*) echo "job.sh: bad PBS_JOBID: $job" >&2; exit 2 ;;
esac
case $timeout in
'' | *[!0-9]*) echo "job.sh: bad RECOVERY_SMOKE_RELEASE_TIMEOUT: $timeout" >&2; exit 2 ;;
esac

attempt=$dir/out/$job
mkdir -p -- "$attempt"

# 1. 入力の写しと sha256
cp -- "$dir/input.bin" "$attempt/input.copy"
in_sha=$(sha256sum < "$attempt/input.copy" | cut -d ' ' -f 1)
printf '%s\n' "$in_sha" > "$attempt/input.sha256"

# 2. release/<PBS_JOBID> を sleep 5 間隔で上限まで待つ
waited=0
while [ ! -e "$dir/release/$job" ]; do
    if [ "$waited" -ge "$timeout" ]; then
        echo "job.sh: release/$job did not appear within ${timeout}s" >&2
        exit 3
    fi
    step=5
    if [ $((timeout - waited)) -lt "$step" ]; then
        step=$((timeout - waited))
    fi
    sleep "$step"
    waited=$((waited + step))
done

# 3. out.bin（入力だけから決まる。job ID・時刻・host を含めない）
tmp=$attempt/out.bin.partial
{
    printf 'celeris.recovery-smoke-out/1\n'
    printf 'input_sha256 %s\n' "$in_sha"
    cat -- "$attempt/input.copy"
} > "$tmp"
mv -- "$tmp" "$attempt/out.bin"
