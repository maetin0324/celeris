#!/bin/sh
# 測定環境 launcher（POSIX sh）。既存の celeris-campaign / celeris CLI を包む薄い入口で、判定は全部 CLI 側にある。
# 単一の設定ファイル（--env <file>、KEY=VALUE の sh 形式）から --api-url・--measure-config 等を生成する。
# 本番 daemon・systemd・本番 config・本番 DB には触れない。停止は launcher が起動した pid（$STATE_DIR/launcher.pid）だけ。
#
#   measure-launcher.sh <prepare|start|preflight|watch|grade|export|stop> --env <file>
#
# 各 subcommand は再実行できる。watch は台帳から再開し、毎回 preflight（env-check を含む）と env-lock の照合を通ってから始まる。
set -eu

usage() {
    echo "usage: $0 <prepare|start|preflight|watch|grade|export|stop> --env <measure-launcher.env>" >&2
    exit 2
}

die() {
    echo "measure-launcher: $*" >&2
    exit 1
}

[ $# -ge 1 ] || usage
SUB=$1
shift
ENV_FILE=
while [ $# -gt 0 ]; do
    case $1 in
        --env) [ $# -ge 2 ] || usage; ENV_FILE=$2; shift 2 ;;
        *) usage ;;
    esac
done
[ -n "$ENV_FILE" ] || usage
[ -f "$ENV_FILE" ] || die "env file not found: $ENV_FILE"
ENV_DIR=$(cd "$(dirname "$ENV_FILE")" && pwd)

# ---- 設定の読み込み（相対 path は env ファイルの dir 基準）
CAMPAIGN_BIN=celeris-campaign
CELERIS_BIN=
MANIFEST= DAEMON_CONFIG= NORMAL_CONFIG= CANARIES= TASK_TEMPLATE= OUT= STATE_DIR=
RELEASE= RUN_PATH= A_PROGRAM=claude A_ACCOUNT= WORKER_PROGRAMS= MEASUREMENT_ID=
MATERIALS_SRC= KB_SRC= REPO_SRC= INPUTS_SRC= CASES_ROOT= GRADER_ROOT= GRADE_CASES_ROOT=
RECORDS= EXECUTOR=celeris INTERVAL_SECS=60 MAX_ITERATIONS=1440 HEALTH_WAIT_SECS=60
# shellcheck disable=SC1090
. "$ENV_FILE"

abs() {
    case $1 in
        /*) printf '%s\n' "$1" ;;
        "~"/*) printf '%s\n' "$HOME/${1#"~"/}" ;;
        *) printf '%s\n' "$ENV_DIR/$1" ;;
    esac
}

need() {
    eval "v=\${$1}"
    [ -n "$v" ] || die "$1 is required in $ENV_FILE"
}

for k in MANIFEST DAEMON_CONFIG NORMAL_CONFIG CANARIES OUT STATE_DIR TASK_TEMPLATE A_ACCOUNT \
    MATERIALS_SRC KB_SRC REPO_SRC INPUTS_SRC; do
    need "$k"
done
MANIFEST=$(abs "$MANIFEST") DAEMON_CONFIG=$(abs "$DAEMON_CONFIG") NORMAL_CONFIG=$(abs "$NORMAL_CONFIG")
CANARIES=$(abs "$CANARIES") TASK_TEMPLATE=$(abs "$TASK_TEMPLATE") OUT=$(abs "$OUT") STATE_DIR=$(abs "$STATE_DIR")
MATERIALS_SRC=$(abs "$MATERIALS_SRC") KB_SRC=$(abs "$KB_SRC") REPO_SRC=$(abs "$REPO_SRC") INPUTS_SRC=$(abs "$INPUTS_SRC")
[ -z "$CELERIS_BIN" ] || CELERIS_BIN=$(abs "$CELERIS_BIN")
[ -z "$CASES_ROOT" ] || CASES_ROOT=$(abs "$CASES_ROOT")
[ -z "$GRADER_ROOT" ] || GRADER_ROOT=$(abs "$GRADER_ROOT")
[ -z "$GRADE_CASES_ROOT" ] || GRADE_CASES_ROOT=$(abs "$GRADE_CASES_ROOT")
[ -z "$RECORDS" ] || RECORDS=$(abs "$RECORDS")
case $CAMPAIGN_BIN in */*) CAMPAIGN_BIN=$(abs "$CAMPAIGN_BIN") ;; esac
[ -z "$RUN_PATH" ] || PATH="$RUN_PATH:$PATH"
export PATH

# ---- 測定 daemon の config から接続先・token を導く（唯一の元）
toml_get() { # <section|-> <key>。"" で囲んだ文字列値だけを返す
    awk -v sec="$1" -v key="$2" '
        /^[ \t]*\[/ { s = $0; gsub(/[ \t\[\]]/, "", s); cur = s; next }
        { line = $0; sub(/[ \t]*#.*$/, "", line) }
        (sec == "-" && cur == "") || cur == sec {
            if (match(line, "^[ \t]*" key "[ \t]*=[ \t]*\"")) {
                v = substr(line, RLENGTH + 1); sub(/".*$/, "", v); print v; exit
            }
        }' "$DAEMON_CONFIG"
}

[ -f "$DAEMON_CONFIG" ] || die "daemon_config not found: $DAEMON_CONFIG"
LISTEN=$(toml_get api listen)
[ -n "$LISTEN" ] || die "[api] listen is missing in $DAEMON_CONFIG"
HOSTPART=${LISTEN%:*}
PORT=${LISTEN##*:}
case $HOSTPART in 0.0.0.0) HOSTPART=127.0.0.1 ;; "[::]") HOSTPART="[::1]" ;; esac
API_URL="http://$HOSTPART:$PORT"
TOKEN_FILE=$(toml_get api token_file)
[ -n "$TOKEN_FILE" ] || die "[api] token_file is missing in $DAEMON_CONFIG"
case $TOKEN_FILE in /*) ;; *) TOKEN_FILE=$(dirname "$DAEMON_CONFIG")/$TOKEN_FILE ;; esac

# token の値は argv・ログ・出力に出さない（CLI には環境変数の「名前」だけを渡す）。
load_token() {
    [ -s "$TOKEN_FILE" ] || die "token file is missing or empty: $TOKEN_FILE (run prepare)"
    CELERIS_MEASURE_TOKEN=$(cat "$TOKEN_FILE")
    export CELERIS_MEASURE_TOKEN
}

# 全 subcommand に共通する CLI 引数。接続先・測定 config・資産は全部ここで 1 回だけ作る。
common_args() {
    set -- --manifest "$MANIFEST" --out "$OUT" \
        --api-url "$API_URL" --token-env CELERIS_MEASURE_TOKEN \
        --task-template "$TASK_TEMPLATE" \
        --measure-config "$DAEMON_CONFIG" --normal-config "$NORMAL_CONFIG" --canaries "$CANARIES" \
        --materials-src "$MATERIALS_SRC" --kb-src "$KB_SRC" --repo-src "$REPO_SRC" --inputs-src "$INPUTS_SRC" \
        --a-account "$A_ACCOUNT" --a-program "$A_PROGRAM"
    # 接続先が launcher 所有の daemon であることの照合（identity.pid）。生きていなければ付けず、検査側が拒否する。
    ! owned_alive || set -- "$@" --daemon-pid "$(cat "$LAUNCHER_PID")"
    [ -z "$CASES_ROOT" ] || set -- "$@" --cases-root "$CASES_ROOT"
    [ -z "$GRADER_ROOT" ] || set -- "$@" --grader-root "$GRADER_ROOT"
    [ -z "$GRADE_CASES_ROOT" ] || set -- "$@" --grade-case-root "$GRADE_CASES_ROOT"
    [ -z "$MEASUREMENT_ID" ] || set -- "$@" --measurement-id "$MEASUREMENT_ID"
    printf '%s\n' "$@"
}

# common_args の出力（1 行 1 引数）を位置引数へ入れる。引数に改行は無い前提。
campaign() { # <subcommand> [追加の引数...]
    sub=$1
    shift
    oldifs=$IFS
    IFS='
'
    # shellcheck disable=SC2046
    set -- "$sub" $(common_args) "$@"
    IFS=$oldifs
    "$CAMPAIGN_BIN" "$@"
}

LAUNCHER_PID=$STATE_DIR/launcher.pid
LAUNCHER_META=$STATE_DIR/launcher.meta

proc_starttime() { # <pid>。/proc/<pid>/stat の 22 番目（comm の ")" より後ろで数える）
    sed -e 's/^.*) //' "/proc/$1/stat" 2>/dev/null | awk '{ print $20 }'
}

proc_cmdline() {
    tr '\0' ' ' <"/proc/$1/cmdline" 2>/dev/null || true
}

# launcher が起動した daemon であることの照合（pid の再利用・別 process を弾く）。成り立てば 0。
owned_alive() {
    [ -f "$LAUNCHER_PID" ] && [ -f "$LAUNCHER_META" ] || return 1
    pid=$(cat "$LAUNCHER_PID")
    case $pid in '' | *[!0-9]*) return 1 ;; esac
    [ "$pid" -gt 1 ] || return 1
    [ -d "/proc/$pid" ] || return 1
    want_start=$(sed -n 's/^starttime=//p' "$LAUNCHER_META")
    [ -n "$want_start" ] && [ "$(proc_starttime "$pid")" = "$want_start" ] || return 1
    case $(proc_cmdline "$pid") in *"--config $DAEMON_CONFIG"*) return 0 ;; esac
    return 1
}

sha256_of() {
    sha256sum "$1" | awk '{ print $1 }'
}

cmd_prepare() {
    mkdir -p "$STATE_DIR"
    miss=
    for p in "$MANIFEST" "$NORMAL_CONFIG" "$CANARIES" "$TASK_TEMPLATE"; do
        [ -f "$p" ] || miss="$miss $p"
    done
    for p in "$MATERIALS_SRC" "$KB_SRC" "$REPO_SRC" "$INPUTS_SRC"; do
        [ -d "$p" ] || miss="$miss $p"
    done
    [ -z "$miss" ] || die "required input missing:$miss"
    "$CAMPAIGN_BIN" validate --manifest "$MANIFEST" >/dev/null || die "manifest validate failed"
    # 測定専用の token（本番の token には触れない）。無いときだけ 0600 で作る。
    if [ ! -s "$TOKEN_FILE" ]; then
        mkdir -p "$(dirname "$TOKEN_FILE")"
        (
            umask 077
            od -An -N32 -tx1 /dev/urandom | tr -d ' \n' >"$TOKEN_FILE"
        )
    fi
    # 測定 daemon の実行ファイルを固定する。start は同じ hash でなければ起動しない（同一 path の差替え）。
    if [ -n "$CELERIS_BIN" ]; then
        [ -x "$CELERIS_BIN" ] || die "celeris binary not executable: $CELERIS_BIN"
        h=$(sha256_of "$CELERIS_BIN")
        if [ -f "$STATE_DIR/celeris.sha256" ] && [ "$(cat "$STATE_DIR/celeris.sha256")" != "$h" ]; then
            die "celeris binary changed since the first prepare (sha256 $h): refusing"
        fi
        printf '%s\n' "$h" >"$STATE_DIR/celeris.sha256"
    fi
    echo "prepared: api_url=$API_URL state_dir=$STATE_DIR"
}

cmd_start() {
    [ -n "$CELERIS_BIN" ] || die "CELERIS_BIN is required to start the measurement daemon"
    need RELEASE
    mkdir -p "$STATE_DIR"
    if owned_alive; then
        echo "daemon already running (pid $(cat "$LAUNCHER_PID")); connecting"
    else
        [ -f "$STATE_DIR/celeris.sha256" ] || die "run prepare first"
        [ "$(sha256_of "$CELERIS_BIN")" = "$(cat "$STATE_DIR/celeris.sha256")" ] ||
            die "celeris binary differs from the one fixed at prepare: refusing to start"
        # 測定 daemon は --config だけで起動する（--db/--listen 等の CLI 上書きを使わない）。
        nohup "$CELERIS_BIN" --config "$DAEMON_CONFIG" --release "$RELEASE" --mode normal \
            >>"$STATE_DIR/daemon.log" 2>&1 </dev/null &
        pid=$!
        printf '%s\n' "$pid" >"$LAUNCHER_PID"
        {
            echo "starttime=$(proc_starttime "$pid")"
            echo "config=$DAEMON_CONFIG"
            echo "release=$RELEASE"
        } >"$LAUNCHER_META"
    fi
    # 出来事待ち（health が答えるまで）。上限は保険。
    i=0
    until curl -fsS -o /dev/null "$API_URL/api/v1/health" 2>/dev/null; do
        owned_alive || die "daemon exited before becoming healthy (see $STATE_DIR/daemon.log)"
        i=$((i + 1))
        [ "$i" -lt $((HEALTH_WAIT_SECS * 10)) ] || die "daemon did not become healthy within ${HEALTH_WAIT_SECS}s"
        sleep 0.1
    done
    load_token
    campaign env-check >/dev/null || {
        campaign env-check >&2 || true
        die "env-check failed: the daemon answering $API_URL is not the measurement daemon"
    }
    echo "started: pid=$(cat "$LAUNCHER_PID") api_url=$API_URL"
}

cmd_preflight() {
    load_token
    campaign preflight
}

worker_args() {
    for w in $WORKER_PROGRAMS; do
        printf '%s\n' --worker-program "$w"
    done
}

cmd_env_lock() { # 作る（初回）か照合する（再開）。不一致は非 0（外部実行の前）。B/C の候補集合・階層は接続先 daemon から取る。
    load_token
    oldifs=$IFS
    IFS='
'
    # shellcheck disable=SC2046
    set -- env-lock --manifest "$MANIFEST" --out "$OUT" --api-url "$API_URL" --token-env CELERIS_MEASURE_TOKEN --a-program "$A_PROGRAM" $(worker_args)
    IFS=$oldifs
    "$CAMPAIGN_BIN" "$@"
}

cmd_watch() {
    load_token
    campaign preflight >/dev/null || {
        campaign preflight >&2 || true
        die "preflight failed: nothing was executed"
    }
    cmd_env_lock >/dev/null || {
        cmd_env_lock >&2 || true
        die "env-lock mismatch: nothing was executed"
    }
    campaign watch --executor "$EXECUTOR" --interval-secs "$INTERVAL_SECS" --max-iterations "$MAX_ITERATIONS"
}

cmd_grade() {
    load_token
    [ -n "$GRADE_CASES_ROOT" ] || die "GRADE_CASES_ROOT is required for grade"
    campaign grade-out --cases "$GRADE_CASES_ROOT"
    campaign check
}

cmd_export() {
    load_token
    set --
    [ -z "$RECORDS" ] || set -- --records "$RECORDS"
    campaign export --dest "$OUT/export" "$@"
}

cmd_stop() {
    [ -f "$LAUNCHER_PID" ] || { echo "nothing to stop (no launcher pidfile)"; return 0; }
    pid=$(cat "$LAUNCHER_PID")
    if ! owned_alive; then
        # 死んでいるなら pidfile だけ片付ける。生きている別 process は触らない。
        if [ -d "/proc/$pid" ] && [ -n "$pid" ]; then
            die "pid $pid is not the daemon this launcher started (starttime/cmdline differ): not signalling"
        fi
        rm -f "$LAUNCHER_PID" "$LAUNCHER_META"
        echo "daemon already gone; pidfile removed"
        return 0
    fi
    kill -TERM "$pid"
    i=0
    while [ -d "/proc/$pid" ] && owned_alive; do
        i=$((i + 1))
        if [ "$i" -ge 300 ]; then
            owned_alive && kill -KILL "$pid"
            break
        fi
        sleep 0.1
    done
    rm -f "$LAUNCHER_PID" "$LAUNCHER_META"
    echo "stopped pid $pid"
}

case $SUB in
    prepare) cmd_prepare ;;
    start) cmd_start ;;
    preflight) cmd_preflight ;;
    watch) cmd_watch ;;
    grade) cmd_grade ;;
    export) cmd_export ;;
    stop) cmd_stop ;;
    *) usage ;;
esac
