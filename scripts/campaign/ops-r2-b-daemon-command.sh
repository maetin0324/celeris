#!/bin/sh
# B の測定 daemon を B の sandbox の形で人が起動する command を出す（ADR 2026-10-31-ops-r2-preflight-fixes 付記 D-bstart）。
# 実行はしない。出した 1 行を人が別の端末で実行する。POSIX sh。
#   ops-r2-b-daemon-command.sh --state <B の state> --release <固定した release dir> --claude-dir <claude の固定 copy の dir> \
#     --account <B の account dir> --ssh-config <専用 ssh config> --socket <ssh master の socket> --seed-sha256 <hex>
# 包みの中で種を <state>/work/benchfs に展開してから <release>/bin/celeris --config <state>/daemon.toml --release <sha12> を exec する。
# <state>/work/benchfs は A と同じ path で、ops-r1-measure-config.sh が setup/project.json の workspace に書く（付記 D-bwork）。
# 前提（無ければ exit 1）: <state>/seed/benchfs.tar（ops-r2-seed.sh place）、<state>/bin/ops-r2-seed.sh、<state>/daemon.toml、
# <release>/bin/celeris（実行可）と manifest.json の sha12、<state>/release-store の current → releases/<sha12> → <release>（付記 D-store）、
# <state>/setup/project.json の workspace.path が <state>/work/benchfs、<state>/work/benchfs が無いか空。
# path は改行以外の文字を受け、出す 1 行では sh の単一引用符で 2 段に quote する（@・: を含む socket も可）。
set -eu
refuse() { echo "ops-r2-b-daemon-command: $*" >&2; exit 1; }
usage() { echo "ops-r2-b-daemon-command: $*" >&2; exit 2; }
STATE= RELEASE= CDIR= ACCOUNT= SSHCFG= SOCK= SHA=
while [ $# -gt 0 ]; do
    [ $# -ge 2 ] || usage "missing value for $1"
    case $1 in
        --state) STATE=$2 ;; --release) RELEASE=$2 ;; --claude-dir) CDIR=$2 ;; --account) ACCOUNT=$2 ;;
        --ssh-config) SSHCFG=$2 ;; --socket) SOCK=$2 ;; --seed-sha256) SHA=$2 ;;
        *) usage "unknown argument: $1" ;;
    esac
    shift 2
done
[ -n "$STATE" ] && [ -n "$RELEASE" ] && [ -n "$CDIR" ] && [ -n "$ACCOUNT" ] && [ -n "$SSHCFG" ] && [ -n "$SOCK" ] \
    && [ -n "$SHA" ] || usage 'requires --state --release --claude-dir --account --ssh-config --socket --seed-sha256'
case $SHA in *[!0-9a-f]*) usage 'sha256 must be lowercase hex' ;; esac
[ ${#SHA} -eq 64 ] || usage 'sha256 must be 64 hex chars'
for p in "$STATE" "$RELEASE" "$CDIR" "$ACCOUNT" "$SSHCFG" "$SOCK"; do
    case $p in /*) ;; *) usage "absolute path required: $p" ;; esac
    case $p in *'
'*) usage 'newline in path' ;; esac
done
# measure-config・sandbox と同じく realpath -m で正規化した path で照合し、command にもその path を出す。
STATE=$(realpath -m -- "$STATE")
RELEASE=$(realpath -m -- "$RELEASE")
WORK=$STATE/work/benchfs
CELERIS=$RELEASE/bin/celeris
[ -x "$CELERIS" ] || refuse "not executable: $CELERIS"
SHA12=$(sed -n 's/.*"sha12"[[:space:]]*:[[:space:]]*"\([0-9a-f]*\)".*/\1/p' "$RELEASE/manifest.json" 2>/dev/null | head -n 1)
[ ${#SHA12} -eq 12 ] || refuse "manifest.json lacks a 12-hex sha12: $RELEASE/manifest.json"
[ -f "$STATE/seed/benchfs.tar" ] || refuse "missing $STATE/seed/benchfs.tar (run ops-r2-seed.sh place)"
[ -f "$STATE/bin/ops-r2-seed.sh" ] || refuse "missing $STATE/bin/ops-r2-seed.sh"
[ -f "$STATE/daemon.toml" ] || refuse "missing $STATE/daemon.toml (run ops-r1-measure-config.sh)"
[ "$(readlink "$STATE/release-store/current" 2>/dev/null)" = "releases/$SHA12" ] &&
    [ "$(readlink "$STATE/release-store/releases/$SHA12" 2>/dev/null)" = "$RELEASE" ] ||
    refuse "$STATE/release-store is not current -> releases/$SHA12 -> $RELEASE (ops-r1-measure-config.sh --release-dir)"
python3 -I - "$STATE/setup/project.json" "$WORK" <<'PY' || refuse "$STATE/setup/project.json does not point workspace at $WORK"
import json
import sys
try:
    with open(sys.argv[1], encoding="utf-8") as f:
        ws = json.load(f).get("workspace") or {}
except (OSError, ValueError) as e:
    sys.exit(f"ops-r2-b-daemon-command: {e}")
sys.exit(0 if ws == {"kind": "local", "path": sys.argv[2]} else 1)
PY
if [ -e "$WORK" ] || [ -L "$WORK" ]; then
    [ -d "$WORK" ] && [ ! -L "$WORK" ] && [ -z "$(ls -A "$WORK")" ] || refuse "work tree is not an empty dir: $WORK (start each trial with a new state)"
fi
SANDBOX=$(dirname -- "$0")/ops-r2-sandbox.sh
# sh の単一引用符で 1 語にする（' は '\'' に）。path は改行を含まないので sed は 1 行だけを見る。
q() { printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"; }
INNER="sh $(q "$STATE/bin/ops-r2-seed.sh") unpack --seed $(q "$STATE/seed/benchfs.tar") --sha256 $SHA --into $(q "$WORK")"
INNER="$INNER && exec $(q "$CELERIS") --config $(q "$STATE/daemon.toml") --release $SHA12"
printf 'sh %s --state %s --release %s --account %s --ro %s --ssh-config %s --socket %s -- sh -c %s\n' \
    "$(q "$SANDBOX")" "$(q "$STATE")" "$(q "$RELEASE")" "$(q "$ACCOUNT")" "$(q "$CDIR")" "$(q "$SSHCFG")" "$(q "$SOCK")" "$(q "$INNER")"
