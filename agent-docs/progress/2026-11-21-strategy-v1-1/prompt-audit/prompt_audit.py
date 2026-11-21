#!/usr/bin/env python3
"""strategy-v1 子 1 prompt-audit: 本番の run の実 prompt に目的（祖先を含む）と受入条件の全文が入ったかを数える。

数え方は H の prompt-audit（agent-docs/progress/2026-11-19-arch-v4-h/prompt-audit.md）と同じ:
両側の空白の連なりを 1 つの空白にし、objective（と祖先の objective・Project.request）は末尾 200 字が
prompt に含まれるか、受入条件（WU の run は done_when）は各 text が含まれるかを見る。
prompt.txt が無い run は missing。

  collect --db <db path> --out <dir>   本番 DB を mode=ro で開き、prompt-audit.tsv と meta.json を <dir> に書く
  render [--write]                     この dir の tsv と meta.json から md の集計の節を組む（--write で md を書き換える）
  check                                commit した tsv・meta.json と md の集計の節が一致するかを確かめる（欠けでは落ちない）

本番の DB と file には書かない（DB は file:<path>?mode=ro の URI でだけ開き、prompt.txt は読むだけ）。
tsv には id・role・数・判定だけを書き、text（秘密・人の名前を含みうる）は書かない。
"""
import argparse
import datetime as dt
import json
import os
import re
import sqlite3
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
MD = os.path.join(os.path.dirname(HERE), 'prompt-audit.md')
TSV = 'prompt-audit.tsv'
META = 'meta.json'
BEGIN = '<!-- prompt-audit:summary:begin (prompt_audit.py render が書く。手で直さない) -->'
END = '<!-- prompt-audit:summary:end -->'

ROLES = ('worker', 'planner', 'reviewer')
KINDS = ('objective', 'acceptance', 'task_objective', 'task_acceptance',
         'ancestor_objective', 'ancestor_acceptance', 'project_request')
RESULTS = ('found', 'cut', 'missing', 'none')
COLS = ['run_id', 'task_id', 'role', 'wu_key', 'run_status', 'started_at', 'prompt',
        'kind', 'ref_task', 'up', 'chars', 'total', 'found', 'cut', 'missing', 'head_only', 'result']

# 修正（ADR 2026-11-19-planner-reviewer-prompt-coverage）の commit は件名で引く（rebase で hash が変わるため）。
FIX_SUBJECT = 'arch-v4 H R1 planner-prompt: planner の 4 builder の prompt に task の受入条件の節を出し'
FIX_HEADING = '## Task acceptance criteria (checked by the final review)'
PROMPT_RS = 'crates/task-worker/src/claude_code/prompt.rs'
TAIL = 200
HEAD = 80


def norm(s):
    return re.sub(r'\s+', ' ', s or '').strip()


def texts(xs):
    return [x.get('text', '') if isinstance(x, dict) else str(x) for x in (xs or [])]


def ts(s):
    """'2026-11-20T15:49:51.976248743Z'・'…+00:00'・'…Z' を aware な datetime にする（小数は 6 桁で切る）。"""
    m = re.match(r'(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)(?:\.(\d+))?(Z|[+-]\d\d:\d\d)?$', s or '')
    if not m:
        raise ValueError(f'unparsable timestamp: {s!r}')
    frac = (m.group(2) or '0')[:6].ljust(6, '0')
    tz = m.group(3) or 'Z'
    tz = '+00:00' if tz == 'Z' else tz
    return dt.datetime.fromisoformat(f'{m.group(1)}.{frac}{tz}')


def git(repo, *args):
    return subprocess.run(['git', '-C', repo, *args], capture_output=True, text=True)


def fix_release(repo, rel_dir):
    """修正の commit を件名で引き、それを祖先に持つ release のうち昇格が最も早いものを返す。"""
    p = git(repo, 'log', '-1', '--format=%H', '-F', f'--grep={FIX_SUBJECT}', 'HEAD')
    fix = p.stdout.strip()
    if p.returncode != 0 or not fix:
        sys.exit(f'fix commit not found by subject: {FIX_SUBJECT}')
    rels = []
    for name in sorted(os.listdir(rel_dir)):
        pj = os.path.join(rel_dir, name, 'promoted.json')
        if not os.path.isfile(pj):
            continue
        with open(pj, encoding='utf-8') as f:
            j = json.load(f)
        if j.get('operation') != 'promote' or j.get('status') != 'succeeded':
            continue
        anc = git(repo, 'merge-base', '--is-ancestor', fix, name).returncode == 0
        src = git(repo, 'show', f'{name}:{PROMPT_RS}').stdout
        rels.append({'sha12': name, 'promoted_at': j['promoted_at'], 'from': j.get('from'),
                     'contains_fix': anc, 'has_heading': FIX_HEADING in src})
    rels.sort(key=lambda r: ts(r['promoted_at']))
    first = next((r for r in rels if r['contains_fix']), None)
    if first is None:
        sys.exit('no promoted release contains the fix')
    prev = next((r for r in rels if r['sha12'] == first['from']), None)
    return fix, first, prev, rels


