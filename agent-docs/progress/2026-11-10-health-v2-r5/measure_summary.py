#!/usr/bin/env python3
"""R5 comparison decisions and measurement orchestration (no synthetic load)."""
import json
import math
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time

HERE = Path(__file__).resolve().parent
BEGIN, END = '<!-- remeasure:begin -->', '<!-- remeasure:end -->'


def peak(v):
    values = [float(v[k].split()[0]) for k in ('loadavg_before', 'loadavg_after')]
    if any(not math.isfinite(x) or x < 0 for x in values):
        raise ValueError('invalid load')
    return max(values)


def pair(base, head):
    # Excluded exactly when any reason applies: ratio undefined or >= 2, a load
    # wait not ready, a non-zero/missing exit, or a missing/invalid timing.
    try:
        b, h = peak(base), peak(head)
        ratio = max(b, h) / min(b, h) if min(b, h) else (1 if b == h else None)
        exits = ('exit',) if 'exit' in base else ('settle_exit', 'list_exit', 'rebuild_exit', 'doc_test_exit')
        timing = 'secs' if 'exit' in base else 'edit_to_verify_with_doc_secs'
        sides = (('base', base), ('head', head))
        reasons = []
        if ratio is None or ratio >= 2:
            reasons.append('load ratio >= 2')
        if not all(v.get('load_wait', {}).get('ready', False) for v in (base, head)):
            reasons.append('load wait expired')
        reasons += [f'{s} {k} {json.dumps(v[k])}' for s, v in sides for k in exits if v.get(k) not in (None, 0)]
        if any(v.get(k) is None for v in (base, head) for k in exits) or not all(
                isinstance(v.get(timing), (int, float)) and math.isfinite(v[timing]) and v[timing] >= 0 for v in (base, head)):
            reasons.append('missing or invalid measurement')
        return {'ratio': ratio, 'excluded': bool(reasons), 'reason': '; '.join(reasons) or None,
                'base_load_1m_max': b, 'head_load_1m_max': h}
    except (KeyError, ValueError, TypeError, IndexError):
        return {'ratio': None, 'excluded': True, 'reason': 'missing or invalid measurement'}


ALL_ITEMS = ('build', 'edit_verify', 'run')


def schema_items(d):
    return ('run',) if d['schema'] == 'health-v2-r5-run-compare/2' else ('build', 'edit_verify')


def comparisons(d):
    rows = []
    for a in d.get('attempts', []):
        for c in a.get('crates', []):
            for item in schema_items(d):
                for n, (b, h) in enumerate(a.get('pairs', {}).get(c, {}).get(item, [])):
                    rows.append(dict(crate=c, item=item, attempt=a['attempt'], pair=n + 1,
                                     base=b, head=h, **pair(b, h)))
    return rows


def parse_spec(spec):
    """'<crate>' or '<crate>:<item>' -> (crate, item or None); ValueError if malformed."""
    crate, sep, item = spec.partition(':')
    if not crate or (sep and item not in ALL_ITEMS):
        raise ValueError(f'invalid require spec {spec!r}: use <crate> or <crate>:<{"|".join(ALL_ITEMS)}>')
    return crate, item if sep else None


def require(d, specs):
    # A bare crate needs every item of the record's schema; <crate>:<item> needs
    # that item only. An item outside the schema is a usage error (ValueError).
    rows = comparisons(d)
    wanted = []
    for spec in specs:
        crate, item = parse_spec(spec)
        if item is not None and item not in schema_items(d):
            raise ValueError(f'item {item!r} is not in schema {d["schema"]}: {", ".join(schema_items(d))}')
        wanted += [(crate, i) for i in ((item,) if item else schema_items(d))]
    return all(any(r['crate'] == c and r['item'] == i and not r['excluded'] for r in rows) for c, i in wanted)


def cell(value):
    return '-' if value is None else str(value).replace('|', '\\|')


def summary(docs):
    # edit_verify also shows the nextest part (edit_to_verify_secs, without the
    # doc test) when the record has it; other stages have no such part.
    lines = [BEGIN, '| crate | stage | attempt/pair | base s | HEAD s | base nextest s | HEAD nextest s | load ratio | result | reason |',
             '|---|---|---|---:|---:|---:|---:|---:|---|---|']
    for d in docs:
        for r in comparisons(d):
            key = 'edit_to_verify_with_doc_secs' if r['item'] == 'edit_verify' else 'secs'
            part = [r[s].get('edit_to_verify_secs') if r['item'] == 'edit_verify' else None for s in ('base', 'head')]
            ratio = '-' if r['ratio'] is None else format(r['ratio'], '.6g')
            lines.append(f"| {r['crate']} | {r['item']} | {r['attempt']}/{r['pair']} | {cell(r['base'].get(key))} | {cell(r['head'].get(key))} | {cell(part[0])} | {cell(part[1])} | {ratio} | {'excluded' if r['excluded'] else 'included'} | {cell(r['reason'])} |")
    return '\n'.join(lines + [END]) + '\n'


