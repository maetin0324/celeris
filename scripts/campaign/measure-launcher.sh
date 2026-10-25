#!/bin/sh
# 測定環境 launcher（POSIX sh）。既存の celeris-campaign / celeris CLI を包む薄い入口で、判定は全部 CLI 側にある。
# 単一の設定ファイル（--env <file>、KEY=VALUE の sh 形式）から --api-url・--measure-config 等を生成する。
# 本番 daemon・systemd・本番 config・本番 DB には触れない。停止は launcher が起動した pid（$STATE_DIR/launcher.pid）だけ。
#
#   measure-launcher.sh <prepare|start|preflight|lock|watch|collect|grade|export|stop> --env <file>
#
# 標準の流れは prepare → start → preflight → watch → grade（評価側の job 観測の収集 → 独立採点 → 検査）→ export。
# 各 subcommand は再実行できる。watch は台帳から再開し、毎回 daemon 実体の照合・preflight（env-check を含む）・
# env-lock の照合を通ってから始まる。
# start は daemon を起こす前に測定 config を検査し（config-check）、不適切なら起動しない。
# daemon 実体（prepare で固定した CELERIS_BIN の sha256 と、起動した process の実行像 /proc/<pid>/exe）は
# start（新規・既存 process への再接続）・preflight・watch の全経路で照合する。
set -eu

usage() {
    echo "usage: $0 <prepare|start|preflight|lock|watch|collect|grade|export|stop> --env <measure-launcher.env>" >&2
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
GLOBAL_LEDGER= VARIANT_B0_SHA256= INITIAL_TREES=
MATERIALS_SRC= KB_SRC= REPO_SRC= INPUTS_SRC= CASES_ROOT= GRADER_ROOT= GRADE_CASES_ROOT=
SERIES_ROOT= SERIES_IMPORT_OUTS=
RECORDS= CAP_ENV= EXECUTOR=celeris INTERVAL_SECS=60 MAX_ITERATIONS=1440 HEALTH_WAIT_SECS=60
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

# A_ACCOUNT は必須にしない: A が有効な manifest では CLI の preflight が要求し、B/C のみの構成は要らない。
# REPO_SRC も必須にしない: 初期 tree を固定する case（K2）を選んだ manifest では CLI の preflight（lock）が要求し、
# 初期 tree の無い case だけ（K4 のみ）の構成では渡すと拒否される。
for k in MANIFEST DAEMON_CONFIG NORMAL_CONFIG CANARIES OUT STATE_DIR TASK_TEMPLATE \
    MATERIALS_SRC KB_SRC INPUTS_SRC; do
    need "$k"
done
MANIFEST=$(abs "$MANIFEST") DAEMON_CONFIG=$(abs "$DAEMON_CONFIG") NORMAL_CONFIG=$(abs "$NORMAL_CONFIG")
CANARIES=$(abs "$CANARIES") TASK_TEMPLATE=$(abs "$TASK_TEMPLATE") OUT=$(abs "$OUT") STATE_DIR=$(abs "$STATE_DIR")
MATERIALS_SRC=$(abs "$MATERIALS_SRC") KB_SRC=$(abs "$KB_SRC") INPUTS_SRC=$(abs "$INPUTS_SRC")
[ -z "$REPO_SRC" ] || REPO_SRC=$(abs "$REPO_SRC")
[ -z "$CELERIS_BIN" ] || CELERIS_BIN=$(abs "$CELERIS_BIN")
[ -z "$CASES_ROOT" ] || CASES_ROOT=$(abs "$CASES_ROOT")
[ -z "$GRADER_ROOT" ] || GRADER_ROOT=$(abs "$GRADER_ROOT")
[ -z "$GRADE_CASES_ROOT" ] || GRADE_CASES_ROOT=$(abs "$GRADE_CASES_ROOT")
[ -z "$SERIES_ROOT" ] || SERIES_ROOT=$(abs "$SERIES_ROOT")
[ -z "$RECORDS" ] || RECORDS=$(abs "$RECORDS")
[ -z "$GLOBAL_LEDGER" ] || GLOBAL_LEDGER=$(abs "$GLOBAL_LEDGER")
[ -z "$INITIAL_TREES" ] || INITIAL_TREES=$(abs "$INITIAL_TREES")
[ -z "$CAP_ENV" ] || CAP_ENV=$(abs "$CAP_ENV")
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
        --materials-src "$MATERIALS_SRC" --kb-src "$KB_SRC" --inputs-src "$INPUTS_SRC" \
        --a-program "$A_PROGRAM"
    # K2 の初期 tree。初期 tree を固定する case が無い構成（K4 のみ）では渡さない（渡すと lock 検査が拒否する）。
    [ -z "$REPO_SRC" ] || set -- "$@" --repo-src "$REPO_SRC"
    # A のアカウントは A が有効な構成だけで渡す（無効 arm の認証を要求しない）。要否の判定は CLI（preflight・run）。
    [ -z "$A_ACCOUNT" ] || set -- "$@" --a-account "$A_ACCOUNT"
    # 共有承認の台帳。manifest が宣言した台帳と一致しなければ CLI が投入前に拒否する（省略も拒否）。
    [ -z "$GLOBAL_LEDGER" ] || set -- "$@" --global-ledger "$GLOBAL_LEDGER"
    # 選択済み specialization の承認済み variant-b0.json の sha256（preflight が manifest の variant と照合する）。
    [ -z "$VARIANT_B0_SHA256" ] || set -- "$@" --variant-b0-sha256 "$VARIANT_B0_SHA256"
    # 接続先が launcher 所有の daemon であることの照合（identity.pid）。生きていなければ付けず、検査側が拒否する。
    ! owned_alive || set -- "$@" --daemon-pid "$(cat "$LAUNCHER_PID")"
    [ -z "$CASES_ROOT" ] || set -- "$@" --cases-root "$CASES_ROOT"
    [ -z "$GRADER_ROOT" ] || set -- "$@" --grader-root "$GRADER_ROOT"
    [ -z "$GRADE_CASES_ROOT" ] || set -- "$@" --grade-case-root "$GRADE_CASES_ROOT"
    [ -z "$MEASUREMENT_ID" ] || set -- "$@" --measurement-id "$MEASUREMENT_ID"
    # 副比較（B1/C1）の系列台帳の root。測定 ID ごとに <root>/<id>/ へ固定され、OUT はその runs/ の下だけ許される。
    # 副比較で未設定なら CLI が拒否する（既消費 0 として扱わない）。
    [ -z "$SERIES_ROOT" ] || set -- "$@" --series-root "$SERIES_ROOT"
    # 必要能力 probe と A の sandbox に足す toolchain（celeris.campaign-cap-env/1）。無ければ preflight が拒否する。
    [ -z "$CAP_ENV" ] || set -- "$@" --cap-env "$CAP_ENV"
    # worker harness の実行ファイル。preflight（run・watch 内のものを含む）の env-lock 照合で env-lock と同じ値を使う。
    for w in $WORKER_PROGRAMS; do
        set -- "$@" --worker-program "$w"
    done
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
    sha256sum "$1" 2>/dev/null | awk '{ print $1 }'
}

