#!/bin/sh
# A・B の初期作業ツリーの種（ADR 2026-10-31-ops-r2-preflight-fixes 付記 D-seed・D-seed2）。POSIX sh。
#   ops-r2-seed.sh make   --archive <公開の初期 tar> --case <case.json> --assets <asset store> --out <tar>
#                                                                           # submodule 込みの非圧縮 tar を作り sha256 を出す
#   ops-r2-seed.sh place  --seed <tar> --sha256 <hex> --state <dir>         # 検査して <state>/seed/benchfs.tar に写す
#   ops-r2-seed.sh unpack --seed <tar> --sha256 <hex> --into <dir>          # sandbox の中で展開する（dir は無いか空）
# make は公開の初期 tar（git archive、submodule なし）の sha256 を case.json の initial.archive_sha256 と照合し、
# initial.submodules の各 tar（<assets>/submodules/<末尾名>-<commit 先頭 8 桁>.tar）を <assets>/SHA256SUMS と照合して
# 浅い path から順に <path>/ の下へ入れる。公開 tar 自体は変えない。同じ入力からは同じ bytes の種になる。
# 種は非圧縮の tar 1 つ。圧縮しないので漏洩検査（check-ops-r1-leak.py --scan <state>）が member の中身を読める。
# .git の member・絶対 path・`..`・木の外を指す link を含む tar は拒む（ops-r2-sandbox.sh の .git 拒否と同じ理由:
# 試行の主体に履歴を見せない）。exit: 0 成功 / 1 検査で拒否 / 2 使い方の誤り。
set -eu
refuse() { echo "ops-r2-seed: $*" >&2; exit 1; }
usage() { echo "ops-r2-seed: $*" >&2; exit 2; }
[ $# -ge 1 ] || usage 'requires make | place | unpack'
CMD=$1; shift
ARCHIVE= CASE= ASSETS= OUT= SEED= SHA= STATE= INTO=
while [ $# -gt 0 ]; do
    [ $# -ge 2 ] || usage "missing value for $1"
    case $1 in
        --archive) ARCHIVE=$2 ;; --case) CASE=$2 ;; --assets) ASSETS=$2 ;; --out) OUT=$2 ;; --seed) SEED=$2 ;;
        --sha256) SHA=$2 ;; --state) STATE=$2 ;; --into) INTO=$2 ;;
        *) usage "unknown argument: $1" ;;
    esac
    shift 2
done
check_tar() {
    python3 -I - "$1" <<'PY' || return 1
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
compose() {
    python3 -I - "$@" <<'PY'
import hashlib, json, os, posixpath, re, sys, tarfile
archive, case_path, assets, out = sys.argv[1:5]
def die(m):
    sys.exit('ops-r2-seed: ' + m)
def sha(p):
    h = hashlib.sha256()
    try:
        with open(p, 'rb') as f:
            for b in iter(lambda: f.read(1 << 20), b''):
                h.update(b)
    except OSError as e:
        die(f'cannot read {p}: {e}')
    return h.hexdigest()
try:
    with open(case_path, encoding='utf-8') as f:
        initial = json.load(f).get('initial')
except (OSError, ValueError) as e:
    die(f'case.json unreadable: {e}')
if not isinstance(initial, dict) or not isinstance(initial.get('archive_sha256'), str):
    die('case.json has no initial.archive_sha256')
got = sha(archive)
if got != initial['archive_sha256']:
    die(f'sha256 mismatch: {archive} is {got}, case.json initial.archive_sha256 is {initial["archive_sha256"]}')
sums = {}
try:
    with open(os.path.join(assets, 'SHA256SUMS'), encoding='utf-8') as f:
        for line in f:
            parts = line.split(None, 1)
            if len(parts) == 2:
                sums[parts[1].strip().lstrip('*')] = parts[0]
except OSError as e:
    die(f'SHA256SUMS unreadable: {e}')
subs = []
for s in initial.get('submodules', []):
    m = re.fullmatch(r'([^@]+)@([0-9a-f]{40})', s) if isinstance(s, str) else None
    if not m:
        die(f'submodule must be <path>@<40-hex commit>: {s!r}')
    path, commit = m.group(1).strip('/'), m.group(2)
    if not path or '..' in path.split('/'):
        die(f'unsafe submodule path: {path}')
    key = f'submodules/{path.rsplit("/", 1)[-1]}-{commit[:8]}.tar'
    if key not in sums:
        die(f'{key} (submodule {path}) is not listed in SHA256SUMS')
    tar = os.path.join(assets, key)
    if sha(tar) != sums[key]:
        die(f'{key} (submodule {path}) differs from SHA256SUMS')
    subs.append((path, tar))
subs.sort(key=lambda x: (x[0].count('/'), x[0]))
seen = {}
def norm(n):
    n = n[2:] if n.startswith('./') else n
    return n.rstrip('/')
with tarfile.open(out, 'w', format=tarfile.PAX_FORMAT) as dst:
    for prefix, src in [('', archive)] + subs:
        if prefix and seen.get(prefix) is not True:
            die(f'submodule path {prefix} has no directory in the tree')
        with tarfile.open(src, 'r:') as t:
            for m in t:
                rel = norm(m.name)
                if not rel or rel == '.':
                    continue
                name = posixpath.join(prefix, rel) if prefix else rel
                if name in seen:
                    if m.isdir() and seen[name]:
                        continue
                    die(f'member appears twice: {name} ({src})')
                seen[name] = m.isdir()
                m.name = name
                m.uid = m.gid = 0
                m.uname = m.gname = ''
                m.pax_headers = {}
                dst.addfile(m, t.extractfile(m) if m.isfile() else None)
print(f'ops-r2-seed: archive {got} + {len(subs)} submodule tar(s)', file=sys.stderr)
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
        [ -n "$ARCHIVE" ] && [ -n "$CASE" ] && [ -n "$ASSETS" ] && [ -n "$OUT" ] \
            || usage 'make requires --archive --case --assets --out'
        [ ! -e "$OUT" ] || refuse "out exists: $OUT"
        compose "$ARCHIVE" "$CASE" "$ASSETS" "$OUT.tmp" || { rm -f "$OUT.tmp"; exit 1; }
        check_tar "$OUT.tmp" || { rm -f "$OUT.tmp"; exit 1; }
        mv "$OUT.tmp" "$OUT"
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
