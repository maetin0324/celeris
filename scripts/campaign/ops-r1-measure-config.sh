#!/bin/sh
# ops-r1 測定 daemon の config 生成（POSIX sh、dash で動く）。設計は
# agent-docs/progress/2026-10-29-k4-ops-r1-pilot/measure-probe.md。daemon のコードは変えず、config だけで
# CoS 経路（[cos] enabled・policy 6・CoS skill・route）を測定専用の state に閉じる。
#
#   ops-r1-measure-config.sh --state <dir> --release-dir <release の dir> --models <key> --out <daemon.toml>
#       [--port <n>] [--harness real|stub] [--stub-command <path>] [--claude-bin <path>]
#       [--claude-accounts <dir>] [--clusters-from <通常運用の config.toml>] [--skills-src <dir>]
#
# --models の key（d-ops-r1-models の option）:
#   cos-opus    CoS = claude-opus-5-5、planner/worker/reviewer = claude-sonnet-5-5
#   all-sonnet  全役割 claude-sonnet-5-5
# --harness real（既定）は claude_code harness・claude_oauth 供給元・[adapters.claude_code]。stub は
# harness id を claude_code のまま kind = "fake" にし、[adapters.fake] command = [--stub-command] にする
# （差は harness・source・adapter の 3 節だけ）。
# --port を省くと 127.0.0.1 の空き port を選ぶ。同じ入力（--port を含む）なら daemon.toml は byte 一致する。
#
# 書くもの: --out（daemon.toml）と --state の下（token・kb/skills・ws・scratch・setup/project.json）。
# 試行専用の案件と案件 CoS thread は daemon 起動後に API で作る（setup/project.json の body で
# POST /api/v1/projects、続けて POST /api/v1/chat/threads {project_id}。docs/ops/ops-r1-measure-daemon.md）。
# exit: 0 書いた / 1 入力の不備（release dir に bin/celeris が無い等）/ 2 使い方の誤り・本番 path・未知の --models。
set -eu

usage() {
    echo "usage: $0 --state <dir> --release-dir <dir> --models <cos-opus|all-sonnet> --out <daemon.toml> [--port <n>] [--harness real|stub] [--stub-command <path>] [--claude-bin <path>] [--claude-accounts <dir>] [--clusters-from <config.toml>] [--skills-src <dir>]" >&2
    exit 2
}
refuse() {
    echo "ops-r1-measure-config: $*" >&2
    exit 2
}
die() {
    echo "ops-r1-measure-config: $*" >&2
    exit 1
}

HERE=$(cd "$(dirname "$0")" && pwd)
STATE= RELEASE_DIR= MODELS= OUT= PORT= HARNESS=real STUB_COMMAND= CLAUDE_BIN=claude
CLAUDE_ACCOUNTS= CLUSTERS_FROM= SKILLS_SRC=$HERE/../../config/skills
while [ $# -gt 0 ]; do
    [ $# -ge 2 ] || usage
    case $1 in
        --state) STATE=$2 ;;
        --release-dir) RELEASE_DIR=$2 ;;
        --models) MODELS=$2 ;;
        --out) OUT=$2 ;;
        --port) PORT=$2 ;;
        --harness) HARNESS=$2 ;;
        --stub-command) STUB_COMMAND=$2 ;;
        --claude-bin) CLAUDE_BIN=$2 ;;
        --claude-accounts) CLAUDE_ACCOUNTS=$2 ;;
        --clusters-from) CLUSTERS_FROM=$2 ;;
        --skills-src) SKILLS_SRC=$2 ;;
        *) usage ;;
    esac
    shift 2
done
[ -n "$STATE" ] && [ -n "$RELEASE_DIR" ] && [ -n "$MODELS" ] && [ -n "$OUT" ] || usage

# ---- models（決定の option key → model）
case $MODELS in
    cos-opus) COS_MODEL=claude-opus-5-5 RUN_MODEL=claude-sonnet-5-5 ;;
    all-sonnet) COS_MODEL=claude-sonnet-5-5 RUN_MODEL=claude-sonnet-5-5 ;;
    *) refuse "unknown --models $MODELS (cos-opus | all-sonnet)" ;;
esac
case $HARNESS in
    real) ;;
    stub) [ -n "$STUB_COMMAND" ] || refuse "--harness stub needs --stub-command" ;;
    *) refuse "unknown --harness $HARNESS (real | stub)" ;;
