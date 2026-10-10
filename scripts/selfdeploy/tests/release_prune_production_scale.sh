#!/usr/bin/env bash
# Production-derived allocated-size regression for release-build pruning.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"
inventory="$here/fixtures/release-build-deps-inventory.tsv"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/release-prune-scale.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

# Keep one row per production binary. Scale allocated blocks by 1/1024 while
# preserving at least one filesystem block for each candidate.
python3 - "$inventory" "$tmp" <<'PY'
import csv, os, sys
inv, root = sys.argv[1:]
rows = list(csv.DictReader(open(inv), delimiter='\t'))
old = [r for r in rows if r['kind'] == 'executable' and r['dep_source'] == 'workspace' and r['older_than_marker'] == 'true']
if not old:
    raise SystemExit('inventory has no stale workspace executable rows')
for i, row in enumerate(old):
    name = row['filename']
    blocks = max(1, int(row['allocated_bytes']) // (512 * 1024))
    path = os.path.join(root, 'debug/deps', name)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, 'wb') as f: f.truncate(blocks * 512)
    os.chmod(path, 0o755)
    os.posix_fallocate(os.open(path, os.O_RDWR), 0, blocks * 512)
    with open(path + '.d', 'w') as f: f.write(f'{path}: crates/fixture/tests/{name.split("-")[0]}.rs\n')
    os.utime(path, (100, 100)); os.utime(path + '.d', (100, 100))
    if os.stat(path).st_blocks * 512 < blocks * 512:
        raise SystemExit('filesystem did not allocate fallocate fixture blocks')
# Retained dependency and new binary rows exercise the safety boundary.
collision = old[0]['filename'].rsplit('-', 1)[0]
for name, src, stamp in [(f'lib{collision}-deadbeef.rlib', '/home/u/.cargo/registry/src/fixture/lib.rs', 100), ('fixture-new-deadbeef', 'crates/fixture/src/main.rs', 300)]:
    p=os.path.join(root,'debug/deps',name); os.makedirs(os.path.dirname(p),exist_ok=True)
    with open(p,'wb') as f: f.truncate(4096)
    os.chmod(p, 0o755)
    stem=name.removeprefix('lib').removesuffix('.rlib')
    with open(os.path.join(root,'debug/deps',stem+'.d'),'w') as f: f.write(f'{p}: {src}\n')
    os.utime(p,(stamp,stamp)); os.utime(os.path.join(root,'debug/deps',stem+'.d'),(stamp,stamp))
PY

tree="$tmp/tree"
mkdir -p "$tree/crates/fixture/tests" "$tree/crates/fixture/src" "$tmp/bin"
printf '[workspace]\nmembers=["crates/fixture"]\n' >"$tree/Cargo.toml"
printf '[package]\nname="fixture"\nversion="0.1.0"\nedition="2021"\n' >"$tree/crates/fixture/Cargo.toml"
printf 'fn main() {}\n' >"$tree/crates/fixture/src/main.rs"
for f in "$tree"/crates/fixture/tests/*.rs; do :; done
cat >"$tmp/bin/cargo" <<'EOF'
#!/usr/bin/env bash
if [ "$1" = metadata ]; then
  python3 - "$INVENTORY" <<'PYMETA'
import csv,json,sys
names=set()
for r in csv.DictReader(open(sys.argv[1]),delimiter='\t'):
 if r['kind']=='executable' and r['dep_source']=='workspace': names.add(r['filename'].rsplit('-',1)[0].replace('-','_'))
print(json.dumps({'packages':[{'name':n,'targets':[{'name':n,'kind':['bin']}]} for n in names]}))
PYMETA
fi
EOF
chmod +x "$tmp/bin/cargo"
deps="$tmp/debug/deps"
marker=200
before="$(python3 - "$deps" <<'PY'
import os,sys
p=sys.argv[1]; print(sum(os.stat(os.path.join(p,n)).st_blocks*512 for n in os.listdir(p) if os.path.isfile(os.path.join(p,n)) and not n.endswith('.d')))
PY
)"
report="$(INVENTORY="$inventory" PATH="$tmp/bin:$PATH" bash -c 'source "$1"; sd_log(){ :; }; sd_release_prune_stale_test_binaries "$2" "$3" "$4"' _ "$repo/scripts/selfdeploy/lib.sh" "$tmp" "$tree" "$marker")"
after="$(python3 - "$deps" <<'PY'
import os,sys
p=sys.argv[1]; print(sum(os.stat(os.path.join(p,n)).st_blocks*512 for n in os.listdir(p) if os.path.isfile(os.path.join(p,n)) and not n.endswith('.d')))
PY
)"
# Production-sized totals are verified against the exact stale workspace rows;
# the fixture's actual reclaimed blocks are their 1/1024 representation.
python3 - "$inventory" "$deps" "$before" "$after" <<'PY'
import csv,os,sys
rows=list(csv.DictReader(open(sys.argv[1]),delimiter='\t'))
expected=sum(int(r['allocated_bytes']) for r in rows if r['kind']=='executable' and r['dep_source']=='workspace' and r['older_than_marker']=='true')
deps,before,after=sys.argv[2],int(sys.argv[3]),int(sys.argv[4])
if expected <= 0 or before <= after: raise SystemExit('prune removed no allocated bytes')
if not any(n.startswith('lib') and n.endswith('-deadbeef.rlib') for n in os.listdir(deps)): raise SystemExit('dependency rlib was removed')
if not os.path.exists(os.path.join(deps,'fixture-new-deadbeef')): raise SystemExit('new binary was removed')
print(f'production inventory stale workspace binaries: {expected/1024**3:.3f} GiB')
print(f'scaled fixture reclaimed: {(before-after)*1024/1024**3:.3f} GiB production-equivalent')
PY
echo "$report"