meta_get() { # <key>
    sed -n "s/^$1=//p" "$LAUNCHER_META" 2>/dev/null
}

# 起動した process の実行像（/proc/<pid>/exe の指す path）。差し替えで消えた path は " (deleted)" が付く。
proc_exe() {
    readlink "/proc/$1/exe" 2>/dev/null || true
}

# 所有 process の起動 identity（starttime・cmdline・実行像）が起動時の記録と同じこと。stop・失敗時の片付けの前に必ず通す。
owned_identity() {
    owned_alive || return 1
    pid=$(cat "$LAUNCHER_PID")
    want_exe=$(meta_get exe)
    # 実行像の file が差し替えで消えても（" (deleted)"）、同じ process であることは変わらない。
    got_exe=$(proc_exe "$pid")
    [ -z "$want_exe" ] || [ "${got_exe% (deleted)}" = "$want_exe" ] || return 1
    return 0
}

# daemon 実体の照合（全経路）。成り立たなければ die（外部実行・task 投入の前）。
#  1. CELERIS_BIN の現物が prepare で固定した sha256 と同じ（同一 path の差替えを弾く）
#  2. launcher 所有の process が生きていて、その実行像（/proc/<pid>/exe）が起動時に記録した path・sha256 と同じで、
#     その path の現物も同じ（動いている binary の同一 path 差替えを弾く）
# 接続先がこの process であることは env-check が identity.pid と launcher 所有 PID の一致で確かめる。
verify_daemon() {
    [ -n "$CELERIS_BIN" ] || die "CELERIS_BIN is required (the measurement daemon binary is pinned and verified on every path)"
    [ -f "$STATE_DIR/celeris.sha256" ] || die "run prepare first (no pinned daemon binary)"
    pinned=$(cat "$STATE_DIR/celeris.sha256")
    [ "$(sha256_of "$CELERIS_BIN")" = "$pinned" ] ||
        die "daemon binary $CELERIS_BIN differs from the one pinned at prepare (sha256 $pinned): refusing"
    owned_alive || die "the launcher-owned daemon is not running (run start): refusing"
    pid=$(cat "$LAUNCHER_PID")
    want_exe=$(meta_get exe)
    want_sha=$(meta_get exe_sha256)
    [ -n "$want_exe" ] && [ -n "$want_sha" ] ||
        die "launcher.meta has no recorded daemon image (stop and start again): refusing"
    got_exe=$(proc_exe "$pid")
    [ "$got_exe" = "$want_exe" ] ||
        die "daemon binary image of pid $pid is '$got_exe', not '$want_exe' recorded at start (replaced or deleted): refusing"
    [ "$(sha256_of "/proc/$pid/exe")" = "$want_sha" ] ||
        die "daemon binary image of pid $pid differs from the one recorded at start: refusing"
    [ "$(sha256_of "$want_exe")" = "$want_sha" ] ||
        die "daemon binary $want_exe was replaced on disk after the daemon started: refusing"
}

