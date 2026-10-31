#!/bin/sh
# A・B の初期作業ツリーの種（ADR 2026-10-31-ops-r2-preflight-fixes 付記 D-seed）。POSIX sh。
#   ops-r2-seed.sh make   --repo <git の作業ツリー> --rev <rev> --out <tar>   # 非圧縮 tar（.git なし）を作り sha256 を出す
#   ops-r2-seed.sh place  --seed <tar> --sha256 <hex> --state <dir>         # 検査して <state>/seed/benchfs.tar に写す
#   ops-r2-seed.sh unpack --seed <tar> --sha256 <hex> --into <dir>          # sandbox の中で展開する（dir は無いか空）
# 種は非圧縮の tar 1 つ。圧縮しないので漏洩検査（check-ops-r1-leak.py --scan <state>）が member の中身を読める。
# .git の member・絶対 path・`..`・木の外を指す link を含む tar は拒む（ops-r2-sandbox.sh の .git 拒否と同じ理由:
# 試行の主体に履歴を見せない）。exit: 0 成功 / 1 検査で拒否 / 2 使い方の誤り。
set -eu
refuse() { echo "ops-r2-seed: $*" >&2; exit 1; }
usage() { echo "ops-r2-seed: $*" >&2; exit 2; }
[ $# -ge 1 ] || usage 'requires make | place | unpack'
CMD=$1; shift
REPO= REV= OUT= SEED= SHA= STATE= INTO=
while [ $# -gt 0 ]; do
    [ $# -ge 2 ] || usage "missing value for $1"
    case $1 in
        --repo) REPO=$2 ;; --rev) REV=$2 ;; --out) OUT=$2 ;; --seed) SEED=$2 ;;
        --sha256) SHA=$2 ;; --state) STATE=$2 ;; --into) INTO=$2 ;;
        *) usage "unknown argument: $1" ;;
    esac
    shift 2
done
check_tar() {
    python3 -I - "$1" <<'PY' || exit 1
import posixpath, sys, tarfile
bad = []
with tarfile.open(sys.argv[1], mode='r:') as t:
    for m in t.getmembers():
        n = m.name.rstrip('/')
        parts = n.split('/')
        if n.startswith('/') or '..' in parts:
            bad.append(f'unsafe path: {m.name}')
        elif '.git' in parts:
            bad.append(f'.git member: {m.name}')
        elif m.issym() or m.islnk():
            base = posixpath.dirname(n) if m.issym() else ''
            target = posixpath.normpath(posixpath.join(base, m.linkname))
            if m.linkname.startswith('/') or target == '..' or target.startswith('../'):
                bad.append(f'link outside the tree: {m.name} -> {m.linkname}')
        elif not (m.isfile() or m.isdir()):
            bad.append(f'special member: {m.name}')
if bad:
    sys.exit('ops-r2-seed: ' + '; '.join(bad))
PY
}
verify() {
    [ -f "$SEED" ] || refuse "seed is not a file: $SEED"
    case $SHA in *[!0-9a-f]*|'') usage '--sha256 must be lowercase hex' ;; esac
    [ ${#SHA} = 64 ] || usage '--sha256 must be 64 hex digits'
    got=$(sha256sum "$SEED" | cut -d ' ' -f 1)
    [ "$got" = "$SHA" ] || refuse "sha256 mismatch: $SEED is $got, expected $SHA"
    check_tar "$SEED"
}
case $CMD in
    make)
        [ -n "$REPO" ] && [ -n "$REV" ] && [ -n "$OUT" ] || usage 'make requires --repo --rev --out'
        [ ! -e "$OUT" ] || refuse "out exists: $OUT"
        git -C "$REPO" archive --format=tar -o "$OUT" "$REV" || refuse "git archive failed: $REPO $REV"
        check_tar "$OUT"
        sha256sum "$OUT" | cut -d ' ' -f 1 ;;
    place)
        [ -n "$SEED" ] && [ -n "$SHA" ] && [ -n "$STATE" ] || usage 'place requires --seed --sha256 --state'
        verify
        dest="$STATE/seed/benchfs.tar"
        if [ -e "$dest" ]; then
            [ "$(sha256sum "$dest" | cut -d ' ' -f 1)" = "$SHA" ] || refuse "existing seed differs: $dest"
        else
            mkdir -p "$STATE/seed"
            cp "$SEED" "$dest"
            chmod 0444 "$dest"
        fi
        echo "$dest" ;;
    unpack)
        [ -n "$SEED" ] && [ -n "$SHA" ] && [ -n "$INTO" ] || usage 'unpack requires --seed --sha256 --into'
        verify
        if [ -e "$INTO" ]; then
            [ -d "$INTO" ] && [ -z "$(ls -A "$INTO")" ] || refuse "into is not an empty dir: $INTO"
        fi
        mkdir -p "$INTO"
        tar -xf "$SEED" -C "$INTO" --no-same-owner --no-same-permissions ;;
    *) usage "unknown command: $CMD" ;;
esac
