#!/bin/sh
# ADR 2026-10-30-ops-r2-isolation-principal D2. Network is shared.
set -eu
set -f
refuse() { echo "ops-r2-sandbox: $*" >&2; exit 2; }
canon() {
    case $1 in ''|*'
'*) refuse 'empty path or newline in path' ;; esac
    realpath -m -- "$1"
}
STATE= RELEASE= ACCOUNT= ROS= PRINT=0 SEPARATOR=0
while [ $# -gt 0 ]; do
    case $1 in
        --print-args) PRINT=1; shift ;;
        --) SEPARATOR=1; shift; break ;;
        --state|--release|--account|--ro)
            [ $# -ge 2 ] || refuse "missing value for $1"
            path=$(canon "$2")
            case $1 in
                --state) STATE=$path ;;
                --release) RELEASE=$path ;;
                --account) ACCOUNT=$path ;;
                --ro) ROS="$ROS
$path" ;;
            esac
            shift 2 ;;
        *) refuse "unknown argument: $1" ;;
    esac
done
[ -n "$STATE" ] && [ -n "$RELEASE" ] && [ -n "$ACCOUNT" ] && [ "$SEPARATOR" = 1 ] && [ $# -gt 0 ] || refuse 'requires --state --release --account -- command'
# Validate canonical sources, including ancestors that would expose forbidden trees.
# Python is used only for path validation; the mount argument table below is shared
# unchanged by --print-args and the actual exec.
python3 -I - "$STATE" "$RELEASE" "$ACCOUNT" "$ROS" <<'PY' || exit 2
import os
import sys
from pathlib import Path
sources = sys.argv[1:4] + sys.argv[4].splitlines()
home = Path.home()
forbidden = [home / '.claude', home / '.config/celeris',
             home / '.local/share/celeris', home / '.local/celeris',
             Path('/local/celeris/data/workspaces'), Path('/local/celeris/state'),
             Path('/home/rmaeda/workspace/agent-platform')]
if os.environ.get('CELERIS_KNOWLEDGE_ROOT'):
    forbidden.append(Path(os.environ['CELERIS_KNOWLEDGE_ROOT']))
def overlaps(a, b):
    return a == b or a in b.parents or b in a.parents
for source in filter(None, sources):
    p = Path(source)
    def denied(f):
        f = Path(os.path.realpath(f))
        # The selected immutable release is allowed inside the production
        # release store; the store itself and sibling releases are not mounted.
        stores = [Path('/local/celeris/state/releases'), home / '.local/celeris/releases']
        if source == sys.argv[2] and any(
                Path(os.path.realpath(store)) in p.parents for store in stores):
            if f in (Path('/local/celeris/state'), home / '.local/celeris'):
                return False
        return overlaps(p, f)
    if any(denied(f) for f in forbidden):
        sys.exit(f'ops-r2-sandbox: forbidden path: {p}')
    if not p.is_dir() and not p.is_file():
        sys.exit(f'ops-r2-sandbox: path does not exist: {p}')
    if any((a / '.git').is_file() or (a / '.git/HEAD').is_file()
           for a in [p, *p.parents]):
        sys.exit(f'ops-r2-sandbox: repo clone: {p}')
    if p.is_dir():
        for root, dirs, files in os.walk(p, followlinks=False):
            if '.git' in dirs or '.git' in files:
                sys.exit(f'ops-r2-sandbox: contains repo clone: {p}')
# Authentication is a dedicated account, never an earlier trial transcript.
for root, dirs, files in os.walk(sys.argv[3]):
    if 'projects' in dirs or 'memory' in dirs:
        sys.exit('ops-r2-sandbox: account contains projects or memory')
PY
[ -d "$STATE/home" ] || { [ "$PRINT" = 1 ] || mkdir -p "$STATE/home"; }
# Preserve command argv, including spaces and empty arguments.
set -- -- "$@"
OLD_IFS=$IFS
IFS='
'
for path in $ROS; do
    [ -n "$path" ] || continue
    set -- --ro-bind "$path" "$path" "$@"
done
IFS=$OLD_IFS
set -- --bind "$STATE" "$STATE" --setenv HOME "$STATE/home" --chdir "$STATE" --bind "$ACCOUNT" "$ACCOUNT" "$@"
for path in /usr /bin /sbin /lib /lib64 /etc; do
    [ -e "$path" ] || continue
    set -- --ro-bind "$path" "$path" "$@"
done
set -- --unshare-pid --unshare-ipc --new-session --die-with-parent --clearenv --setenv PATH /usr/bin:/bin --tmpfs /tmp --proc /proc --dev /dev --ro-bind "$RELEASE" "$RELEASE" "$@"
if [ "$PRINT" = 1 ]; then
    printf '%s\n' "$@"
else
    exec bwrap "$@"
fi