# 起動前の検査（daemon に接続しない）。測定 config が通常運用の DB・作業場所・KB・記憶に重なる、自動整理が有効、等は拒否。
config_check() {
    set -- config-check --manifest "$MANIFEST" \
        --measure-config "$DAEMON_CONFIG" --normal-config "$NORMAL_CONFIG" --canaries "$CANARIES" \
        --kb-src "$KB_SRC"
    "$CAMPAIGN_BIN" "$@"
}

cmd_prepare() {
    mkdir -p "$STATE_DIR"
    miss=
    for p in "$MANIFEST" "$NORMAL_CONFIG" "$CANARIES" "$TASK_TEMPLATE"; do
        [ -f "$p" ] || miss="$miss $p"
    done
    for p in "$MATERIALS_SRC" "$KB_SRC" "$INPUTS_SRC" ${REPO_SRC:+"$REPO_SRC"}; do
        [ -d "$p" ] || miss="$miss $p"
    done
    [ -z "$miss" ] || die "required input missing:$miss"
    "$CAMPAIGN_BIN" validate --manifest "$MANIFEST" >/dev/null || die "manifest validate failed"
    # 評価側の固定入力 tgz（grade-case の input_archive）を INPUTS_SRC から grade-case の隣へ写す
    # （ADR 2026-10-23-k4-cells-validity-grading D3。sha256 が合わなければ写さず止まる。preflight が実在・一致・展開を確かめる）。
    [ -z "$GRADE_CASES_ROOT" ] || "$CAMPAIGN_BIN" stage-grader-inputs --manifest "$MANIFEST" --grade-case-root "$GRADE_CASES_ROOT" --inputs-src "$INPUTS_SRC" \
        || die "stage-grader-inputs failed: the grader's pinned input archive is missing or differs from INPUTS_SRC"
    config_check >/dev/null || {
        config_check >&2 || true
        die "config-check failed: the measurement config is not isolated from normal operation"
    }
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
    # 副比較の系列台帳（冪等）。SERIES_IMPORT_OUTS は引継ぎ元の既存 out（空白区切り。消費を台帳へ追記する）。
    if [ -n "$SERIES_ROOT" ]; then
        [ -n "$MEASUREMENT_ID" ] || die "MEASUREMENT_ID is required with SERIES_ROOT"
        set -- series-init --manifest "$MANIFEST" --measurement-id "$MEASUREMENT_ID" --series-root "$SERIES_ROOT"
        for o in $SERIES_IMPORT_OUTS; do set -- "$@" --import-out "$(abs "$o")"; done
        "$CAMPAIGN_BIN" "$@" >/dev/null || die "series-init failed: the series ledger is not ready"
    fi
    echo "prepared: api_url=$API_URL state_dir=$STATE_DIR"
}

# 起動に失敗したとき、この呼び出しで起動した process だけを（起動 identity を確かめてから）止める。
cleanup_started() {
    [ "${STARTED_NOW:-0}" = 1 ] || return 0
    stop_owned >&2 || true
}

