#!/bin/sh
# 試行 A（単体 Claude Code）を A の sandbox の形で人が起動する command を出す（ADR 2026-10-31-ops-r2-preflight-fixes 付記 D-trial-a）。
# 実行はしない。出した 1 行を人が別の端末で実行する。POSIX sh。
#   ops-r2-trial-a-command.sh --state <A の state> --claude-dir <claude 2.1.303 の固定 copy の dir> --account <A の account dir> \
#     --ssh-config <専用 ssh config> --socket <ssh master の socket> --seed-sha256 <hex> --session-id <小文字の UUID>
# claude の権限の形は K4 ops-r1 の A と同じ（付記 D-trial-a2）: --session-id 固定・--permission-mode bypassPermissions・
# --disallowedTools Task。path は改行以外の文字を受け、出す 1 行では sh の単一引用符で quote する（@・: を含む socket も可）。
# 前提（無ければ exit 1）: <state>/seed/benchfs.tar（ops-r2-seed.sh place）、<state>/bin/ops-r2-seed.sh、
# <state>/prompt.md（承認済みの固定依頼文）、<claude-dir>/claude-2.1.303。
# 承認票の条件: model claude-sonnet-5-5、1 試行 3600 秒（timeout で強制）・1,500,000 token（CLI では強制できない。停止規則で operator が見る）。
set -eu
refuse() { echo "ops-r2-trial-a-command: $*" >&2; exit 1; }
usage() { echo "ops-r2-trial-a-command: $*" >&2; exit 2; }
STATE= CDIR= ACCOUNT= SSHCFG= SOCK= SHA= SID=
while [ $# -gt 0 ]; do
    [ $# -ge 2 ] || usage "missing value for $1"
    case $1 in
        --state) STATE=$2 ;; --claude-dir) CDIR=$2 ;; --account) ACCOUNT=$2 ;;
        --ssh-config) SSHCFG=$2 ;; --socket) SOCK=$2 ;; --seed-sha256) SHA=$2 ;;
        --session-id) SID=$2 ;;
        *) usage "unknown argument: $1" ;;
    esac
    shift 2
done
[ -n "$STATE" ] && [ -n "$CDIR" ] && [ -n "$ACCOUNT" ] && [ -n "$SSHCFG" ] && [ -n "$SOCK" ] && [ -n "$SHA" ] \
    && [ -n "$SID" ] || usage 'requires --state --claude-dir --account --ssh-config --socket --seed-sha256 --session-id'
case $SHA in *[!0-9a-f]*) usage 'sha256 must be lowercase hex' ;; esac
[ ${#SHA} -eq 64 ] || usage 'sha256 must be 64 hex chars'
printf '%s\n' "$SID" | grep -Eqx '[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}' \
    || usage 'session id must be a lowercase UUID'
for p in "$STATE" "$CDIR" "$ACCOUNT" "$SSHCFG" "$SOCK"; do
    case $p in /*) ;; *) usage "absolute path required: $p" ;; esac
    case $p in *'
'*) usage 'newline in path' ;; esac
done
CLAUDE=$CDIR/claude-2.1.303
[ -x "$CLAUDE" ] || refuse "not executable: $CLAUDE"
[ -f "$STATE/seed/benchfs.tar" ] || refuse "missing $STATE/seed/benchfs.tar (run ops-r2-seed.sh place)"
[ -f "$STATE/bin/ops-r2-seed.sh" ] || refuse "missing $STATE/bin/ops-r2-seed.sh"
[ -f "$STATE/prompt.md" ] || refuse "missing $STATE/prompt.md"
SANDBOX=$(dirname -- "$0")/ops-r2-sandbox.sh
# sh の単一引用符で 1 語にする（' は '\'' に）。path は改行を含まないので sed は 1 行だけを見る。
q() { printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"; }
INNER="sh $(q "$STATE/bin/ops-r2-seed.sh") unpack --seed $(q "$STATE/seed/benchfs.tar") --sha256 $SHA --into $(q "$STATE/work/benchfs")"
INNER="$INNER && cd $(q "$STATE/work/benchfs") && CLAUDE_CONFIG_DIR=$(q "$ACCOUNT") exec timeout 3600 $(q "$CLAUDE")"
INNER="$INNER -p --model claude-sonnet-5-5 --session-id $SID --output-format stream-json --verbose"
INNER="$INNER --permission-mode bypassPermissions --disallowedTools Task"
INNER="$INNER < $(q "$STATE/prompt.md") > $(q "$STATE/a-transcript.jsonl") 2> $(q "$STATE/a-stderr.log")"
printf 'sh %s --state %s --release %s --account %s --ssh-config %s --socket %s -- sh -c %s\n' \
    "$(q "$SANDBOX")" "$(q "$STATE")" "$(q "$CDIR")" "$(q "$ACCOUNT")" "$(q "$SSHCFG")" "$(q "$SOCK")" "$(q "$INNER")"