def wait_load():
    start = time.monotonic()
    limit = float(os.environ.get('MEASURE_MAX_LOAD', '24'))
    duration = float(os.environ.get('MEASURE_LOAD_WAIT', '600'))
    while True:
        value = Path('/proc/loadavg').read_text().strip()
        elapsed = time.monotonic() - start
        ready = float(value.split()[0]) <= limit
        if ready or elapsed >= duration:
            return dict(waited_secs=round(elapsed, 3), loadavg=value, ready=ready, max_load=limit)
        time.sleep(min(5, duration - elapsed))


def write(path, d):
    d['comparisons'] = comparisons(d)
    Path(path).write_text(json.dumps(d, indent=2) + '\n')


def parse_run(txt):
    outcomes = {}
    for status, name in re.findall(r'^\s*(PASS|FAIL|SKIP|TIMEOUT|SIGSEGV|SIGABRT|SIGKILL|ABORT|LEAK)\s+\[[^\]]*\]\s+(?:\(\s*\d+/\d+\)\s+)?(\S+\s+\S+)', txt, re.M):
        outcomes[name] = status
    m = re.search(r'(\d+) tests? run:', txt)
    skipped = re.search(r'(\d+) skipped', txt)
    return dict(tests_run=int(m[1]) if m else None, skipped_tests=int(skipped[1]) if skipped else 0,
                failed_tests=sorted(k for k, v in outcomes.items() if v not in ('PASS', 'SKIP')),
                outcomes=outcomes)


def flaky(results):
    # Bundles wrap the former target in a module. Match that exact suffix to
    # the base identity, only when unique; keep original outcomes in the raw data.
    maps = [dict(r.get('outcomes', {})) for r in results]
    base_names = set(maps[0]) | set(maps[3])
    for i in (1, 2):
        for name in list(maps[i]):
            if name in base_names or ' ' not in name:
                continue
            path = name.split(' ', 1)[1]
            candidates = [n for n in base_names if ' ' in n and
                          path.endswith(n.split(' ', 1)[0].split('::')[-1] + '::' + n.split(' ', 1)[1])]
            if len(candidates) == 1 and candidates[0] not in maps[i]:
                maps[i][candidates[0]] = maps[i].pop(name)
    names = set().union(*maps)
    return sorted(n for n in names if any(
        n in maps[i] and n in maps[j] and maps[i][n] != maps[j][n]
        for i, j in ((0, 3), (1, 2), (0, 1), (2, 3))))