def ws_path(ws_root, tid, j):
    p = (j.get('workspace') or {}).get('path') or tid
    return p if os.path.isabs(p) else os.path.join(ws_root, p)


def collect(a):
    repo = a.repo or git(os.getcwd(), 'rev-parse', '--show-toplevel').stdout.strip()
    fix, first, prev, rels = fix_release(repo, a.releases_dir)
    cutoff = ts(first['promoted_at'])
    read_at = dt.datetime.now(dt.timezone.utc).replace(microsecond=0)
    c = sqlite3.connect(f'file:{a.db}?mode=ro', uri=True)   # mode=ro: 本番 DB に書かない

    tasks = {}

    def task(tid):
        if tid not in tasks:
            r = c.execute('select objective, json, parent_id, project_id from tasks where id=?', (tid,)).fetchone()
            j = json.loads(r[1]) if r else {}
            tasks[tid] = {'objective': (r[0] if r else '') or '', 'acc': texts(j.get('acceptance')),
                          'ws': ws_path(a.workspaces, tid, j), 'parent': r[2] if r else None,
                          'project': r[3] if r else None}
        return tasks[tid]

    def ancestors(tid):
        out, cur, seen = [], task(tid)['parent'], {tid}
        while cur and cur not in seen and len(out) < 16:
            seen.add(cur)
            out.append(cur)
            cur = task(cur)['parent']
        return out   # 親が先頭、root が末尾

    def wu_spec(tid, wid, started):
        """WU の run: run の開始時に有効だった計画の版（created_at <= started の最大の版）の unit。"""
        r = c.execute('select key, json from work_units where id=?', (wid,)).fetchone()
        key, cur = r[0], json.loads(r[1])
        plans = c.execute('select version, created_at, json from execution_plans where task_id=? '
                          'order by version desc', (tid,)).fetchall()
        for ver, ca, js in plans:
            if ts(ca) <= started:
                j = json.loads(js)
                for u in (j.get('units') or []) + (j.get('work_units') or []):
                    if u.get('key') == key:
                        return key, u.get('objective') or '', texts(u.get('done_when'))
        return key, cur.get('objective') or '', texts(cur.get('done_when'))

    runs = []
    for run_id, tid, wid, role, status, started in c.execute(
            "select run_id, task_id, work_unit_id, role, status, started_at from runs "
            "where role in ('worker','planner','reviewer')"):
        st = ts(started)
        if st > cutoff:
            runs.append((st, run_id, tid, wid, role, status, started))
    runs.sort()
    selected, dropped = runs[:a.limit], runs[a.limit:]

    rows = []
    for st, run_id, tid, wid, role, status, started in selected:
        t = task(tid)
        path = os.path.join(t['ws'], 'runs', run_id, 'prompt.txt')
        ptxt = None
        if os.path.isfile(path):
            with open(path, encoding='utf-8', errors='replace') as f:
                ptxt = norm(f.read())
        targets = []   # (kind, ref_task, up, mode, payload)
        if wid:
            key, obj, acc = wu_spec(tid, wid, st)
            targets += [('objective', tid, 0, 'tail', obj), ('acceptance', tid, 0, 'items', acc),
                        ('task_objective', tid, 0, 'tail', t['objective']),
                        ('task_acceptance', tid, 0, 'items', t['acc'])]
        else:
            key = '-'
            targets += [('objective', tid, 0, 'tail', t['objective']), ('acceptance', tid, 0, 'items', t['acc'])]
        for up, aid in enumerate(ancestors(tid), start=1):
            targets += [('ancestor_objective', aid, up, 'tail', task(aid)['objective']),
                        ('ancestor_acceptance', aid, up, 'items', task(aid)['acc'])]
        if t['project']:
            r = c.execute('select request from projects where id=?', (t['project'],)).fetchone()
            targets.append(('project_request', t['project'], '-', 'tail', (r[0] if r else '') or ''))
        for kind, ref, up, mode, payload in targets:
            if mode == 'tail':
                items = [norm(payload)[-TAIL:]] if norm(payload) else []
                heads = [norm(payload)[:HEAD]] if norm(payload) else []
                chars = len(payload or '')
            else:
                items = [norm(x) for x in payload if norm(x)]
                heads = [x[:HEAD] for x in items]
                chars = sum(len(x) for x in payload)
            total = len(items)
            if ptxt is None:
                found, cut, missing, head_only = 0, 0, total, 0
            else:
                hit = [x in ptxt for x in items]
                found = sum(hit)
                cut = total - found
                head_only = sum(1 for h, ok in zip(heads, hit) if not ok and h in ptxt)
                missing = 0
            result = ('none' if total == 0 else 'missing' if ptxt is None
                      else 'found' if cut == 0 else 'cut')
            rows.append([run_id, tid, role, key, status, started, 'present' if ptxt is not None else 'missing',
                         kind, ref, str(up), str(chars), str(total), str(found), str(cut), str(missing),
                         str(head_only), result])

    os.makedirs(a.out, exist_ok=True)
    with open(os.path.join(a.out, TSV), 'w', encoding='utf-8') as f:
        f.write('\n'.join('\t'.join(r) for r in [COLS] + rows) + '\n')
    meta = {
        'read_at': read_at.isoformat().replace('+00:00', 'Z'),
        'db': 'file:<db path>?mode=ro',
        'fix_subject': FIX_SUBJECT,
        'fix_is_ancestor_of_head': git(repo, 'merge-base', '--is-ancestor', fix, 'HEAD').returncode == 0,
        'first_release': first, 'first_release_from': prev,
        'releases': rels,
        'cutoff': first['promoted_at'],
        'selection': "runs.role in (worker, planner, reviewer) and started_at > cutoff, order by started_at, run_id",
        'limit': a.limit, 'eligible': len(runs), 'selected': len(selected), 'dropped': len(dropped),
    }
    with open(os.path.join(a.out, META), 'w', encoding='utf-8') as f:
        json.dump(meta, f, ensure_ascii=False, indent=1)
        f.write('\n')
    print(f'{len(selected)} runs ({len(rows)} rows) after {first["promoted_at"]} ({first["sha12"]}); '
          f'eligible {len(runs)}, limit {a.limit}')