esac
if [ -n "$PORT" ]; then
    case $PORT in
        *[!0-9]*) refuse "--port must be a number: $PORT" ;;
    esac
    [ "$PORT" -ge 1 ] && [ "$PORT" -le 65535 ] || refuse "--port out of range: $PORT"
fi

# ---- 本番 path の拒否（何かを書く前に判定する。symlink と .. は realpath -m で解く）
canon() { realpath -m -- "$1"; }
STATE=$(canon "$STATE")
OUT=$(canon "$OUT")
RELEASE_DIR=$(canon "$RELEASE_DIR")
SKILLS_SRC=$(canon "$SKILLS_SRC")
PROD_ROOTS="/local/celeris/state"
if [ -n "${HOME:-}" ]; then
    PROD_ROOTS="$(canon "$HOME/.config/celeris") $(canon "$HOME/.local/celeris") $PROD_ROOTS"
fi
under() { # $1 path, $2 root
    case $1 in
        "$2" | "$2"/*) return 0 ;;
    esac
    return 1
}
for root in $PROD_ROOTS; do
    under "$STATE" "$root" && refuse "--state $STATE is under the production path $root"
    under "$OUT" "$root" && refuse "--out $OUT is under the production path $root"
done

# ---- 入力の検査
[ -f "$RELEASE_DIR/bin/celeris" ] || die "release dir lacks bin/celeris: $RELEASE_DIR"
[ -f "$RELEASE_DIR/manifest.json" ] || die "release dir lacks manifest.json: $RELEASE_DIR"
RELEASE_SHA12=$(sed -n 's/.*"sha12"[[:space:]]*:[[:space:]]*"\([0-9a-f]*\)".*/\1/p' "$RELEASE_DIR/manifest.json" | head -n 1)
[ -n "$RELEASE_SHA12" ] || die "manifest.json lacks sha12: $RELEASE_DIR/manifest.json"
CELERIS_SHA256=$(sha256sum "$RELEASE_DIR/bin/celeris" | cut -d ' ' -f 1)
for name in cos-operator cos-inbox-triage; do
    [ -f "$SKILLS_SRC/$name/SKILL.md" ] || die "skills source lacks $name/SKILL.md: $SKILLS_SRC"
done
if [ -n "$CLUSTERS_FROM" ]; then
    [ -f "$CLUSTERS_FROM" ] || die "--clusters-from not found: $CLUSTERS_FROM"
fi
if [ -z "$PORT" ]; then
    PORT=$(python3 -I -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])') \
        || die "cannot pick a free loopback port"
fi

# TOML の基本文字列（\ と " を escape。改行を含む path は拒否）。
q() {
    case $1 in
        *"
"*) refuse "path contains a newline: $1" ;;
    esac
    printf '"%s"' "$(printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g')"
}

# ---- state（db・token・knowledge・workspace・scratch を全部この下に置く）
mkdir -p "$STATE/kb/skills" "$STATE/ws" "$STATE/scratch" "$STATE/setup" "$(dirname "$OUT")"
if [ ! -s "$STATE/token" ]; then
    (umask 077 && od -An -N32 -tx1 /dev/urandom | tr -d ' \n' >"$STATE/token" && echo >>"$STATE/token")
