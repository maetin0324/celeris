#!/usr/bin/env bash
# Eight production-scaled release builds against the real pruning functions.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"
exec python3 - "$repo" "$here/fixtures/release-build-deps-inventory.tsv" <<'PY'
import csv, json, os, pathlib, subprocess, sys, tempfile

repo, inventory = sys.argv[1:]
rows = list(csv.DictReader(open(inventory), delimiter='\t'))
bins = [r for r in rows if r['kind'] == 'executable' and r['dep_source'] == 'workspace']
if len(bins) != 199: raise SystemExit(f'expected 199 workspace executables, got {len(bins)}')
scale = 1024 * 4096
weights = [max(1, (int(r['allocated_bytes']) + scale - 1) // scale) for r in bins]
# Deterministic four-way weighted partition: each release adds about 13 GiB.
groups = [[] for _ in range(4)]; totals = [0] * 4
for i in sorted(range(len(bins)), key=lambda i: (-weights[i], bins[i]['filename'])):
    j = min(range(4), key=lambda j: totals[j]); groups[j].append(i); totals[j] += weights[i]
names = sorted({r['filename'].rsplit('-', 1)[0].replace('-', '_') for r in bins})

def allocated(root):
    return sum(p.stat().st_blocks * 512 for p in pathlib.Path(root, 'debug/deps').iterdir() if p.is_file())

def setup(label):
    base = pathlib.Path(tempfile.mkdtemp(prefix='release-prune-scale-', dir=os.environ.get('TMPDIR', '/tmp')))
    target, tree, bindir = base/'target', base/'tree', base/'bin'
    deps = target/'debug/deps'; deps.mkdir(parents=True); bindir.mkdir(); (tree/'crates/fixture/src').mkdir(parents=True)
    (tree/'Cargo.toml').write_text('[workspace]\nmembers=["crates/fixture"]\n')
    (tree/'crates/fixture/Cargo.toml').write_text('[package]\nname="fixture"\nversion="0.1.0"\nedition="2021"\n')
    cargo = bindir/'cargo'
    cargo.write_text('#!/usr/bin/env python3\nimport json\nnames='+repr(names)+'\nprint(json.dumps({"packages":[{"name":n,"targets":[{"name":n,"kind":["bin"]}]} for n in names]}))\n')
    cargo.chmod(0o755)
    env = os.environ.copy(); env['PATH'] = str(bindir) + os.pathsep + env['PATH']; env['SD_RELEASE_TARGET_MAX_BYTES'] = str(10**15)
    env['SD_RELEASE_TARGET_SEED'] = str(base/'seed'); (base/'seed').mkdir()
    # Inventory's 199 stale binaries form one baseline generation.
    for i, row in enumerate(bins): create(deps, row, f'{i:016x}', 100)
    # Registry artifacts are explicitly retained by the real candidate filter.
    dep = deps/'libserde-deadbeef.rlib'; dep.write_bytes(b'R' * 4096); os.utime(dep, (100,100))
    d = deps/'serde-deadbeef.d'; d.write_text(str(dep)+': /home/user/.cargo/registry/src/serde/lib.rs\n'); os.utime(d,(100,100))
    return base, target, tree, env

def create(deps, row, h, stamp):
    name = row['filename'].rsplit('-', 1)[0] + '-' + h
    p = deps/name; blocks = max(1, (int(row['allocated_bytes']) + scale - 1) // scale)
    fd = os.open(p, os.O_CREAT|os.O_RDWR, 0o755)
    try: os.posix_fallocate(fd, 0, blocks * 4096)
    finally: os.close(fd)
    p.chmod(0o755)
    d = deps/(name+'.d'); d.write_text(f'{p}: crates/fixture/tests/{row["filename"].split("-")[0]}.rs\n')
    os.utime(p, (stamp,stamp)); os.utime(d,(stamp,stamp))

def invoke(target, tree, env, marker, prune):
    code = 'source "$1"; sd_log(){ :; }; '
    if prune: code += 'sd_release_prune_stale_test_binaries "$2" "$3" "$4"; '
    code += 'sd_release_prune_enforce_limit "$2"; sd_release_prune_record_start "$2"'
    subprocess.run(['bash','-c',code,'_',repo+'/scripts/selfdeploy/lib.sh',str(target),str(tree),str(marker)],check=True,env=env)

def series(label, prune):
    base,target,tree,env=setup(label); deps=target/'debug/deps'; marker=200; amounts=[]; maxsize=0; recreated=False
    for release in range(1,9):
        # Release 1 records the prior build's marker after baseline creation; later
        # iterations prune using that prior marker, then record the new start.
        if release > 1:
            deps.mkdir(parents=True, exist_ok=True)
            before=set(p.name for p in deps.iterdir())
            invoke(target,tree,env,marker,prune)
            deps=target/'debug/deps'
            deps.mkdir(parents=True, exist_ok=True)
            after=set(p.name for p in deps.iterdir())
        marker=200+release*10
        # First release has the same prune/enforce/record ordering as production.
        if release == 1: invoke(target,tree,env,100,prune)
        deps.mkdir(parents=True, exist_ok=True)
        for i in groups[(release-1)%4]: create(deps,bins[i],f'{release:02x}{i:014x}',marker+1)
        amount=allocated(target); amounts.append(amount); maxsize=max(maxsize,amount)
        if amount <= 68719476736//1024: recreated=True
        print(f'{label} release={release} allocated_gib={amount*1024/1024**3:.3f}')
    return base,target,tree,env,amounts,maxsize,recreated

# Each series is isolated; A gets one final catch-up prune with its last recorded marker.
a=series('A',False); b=series('B',True)
_,ta,treea,enva,aa,_,reca=a; _,tb,treeb,envb,bb,maxb,recb=b
depsa=ta/'debug/deps'; before=allocated(ta)
no_prune_bins=sum(1 for p in depsa.iterdir() if p.is_file() and not p.name.endswith('.d') and not p.name.endswith('.rlib'))
invoke(ta,treea,enva,280,True); after=allocated(ta)
reclaimed=(before-after)*1024/1024**3
no_prune_gib=before*1024/1024**3
prune_max=max(bb)*1024/1024**3; prune_final=bb[-1]*1024/1024**3
# Include one filesystem block per generated depfile plus the two registry files.
bound=(sum(weights)+max(totals)+len(bins)+max(map(len, groups))+2)*4096*1024/1024**3
# Validate nonincrease after each release's prune-before-build behavior.
nonincreasing=int(all(bb[i] <= bb[i-1] + max(totals)*512 for i in range(1,8)))
deps_kept=int((tb/'debug/deps/libserde-deadbeef.rlib').exists() and (tb/'debug/deps/serde-deadbeef.d').exists())
# Exercise the scaled 64 GiB threshold independently so the limit's intentional
# target recreation does not erase the eight-release accumulation measurements.
def limit_probe(prune):
    base,t,tree,env=setup('limit'); deps=t/'debug/deps'; env['SD_RELEASE_TARGET_MAX_BYTES']=str(68719476736//1024)
    for i in groups[0]: create(deps,bins[i],f'face{i:012x}',201)
    invoke(t,tree,env,200,prune)
    return not (t/'debug/deps/libserde-deadbeef.rlib').exists()
recreate_without=limit_probe(False)
recreate_with=limit_probe(True)
reca=int(recreate_without); recb=int(recreate_with)
print('RESULT no_prune_bins=%d no_prune_gib=%.3f catchup_reclaimed_gib=%.3f prune_max_gib=%.3f prune_final_gib=%.3f prune_bound_gib=%.3f nonincreasing=%d limit_recreate_without_prune=%d limit_recreate_with_prune=%d deps_kept=%d' % (no_prune_bins,no_prune_gib,reclaimed,prune_max,prune_final,bound,nonincreasing,reca,recb,deps_kept))
if no_prune_bins < 370 or no_prune_gib < 100 or reclaimed < no_prune_gib-bound or prune_max > bound or prune_final > prune_max or not nonincreasing or not reca or recb or not deps_kept:
    raise SystemExit('production-scale pruning assertions failed')
PY