cmd_start() {
    [ -n "$CELERIS_BIN" ] || die "CELERIS_BIN is required to start the measurement daemon"
    need RELEASE
    mkdir -p "$STATE_DIR"
    # daemon を起こす前に config を検査する（不適切な DB・workspace・自動処理の設定では副作用を出さない）。
    config_check >/dev/null || {
        config_check >&2 || true
        die "config-check failed: the measurement daemon was not started"
    }
    STARTED_NOW=0
    if owned_alive; then
        # 既存 process への再接続も、新規起動と同じ daemon 実体の照合を通す。
        verify_daemon
        echo "daemon already running (pid $(cat "$LAUNCHER_PID")); connecting"
    else
        [ -f "$STATE_DIR/celeris.sha256" ] || die "run prepare first"
        [ "$(sha256_of "$CELERIS_BIN")" = "$(cat "$STATE_DIR/celeris.sha256")" ] ||
            die "daemon binary $CELERIS_BIN differs from the one pinned at prepare: refusing to start"
        # 測定 daemon は --config だけで起動する（--db/--listen 等の CLI 上書きを使わない）。
        nohup "$CELERIS_BIN" --config "$DAEMON_CONFIG" --release "$RELEASE" --mode normal \
            >>"$STATE_DIR/daemon.log" 2>&1 </dev/null &
        pid=$!
        STARTED_NOW=1
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
        owned_alive || {
            cleanup_started
            die "daemon exited before becoming healthy (see $STATE_DIR/daemon.log)"
        }
        i=$((i + 1))
        [ "$i" -lt $((HEALTH_WAIT_SECS * 10)) ] || {
            cleanup_started
            die "daemon did not become healthy within ${HEALTH_WAIT_SECS}s"
        }
        sleep 0.1
    done
    if [ "$STARTED_NOW" = 1 ]; then
        # 起動した process の実行像を記録する（以後の全経路はこれと照合する）。
        if ! owned_alive; then
            cleanup_started
            die "daemon exited before becoming healthy (see $STATE_DIR/daemon.log)"
        fi
        pid=$(cat "$LAUNCHER_PID")
        exe=$(proc_exe "$pid")
        {
            echo "exe=$exe"
            echo "exe_sha256=$(sha256_of "/proc/$pid/exe")"
        } >>"$LAUNCHER_META"
        verify_daemon
    fi
    load_token
    campaign env-check >/dev/null || {
        campaign env-check >&2 || true
        cleanup_started
        die "env-check failed: the daemon answering $API_URL is not the measurement daemon"
    }
    echo "started: pid=$(cat "$LAUNCHER_PID") api_url=$API_URL"
}

cmd_preflight() {
    verify_daemon
    load_token
    # watch と同じ env-lock の照合（<out>/env-lock.json が在れば接続先の測定 daemon の一覧と突き合わせる）も行う。
    campaign preflight
}

# offline-lock の入力（GET /api/v1/llm/models の保存）は launcher.env の測定 daemon から取る。本番 daemon の一覧は
# 使わない（測定 daemon と source が違えば watch・preflight の env-lock 照合で拒否される。ADR 2026-10-21-preflight-env-lock-check）。
cmd_lock() { # 生成先は環境変数 LOCK_OUT（空 dir か未作成）
    gen=${LOCK_OUT:-}
    [ -n "$gen" ] || die "lock: set LOCK_OUT=<empty dir> (env-lock.json / lock.json are written there)"
    [ ! -e "$gen" ] || [ -z "$(ls -A "$gen")" ] || die "$gen: lock output dir must be empty"
    verify_daemon
    load_token
    mkdir -p "$gen"
    ev="$gen.measure-llm-models.json"
    curl -fsS -H "Authorization: Bearer $CELERIS_MEASURE_TOKEN" "$API_URL/api/v1/llm/models" >"$ev" ||
        die "GET $API_URL/api/v1/llm/models failed"
    sha=$(sha256sum "$ev" | cut -d' ' -f1)
    echo "models evidence: $ev sha256=$sha" >&2
    oldifs=$IFS
    IFS='
'
    # shellcheck disable=SC2046
    set -- $(common_args)
    IFS=$oldifs
    # common_args の --out（試行の out）を生成先に置き換える（後の指定が勝たない CLI もあるので取り除く）。
    n=$#
    i=0
    skip=
    for x in "$@"; do
        i=$((i + 1))
        if [ -n "$skip" ]; then skip=; elif [ "$x" = --out ]; then skip=1; else set -- "$@" "$x"; fi
        [ $i -lt $n ] || break
    done
    shift "$n"
    "$CAMPAIGN_BIN" offline-lock "$@" --out "$gen" --models-evidence "$ev" --models-evidence-sha256 "$sha"
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
    [ -z "$CAP_ENV" ] || set -- "$@" --cap-env "$CAP_ENV"
    "$CAMPAIGN_BIN" "$@"
}