def load(d=HERE):
    with open(os.path.join(d, TSV), encoding='utf-8') as f:
        lines = f.read().splitlines()
    with open(os.path.join(d, META), encoding='utf-8') as f:
        meta = json.load(f)
    return [ln.split('\t') for ln in lines], meta


def render(rows, meta):
    body = [dict(zip(COLS, r)) for r in rows[1:]]
    runs = {}
    for r in body:
        runs.setdefault(r['run_id'], r)
    out = [BEGIN, '']
    out.append(f"- DB を読んだ時刻: {meta['read_at']}。基点: release `{meta['first_release']['sha12']}` の昇格 "
               f"{meta['cutoff']}（その前の release `{(meta['first_release_from'] or {}).get('sha12', '-')}` は修正を含まない）。")
    out.append(f"- 対象の run: {meta['selected']} 件（条件に合う run {meta['eligible']} 件、上限 {meta['limit']}、"
               f"上限で落とした run {meta['dropped']} 件）。tsv の行: {len(body)}。")
    by_role = {}
    for r in runs.values():
        by_role.setdefault(r['role'], []).append(r)
    out.append('- role ごとの run の数（prompt のある run / ない run）: ' + '、'.join(
        f"{k} {len(v)}（{sum(x['prompt'] == 'present' for x in v)} / {sum(x['prompt'] == 'missing' for x in v)}）"
        for k, v in sorted(by_role.items())) + '。')
    out += ['', '| role | kind | 行 | 項目 | found | cut | missing | 先頭だけ | 判定 found / cut / missing / none |',
            '|---|---|---:|---:|---:|---:|---:|---:|---|']
    agg = {}
    for r in body:
        g = agg.setdefault((r['role'], r['kind']), [0, 0, 0, 0, 0, 0, {k: 0 for k in RESULTS}])
        g[0] += 1
        for i, col in enumerate(('total', 'found', 'cut', 'missing', 'head_only'), start=1):
            g[i] += int(r[col])
        g[6][r['result']] += 1
    for (role, kind) in sorted(agg, key=lambda x: (ROLES.index(x[0]), KINDS.index(x[1]))):
        g = agg[(role, kind)]
        out.append(f'| {role} | {kind} | {g[0]} | {g[1]} | {g[2]} | {g[3]} | {g[4]} | {g[5]} | '
                   + ' / '.join(str(g[6][k]) for k in RESULTS) + ' |')
    out += ['', '### run ごとの判定', '',
            '| run | task | role | WU | 状態 | prompt | objective | acceptance | task_objective | task_acceptance | 祖先 objective | 祖先 acceptance | project_request |',
            '|---|---|---|---|---|---|---|---|---|---|---|---|---|']
    for rid, r0 in runs.items():
        rr = [x for x in body if x['run_id'] == rid]

        def cell(kind):
            xs = [x for x in rr if x['kind'] == kind]
            if not xs:
                return '-'
            return ' '.join(f"{x['found']}/{x['total']}" + (f"(up{x['up']})" if x['up'] not in ('0', '-') else '')
                            for x in xs)
        out.append(f"| {rid} | {r0['task_id']} | {r0['role']} | {r0['wu_key']} | {r0['run_status']} | {r0['prompt']} | "
                   + ' | '.join(cell(k) for k in KINDS) + ' |')
    out += ['', '### planner の受入条件', '']
    pl = [x for x in body if x['role'] == 'planner' and x['kind'] == 'acceptance']
    pt = sum(int(x['total']) for x in pl if x['prompt'] == 'present')
    pf = sum(int(x['found']) for x in pl if x['prompt'] == 'present')
    out.append(f"- planner の run {len(pl)} 件（{', '.join(x['run_id'] for x in pl) or '無し'}）。"
               f"prompt のある run の受入条件: {pf}/{pt}。")
    out += ['', END]
    return '\n'.join(out) + '\n'


