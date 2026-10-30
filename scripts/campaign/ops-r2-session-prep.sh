#!/bin/sh
# ops-r2 の実験専用 operator session を試行の前に用意する（ADR 2026-10-30-ops-r2-isolation-principal D7）。
# LLM・Claude Code・daemon を起動しない。作るのは証跡 dir の中の次の物だけ:
#   claude-config/settings.json  測定専用の CLAUDE_CONFIG_DIR（保持 3650 日、Agent・Task の拒否。CLAUDE.md・memory・skills は置かない）
#   context.md                   初期 context（ops-r2-operator-context.md.tmpl から。試行の識別・使ってよい script・停止条件・記録の規則だけ）
#   session-map.base.json        operator_usage::Mapping の配列（prep・observe。schema は今のまま）
#   session-trials.json          trial_key ↔ condition_id・request_ref
#   bin/                         session の中で使う script の写し（repo は session から見えない）
#   launch.sh                    bwrap の中で Claude Code を --session-id で起動する script（人が実行する）
#   ticket.json・SHA256SUMS      起動票と digest
#
# usage: ops-r2-session-prep.sh --evidence DIR --trials FILE --claude-version V --model M --account-dir D
#          --policy-version V --claude-bin P --campaign-bin P --celerisctl P --token-file F --api-url URL
#          [--session-id UUID] [--now RFC3339]
# --trials: {"trials":[{"trial_key","condition_id","request_ref","prep":{"start","end"},"observe":{"start","end"}}]}
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
die() { echo "ops-r2-session-prep: $*" >&2; exit 2; }

EVIDENCE= TRIALS= CLAUDE_VERSION= MODEL= ACCOUNT= POLICY= CLAUDE_BIN= CAMPAIGN_BIN= CELERISCTL= TOKEN= API_URL=
SESSION= NOW=${CELERIS_OPS_R2_NOW:-}
while [ $# -gt 0 ]; do
    [ $# -ge 2 ] || die "$1 に値が要る"
    case $1 in
    --evidence) EVIDENCE=$2 ;; --trials) TRIALS=$2 ;; --claude-version) CLAUDE_VERSION=$2 ;;
    --model) MODEL=$2 ;; --account-dir) ACCOUNT=$2 ;; --policy-version) POLICY=$2 ;;
    --claude-bin) CLAUDE_BIN=$2 ;; --campaign-bin) CAMPAIGN_BIN=$2 ;; --celerisctl) CELERISCTL=$2 ;;
    --token-file) TOKEN=$2 ;; --api-url) API_URL=$2 ;; --session-id) SESSION=$2 ;; --now) NOW=$2 ;;
    *) die "知らない引数 $1" ;;
    esac
    shift 2
done
for v in EVIDENCE TRIALS CLAUDE_VERSION MODEL ACCOUNT POLICY CLAUDE_BIN CAMPAIGN_BIN CELERISCTL TOKEN API_URL; do
    eval "[ -n \"\$$v\" ]" || die "--$(echo "$v" | tr 'A-Z_' 'a-z-') が要る"