fi
# CoS skill の配送: config/skills の各 skill を測定 KB の skills/ へ写す（毎回作り直し、同じ中身にする）。
SKILLS=
for dir in "$SKILLS_SRC"/*/; do
    name=$(basename "$dir")
    [ -f "$dir/SKILL.md" ] || continue
    rm -rf "$STATE/kb/skills/$name"
    cp -R "$SKILLS_SRC/$name" "$STATE/kb/skills/$name"
    SKILLS="$SKILLS $name"
done
cat >"$STATE/setup/project.json" <<'EOF'
{"title": "ops-r1 trial", "request": "ops-r1 K4 pilot の試行専用の案件。人の依頼は案件 CoS thread に入れる。"}
EOF

# ---- daemon.toml
if [ "$HARNESS" = real ]; then
    SOURCE=claude_oauth
else
    SOURCE=stub
fi
if [ "$COS_MODEL" = "$RUN_MODEL" ]; then
    SOURCE_MODELS="\"$RUN_MODEL\""
else
    SOURCE_MODELS="\"$COS_MODEL\", \"$RUN_MODEL\""
fi
TMP_OUT="$OUT.tmp.$$"
trap 'rm -f "$TMP_OUT"' EXIT
{
    cat <<EOF
# ops-r1 測定 daemon の config（scripts/campaign/ops-r1-measure-config.sh が生成。手で書き換えない）。秘密値なし。
# release $RELEASE_SHA12（bin/celeris sha256 $CELERIS_SHA256）、models $MODELS、harness $HARNESS。
# 本番と別: db・token・knowledge・workspace・scratch は $STATE の下、api は 127.0.0.1:$PORT。
# [memory]・[llm_proxy]・[[mcp.listeners]]・[[cron.seed]] は置かない（既定で無効）。

db = $(q "$STATE/celeris.db")
workspace_root = $(q "$STATE/ws")
tick_ms = 2000
max_concurrency = 1
max_requeues = 0

[api]
listen = "127.0.0.1:$PORT"

[[api.tokens]]
principal = "operator:ops-r1"
token_file = $(q "$STATE/token")

[knowledge]
root = $(q "$STATE/kb")
default_mounts = ["kb:trial"]
[knowledge.langmem]
enabled = false
[knowledge.gc]
enabled = false

[scratch]
dir = $(q "$STATE/scratch")

[selfdeploy]
releases_dir = $(q "$(dirname "$RELEASE_DIR")")

[execution]
max_parallel_work_units = 1
[execution.tree]
max_parallel_child_tasks = 1
EOF
    # [accounts] は claude_dir 無しでは読み込みエラーになるので、--claude-accounts のときだけ書く。
    if [ -n "$CLAUDE_ACCOUNTS" ]; then
        echo
        echo "[accounts]"
        echo "claude_dir = $(q "$(canon "$CLAUDE_ACCOUNTS")")"
        echo "max_runs_per_account = 1"
    fi
    cat <<EOF

[cos]
enabled = true
route_candidates = [{ source = "$SOURCE", model = "$COS_MODEL" }]
[cos.triage]
policy_skill = "cos-inbox-triage"
policy_version = "6"

[route_defaults.claude_code]
candidates = [{ source = "$SOURCE", model = "$RUN_MODEL" }]

# 全 task（genre・role・execution class によらず）を claude_code に送る。無いと CoS が起票した task が
# 「no eligible harness is declared」で route されない（measure-e2e で判明）。
[harness_selection]
rules = [{ required_capabilities = [], harness = "claude_code" }]
execution_class_defaults = { coding = ["claude_code"], planner = ["claude_code"], review = ["claude_code"] }

EOF
    if [ "$HARNESS" = real ]; then
        cat <<EOF
[adapters.claude_code]
command = $(q "$CLAUDE_BIN")

[llm_sources.claude_oauth]
kind = "claude_oauth"
api_format = "anthropic"
models = [$SOURCE_MODELS]

[harnesses.claude_code]
kind = "claude_code"
api_formats = ["anthropic"]
source_kinds = ["claude_oauth"]
capabilities = ["tools", "repo", "skills"]
EOF
    else
        cat <<EOF
[adapters.fake]
command = [$(q "$(canon "$STUB_COMMAND")")]

[llm_sources.stub]
kind = "fake"
accounts = ["stub"]
models = [$SOURCE_MODELS]

[harnesses.claude_code]
kind = "fake"
api_formats = ["fake"]
source_kinds = ["fake"]
EOF
    fi
    if [ -n "$CLUSTERS_FROM" ]; then
        echo
        echo "# ---- 通常運用の [[clusters]] の写し（$(canon "$CLUSTERS_FROM")）"
        # [[clusters]] / [[clusters.*]] の節だけを写す（次の別の表の見出しで止まる）。
        awk '
            /^[[:space:]]*\[/ {
                keep = ($0 ~ /^[[:space:]]*\[\[clusters(\.[A-Za-z0-9_-]+)?\]\]/)
            }
            keep { print }
        ' "$CLUSTERS_FROM"
    fi
} >"$TMP_OUT"
mv "$TMP_OUT" "$OUT"
trap - EXIT
echo "wrote $OUT (port $PORT, release $RELEASE_SHA12, models $MODELS, harness $HARNESS, skills:$SKILLS)"