def execute(out, crates, run=False):
    if not all(crates) or any(':' in c for c in crates):
        raise ValueError('compare and run-compare take crate names only (no <crate>:<item>)')
    raw = Path(str(Path(out).with_suffix('')) + '-raw')
    raw.mkdir(parents=True, exist_ok=True)
    d = dict(schema='health-v2-r5-' + ('run' if run else 'build') + '-compare/2', attempts=[],
             git_head=subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
             merge_base_main=subprocess.check_output(['git', 'merge-base', 'HEAD', 'main'], text=True).strip(),
             host=os.uname().nodename, cargo_target_dir=os.environ.get('CARGO_TARGET_DIR'),
             started_at=time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),
             order=['base', 'head', 'head', 'base'] if run else ['base', 'head'])
    for attempt in range(int(os.environ.get('MEASURE_RETRIES', '2')) + 1):
        pending = [c for c in crates if not require(d, [c])]
        if not pending:
            break
        a = dict(attempt=attempt + 1, crates=pending, pairs={})
        d['attempts'].append(a)
        if not run:
            target = raw / f'attempt-{attempt + 1}.json'
            subprocess.run(['bash', str(HERE / 'measure.sh'), '_compare-once', str(target), ','.join(pending)], check=True)
            old = json.loads(target.read_text())
            a['raw'] = old
            for c in pending:
                a['pairs'][c] = {item: [[old['sides']['base']['crates'].get(c, {}).get(item, {}),
                                        old['sides']['head']['crates'].get(c, {}).get(item, {})]]
                                 for item in ('build', 'edit_verify')}
            write(out, d)
        else:
            base = d['merge_base_main']
            with tempfile.TemporaryDirectory(prefix='r5-run-', dir=os.environ['TMPDIR']) as tmp:
                tree = Path(tmp) / 'base'
                subprocess.run(['git', 'worktree', 'add', '--detach', str(tree), base], check=True)
                try:
                    for c in pending:
                        results = []
                        for index, side in enumerate(('base', 'head', 'head', 'base')):
                            cwd = tree if side == 'base' else Path.cwd()
                            prefix = raw / f'{attempt}-{c}-{index}'
                            # Shared target: settle this side immediately before each run.
                            build = ['cargo', 'nextest', 'run', '-p', c, '--no-run']
                            with Path(str(prefix) + '.build.log').open('w') as log:
                                br = subprocess.run(build, cwd=cwd, stdout=log, stderr=log).returncode
                                if br == 0 and c == 'celeris':
                                    br = subprocess.run(['cargo', 'build', '-p', 'celeris-campaign', '--bins'], cwd=cwd, stdout=log, stderr=log).returncode
                            w = wait_load()
                            r = dict(load_wait=w, loadavg_before=w['loadavg'], exit=br, secs=0)
                            if br == 0 and w['ready']:
                                t0 = time.monotonic()
                                with Path(str(prefix) + '.log').open('w') as log:
                                    rc = subprocess.run(['cargo', 'nextest', 'run', '-p', c, '--no-fail-fast', '--status-level', 'all', '--final-status-level', 'all'], cwd=cwd, stdout=log, stderr=log).returncode
                                r.update(parse_run(Path(str(prefix) + '.log').read_text()))
                                r.update(exit=rc if r['tests_run'] is not None else 126, secs=round(time.monotonic() - t0, 3))
                            else:
                                r['exit'] = br or 125
                            r['loadavg_after'] = Path('/proc/loadavg').read_text().strip()
                            results.append(r)
                            a['results'] = {**a.get('results', {}), c: results}
                            write(out, d)
                        a['pairs'][c] = {'run': [[results[0], results[1]], [results[3], results[2]]]}
                        a.setdefault('flaky', {})[c] = flaky(results)
                        write(out, d)
                finally:
                    subprocess.run(['git', 'worktree', 'remove', '--force', str(tree)], check=True)
    write(out, d)
    return 0 if require(d, crates) else 1


def main():
    if len(sys.argv) < 2:
        return 2
    mode, *args = sys.argv[1:]
    arity = {'wait': 0, 'compare': 2, 'run-compare': 2, 'validate': 1, 'require': 2}
    if mode in arity and len(args) != arity[mode]:
        return 2
    if mode == 'wait':
        print(json.dumps(wait_load())); return 0
    if mode in ('compare', 'run-compare'):
        return execute(args[0], args[1].split(','), mode == 'run-compare')
    if mode == 'summary':
        check = args[0] == '--check'
        md = args[1] if check else None
        paths = args[2:] if check else args
        docs = [json.loads(Path(p).read_text()) for p in paths]
        if not docs or any(not comparisons(d) for d in docs):
            raise ValueError('summary requires /2 comparison records')
        block = summary(docs)
        if check:
            data = Path(md).read_bytes()
            return 0 if data.count(BEGIN.encode()) == 1 and data.count(END.encode()) == 1 and block.encode() in data else 1
        print(block, end=''); return 0
    if mode == 'require':
        specs = args[1].split(',')
        for spec in specs:
            parse_spec(spec)  # malformed spec or unknown item: exit 2 before reading the record
    d = json.loads(Path(args[0]).read_text())
    if mode == 'validate':
        if d.get('schema') not in ('health-v2-r5-build-compare/2', 'health-v2-r5-run-compare/2') or not comparisons(d):
            return 1
        return 0 if d.get('comparisons') == comparisons(d) else 1
    if mode == 'require':
        valid = d.get('schema') in ('health-v2-r5-build-compare/2', 'health-v2-r5-run-compare/2') and d.get('comparisons') == comparisons(d)
        return 0 if valid and require(d, specs) else 1
    return 2


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (ValueError, KeyError, IndexError, OSError, TypeError, subprocess.CalledProcessError) as e:
        print(str(e), file=sys.stderr)
        sys.exit(2)