done
case $EVIDENCE in /*) ;; *) die "--evidence は絶対 path" ;; esac
[ ! -e "$EVIDENCE" ] || die "--evidence $EVIDENCE は既にある（作り直しは別の dir に。旧証跡を上書きしない）"
[ -f "$TRIALS" ] || die "--trials $TRIALS が無い"
[ -f "$ACCOUNT/.credentials.json" ] || die "--account-dir に .credentials.json が無い（人が CLAUDE_CONFIG_DIR=<dir> で 1 回 login する）"
ACCOUNT=$(cd "$ACCOUNT" && pwd -P)
# 人が普段使う account dir は渡さない（projects/ に過去の解答・transcript が入っている）。
[ "$ACCOUNT" != "$(cd "${HOME:-/nonexistent}" 2>/dev/null && pwd -P)/.claude" ] || die "--account-dir に人の普段の ~/.claude は使えない"
for f in "$CLAUDE_BIN" "$CAMPAIGN_BIN" "$CELERISCTL"; do [ -x "$f" ] || die "実行できない $f"; done
[ -s "$TOKEN" ] || die "--token-file $TOKEN が空か無い"
if [ -z "$SESSION" ]; then SESSION=$(cat /proc/sys/kernel/random/uuid); fi
echo "$SESSION" | grep -Eq '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$' || die "--session-id は小文字の UUID"
[ -n "$NOW" ] || NOW=$(date -u +%Y-%m-%dT%H:%M:%SZ)

umask 077
mkdir -p "$(dirname "$EVIDENCE")"
mkdir "$EVIDENCE"
CONFIG=$EVIDENCE/claude-config
mkdir "$CONFIG" "$EVIDENCE/bin" "$EVIDENCE/home"
python3 -I "$HERE/ops-r2-session-map.py" base --session-id "$SESSION" --trials "$TRIALS" \
    --out-map "$EVIDENCE/session-map.base.json" --out-trials "$EVIDENCE/session-trials.json" >/dev/null
cp -- "$HERE/ops-r2-poll.sh" "$HERE/ops-r2-human-span.sh" "$EVIDENCE/bin/"
chmod 0500 "$EVIDENCE/bin/ops-r2-poll.sh" "$EVIDENCE/bin/ops-r2-human-span.sh"

# 起動の中の固定 path（bwrap が ro で bind する。host の path は session から見えない）。
IN_BIN=/opt/ops-r2/bin IN_TOKEN=/opt/ops-r2/token
python3 -I - "$HERE/ops-r2-operator-context.md.tmpl" "$EVIDENCE" "$SESSION" "$API_URL" "$IN_TOKEN" "$IN_BIN" \
    "$CONFIG" <<'PY'
import json, sys
tmpl, ev, session, api, token, inbin, config = sys.argv[1:]
trials = json.load(open(f"{ev}/session-trials.json"))["trials"]
rows = "\n".join(f"- trial_key `{t['trial_key']}` / condition `{t['condition_id']}` / request `{t['request_ref']}`"
                 for t in trials)
text = open(tmpl, encoding="utf-8").read()
for k, v in {"SESSION_ID": session, "TRIALS": rows, "SCRIPTS": f"{ev}/bin", "CAMPAIGN_BIN": f"{inbin}/celeris-campaign",
             "EVIDENCE": ev, "API_URL": api, "TOKEN_FILE": token}.items():
    text = text.replace("{{" + k + "}}", v)
if "{{" in text:
    sys.exit("context: 埋まらない placeholder がある")
open(f"{ev}/context.md", "w", encoding="utf-8").write(text)
settings = {"cleanupPeriodDays": 3650, "permissions": {"deny": ["Agent", "Task"]}}
open(f"{config}/settings.json", "w", encoding="utf-8").write(json.dumps(settings, indent=2, sort_keys=True) + "\n")
PY

q() { printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"; }
{
    echo '#!/bin/sh'
    echo '# 人が実行する。ops-r2-session-prep.sh が生成した。測定専用の CLAUDE_CONFIG_DIR で、証跡 dir と固定の道具だけが見える。'
    echo 'set -eu'
    echo "EVIDENCE=$(q "$EVIDENCE")"
    echo 'exec bwrap --unshare-pid --unshare-ipc --new-session --die-with-parent --clearenv \'
    echo '    --proc /proc --dev /dev --tmpfs /tmp --ro-bind /usr /usr --ro-bind /etc /etc \'
    echo '    --ro-bind-try /bin /bin --ro-bind-try /sbin /sbin --ro-bind-try /lib /lib --ro-bind-try /lib64 /lib64 \'
    echo '    --bind "$EVIDENCE" "$EVIDENCE" \'
    echo "    --ro-bind $(q "$ACCOUNT/.credentials.json") \"\$EVIDENCE/claude-config/.credentials.json\" \\"
    echo "    --ro-bind $(q "$CLAUDE_BIN") $IN_BIN/claude --ro-bind $(q "$CAMPAIGN_BIN") $IN_BIN/celeris-campaign \\"
    echo "    --ro-bind $(q "$CELERISCTL") $IN_BIN/celerisctl --ro-bind $(q "$TOKEN") $IN_TOKEN \\"
    echo "    --setenv HOME \"\$EVIDENCE/home\" --setenv CLAUDE_CONFIG_DIR \"\$EVIDENCE/claude-config\" \\"
    echo "    --setenv PATH $IN_BIN:/usr/bin:/bin --setenv CELERIS_API_URL $(q "$API_URL") --chdir \"\$EVIDENCE\" \\"
    echo "    $IN_BIN/claude --session-id $SESSION --model $(q "$MODEL") \\"
    echo '    --append-system-prompt "$(cat "$EVIDENCE/context.md")" "$@"'
} >"$EVIDENCE/launch.sh"
chmod 0500 "$EVIDENCE/launch.sh"

sha() { sha256sum -- "$1" | cut -d' ' -f1; }
# Claude Code は cwd の英数字以外を '-' にした名前の dir に transcript を書く（2.1 系で確認した形。別の版は起動後に確かめる）。
PROJ=$(printf '%s' "$EVIDENCE" | sed 's/[^A-Za-z0-9]/-/g')
python3 -I - "$EVIDENCE/ticket.json" "$SESSION" "$NOW" "$CLAUDE_VERSION" "$MODEL" "$ACCOUNT" "$POLICY" "$API_URL" \
    "$CONFIG" "$CONFIG/projects/$PROJ" \
    context.md="$(sha "$EVIDENCE/context.md")" settings.json="$(sha "$CONFIG/settings.json")" \
    session-map.base.json="$(sha "$EVIDENCE/session-map.base.json")" \
    session-trials.json="$(sha "$EVIDENCE/session-trials.json")" launch.sh="$(sha "$EVIDENCE/launch.sh")" \
    claude="$(sha "$CLAUDE_BIN")" celeris-campaign="$(sha "$CAMPAIGN_BIN")" celerisctl="$(sha "$CELERISCTL")" <<'PY'
import json, sys
out, session, now, version, model, account, policy, api, config, proj, *digests = sys.argv[1:]
t = {"schema": "ops-r2-operator-session/1", "session_id": session, "created_at": now,
     "claude_code_version": version, "model": model, "account_dir": account, "policy_version": policy,
     "api_url": api, "claude_config_dir": config,
     "transcript_expected": f"{proj}/{session}.jsonl", "subagents_expected": f"{proj}/{session}/subagents",
     "sha256": dict(d.split("=", 1) for d in digests)}
open(out, "w", encoding="utf-8").write(json.dumps(t, ensure_ascii=False, indent=2, sort_keys=True) + "\n")
PY
(cd "$EVIDENCE" && sha256sum -- context.md claude-config/settings.json session-map.base.json session-trials.json \
    launch.sh ticket.json bin/ops-r2-poll.sh bin/ops-r2-human-span.sh >SHA256SUMS)
printf '{"session_id":"%s","evidence":"%s"}\n' "$SESSION" "$EVIDENCE"