def md_block(text):
    i, j = text.find(BEGIN), text.find(END)
    if i < 0 or j < 0 or j < i:
        return None
    return text[i:j + len(END)] + '\n'


def validate(rows, meta):
    errs = []
    if not rows or rows[0] != COLS:
        return ['tsv header differs']
    for n, r in enumerate(rows[1:], start=2):
        if len(r) != len(COLS):
            errs.append(f'line {n}: {len(r)} columns')
            continue
        d = dict(zip(COLS, r))
        if d['role'] not in ROLES or d['kind'] not in KINDS or d['result'] not in RESULTS:
            errs.append(f'line {n}: role/kind/result out of domain')
        if d['prompt'] not in ('present', 'missing'):
            errs.append(f'line {n}: prompt out of domain')
        try:
            t, f, c, m, h = (int(d[k]) for k in ('total', 'found', 'cut', 'missing', 'head_only'))
        except ValueError:
            errs.append(f'line {n}: non-integer count')
            continue
        if f + c + m != t or h > c:
            errs.append(f'line {n}: found+cut+missing != total or head_only > cut')
        want = 'none' if t == 0 else 'missing' if d['prompt'] == 'missing' else 'found' if c == 0 else 'cut'
        if d['result'] != want:
            errs.append(f'line {n}: result {d["result"]} != {want}')
    ids = {r[0] for r in rows[1:]}
    if len(ids) != meta.get('selected'):
        errs.append(f'run ids in tsv {len(ids)} != meta.selected {meta.get("selected")}')
    for role in ROLES:
        if not any(r[2] == role for r in rows[1:]):
            errs.append(f'no run with role {role}')
    for kind in ('objective', 'acceptance', 'ancestor_objective'):
        if not any(r[7] == kind for r in rows[1:]):
            errs.append(f'no row with kind {kind}')
    return errs


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest='cmd', required=True)
    co = sub.add_parser('collect')
    co.add_argument('--db', required=True)
    co.add_argument('--out', required=True)
    co.add_argument('--limit', type=int, default=300)
    co.add_argument('--repo')
    co.add_argument('--releases-dir', default='/local/celeris/state/releases')
    co.add_argument('--workspaces', default='/local/celeris/data/workspaces')
    re_ = sub.add_parser('render')
    re_.add_argument('--write', action='store_true')
    sub.add_parser('check')
    a = ap.parse_args()
    if a.cmd == 'collect':
        collect(a)
        return 0
    rows, meta = load()
    block = render(rows, meta)
    if a.cmd == 'render':
        if not a.write:
            sys.stdout.write(block)
            return 0
        with open(MD, encoding='utf-8') as f:
            text = f.read()
        old = md_block(text)
        if old is None:
            sys.exit(f'markers not found in {MD}')
        with open(MD, 'w', encoding='utf-8') as f:
            f.write(text.replace(old, block))
        return 0
    errs = validate(rows, meta)
    with open(MD, encoding='utf-8') as f:
        got = md_block(f.read())
    if got != block:
        errs.append('md summary block differs from the one rendered from the tsv (run: prompt_audit.py render --write)')
    for e in errs:
        print(f'FAIL: {e}')
    if errs:
        return 1
    body = rows[1:]
    gaps = sum(1 for r in body if r[16] in ('cut', 'missing'))
    print(f'ok: {len({r[0] for r in body})} runs, {len(body)} rows, {gaps} rows with gaps (cut/missing); md matches tsv')
    return 0


if __name__ == '__main__':
    sys.exit(main())