cmd_watch() {
    verify_daemon
    load_token
    campaign preflight >/dev/null || {
        campaign preflight >&2 || true
        die "preflight failed: nothing was executed"
    }
    cmd_env_lock >/dev/null || {
        cmd_env_lock >&2 || true
        die "env-lock mismatch: nothing was executed"
    }
    # 照合と実行の間に daemon が差し替わっていないことを、実行の直前にもう一度確かめる。
    verify_daemon
    campaign watch --executor "$EXECUTOR" --interval-secs "$INTERVAL_SECS" --max-iterations "$MAX_ITERATIONS"
}

# 評価側が scheduler から取る job の終了記録（<out>/observed）。台帳の job ID・trial identity・成果物の hash に結ぶ。
# 未終了・欠測・成果物の改変は記録を作らず非 0（採点は grade が続けて行い、その trial を成功にしない）。
cmd_collect() {
    load_token
    campaign collect-observed-out
}

cmd_grade() {
    load_token
    [ -n "$GRADE_CASES_ROOT" ] || die "GRADE_CASES_ROOT is required for grade"
    collect_rc=0
    collect_out=$(campaign collect-observed-out 2>&1) || collect_rc=$?
    printf '%s\n' "$collect_out"
    set -- --cases "$GRADE_CASES_ROOT"
    [ -z "$INITIAL_TREES" ] || set -- "$@" --initial-trees "$INITIAL_TREES"
    campaign grade-out "$@"
    campaign check
    # 収集できなかった trial（未終了・欠測・改変）は観測記録が無いまま grade-out が採点し、実験証拠の検査で成功にならない。
    [ "$collect_rc" -eq 0 ] || echo "measure-launcher: warning: collect-observed-out reported trials without evaluator observations (see above); they are not success evidence" >&2
}

cmd_export() {
    load_token
    set --
    # RECORDS（decision-log の records.jsonl）は collect/grade では作られない。測定 DB の写しから
    # `celerisctl --db <写し> campaign export <campaign_id> --out <dir>` で作る（docs/ops/measure-run-final.md §8）。
    # 無ければ表の consumption は欠測（0 ではない）として書かれる。
    if [ -n "$RECORDS" ]; then
        set -- --records "$RECORDS"
    else
        echo "measure-launcher: warning: RECORDS is unset: final accounting is not computed and the table records total consumption as missing" >&2
    fi
    campaign export --dest "$OUT/export" "$@"
}

# launcher 所有の process（起動 identity が記録と一致するもの）だけに signal を送る。
stop_owned() {
    [ -f "$LAUNCHER_PID" ] || { echo "nothing to stop (no launcher pidfile)"; return 0; }
    pid=$(cat "$LAUNCHER_PID")
    if ! owned_identity; then
        # 死んでいるなら pidfile だけ片付ける。生きている別 process は触らない。
        if [ -n "$pid" ] && [ -d "/proc/$pid" ]; then
            echo "measure-launcher: pid $pid is not the daemon this launcher started (starttime/cmdline/image differ): not signalling" >&2
            return 1
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
            ! owned_identity || kill -KILL "$pid"
            break
        fi
        sleep 0.1
    done
    rm -f "$LAUNCHER_PID" "$LAUNCHER_META"
    echo "stopped pid $pid"
}

cmd_stop() {
    stop_owned || exit 1
}

case $SUB in
    prepare) cmd_prepare ;;
    start) cmd_start ;;
    preflight) cmd_preflight ;;
    lock) cmd_lock ;;
    watch) cmd_watch ;;
    collect) cmd_collect ;;
    grade) cmd_grade ;;
    export) cmd_export ;;
    stop) cmd_stop ;;
    *) usage ;;
esac
