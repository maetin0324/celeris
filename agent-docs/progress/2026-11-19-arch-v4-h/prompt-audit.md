---
title: アーキ v4 H prompt-audit — アーキv4 の木の worker・planner・reviewer の実 prompt に objective の全文と受入条件が入ったか（本番 DB は mode=ro で読むだけ）
tasks: [01M7MZTKBPZPQE4XN451F8X84F]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# prompt-audit（WorkUnit）

## 結論

- worker と reviewer: prompt が残る run のすべてで、objective の末尾 200 字と受入条件（WU の run は done_when）が全部入っていた。切れた（cut）run は 0 件。
- 未確認が 4 件ある: worker の 4 run は prompt.txt が無い（missing）。4 run とも開始から約 2.7 秒で lease 喪失により中止され、run の dir が作られていない。
- planner: objective の末尾は 24/24 run で入った。受入条件は 0/126 件で、1 件も入っていない。planner の prompt に task の受入条件を載せる節が無いことが原因（下の「planner の受入条件」）。H の task の受入条件 1 は Worker・Reviewer の prompt を対象にしているので、planner の欠けはそれとは別の観察として書く。直すかどうかは人が決める（提案 1）。
- 本番 DB は `sqlite3.connect('file:/local/celeris/data/db/celeris.sqlite3?mode=ro', uri=True)` で開いて読んだだけ。本番の DB と file には書いていない（このノートの「再現」）。

## 数えた範囲と規則

- 対象の task: 親 01M7MZT8KX547464YYS76QNP4G の子孫の 15 task（A〜H の 8 件と子 task の 7 件。H の子 safety 01M7STHNG3FB9Z88KKF1TYG49S を含む）。親自身の run は 0 件。
- 対象の run: role が worker・planner・reviewer の run（WorkUnit の run を含む）。計 201 run（worker 157・planner 24・reviewer 20）。対象の task に他の role の run は無い。DB の読み取りは 2026-11-19T11:58Z。対象の run の最新の開始は 11:55Z。走っている run（この run を含む H の run）も数えた。
- prompt の場所: `<task の workspace>/runs/<run id>/prompt.txt`（workspace は tasks.json の `workspace.path`。`/local/celeris/data/workspaces/` からの相対）。worker が `crates/task-worker/src/subprocess.rs` の `write_run_prompt` でこの file を書く（ADR-0023 M1）。WU の run も同じ task の `runs/` に置かれる。
- 比べる相手:
  - WU の無い worker・planner: その task の objective と受入条件（tasks.json の `acceptance[].text`）。
  - reviewer: review する task（run の task_id）の objective と受入条件。
  - WorkUnit の run: WU の objective と done_when。値は、run の開始時に有効だった計画の版（`execution_plans.created_at <= runs.started_at` の最大の版）の `units[]` から取る。理由は下の「今の work_units 行と比べた場合」。
- 判定: 比べる前に両側の空白の連なりを 1 つの空白にする（空白だけを正規化）。`objective_tail` は、正規化した objective の末尾 200 字が正規化した prompt に含まれれば found、含まれなければ cut、prompt.txt が無ければ missing。`acceptance_found` は、正規化した受入条件の text のうち prompt に含まれるものの数（missing の run は 0）。
- 結果の行: [prompt-audit.tsv](prompt-audit.tsv)（201 行と header）。

## kind ごとの集計

| kind | run | found | cut | missing | 受入条件（prompt のある run） | 受入条件の欠け | prompt の無い run の受入条件 |
|---|---:|---:|---:|---:|---:|---:|---:|
| worker | 157 | 153 | 0 | 4 | 651/651 | 0 | 18（未確認） |
| reviewer | 20 | 20 | 0 | 0 | 112/112 | 0 | 0 |
| planner | 24 | 24 | 0 | 0 | 0/126 | 126 | 0 |
| 計 | 201 | 197 | 0 | 4 | 763/889 | 126 | 18 |

- objective が 1,500 字を超えて found の run: worker 116・reviewer 19・planner 23。最長は A の 7,247 字で、A の worker 3 run と reviewer 2 run の全部で末尾が入っていた。以前の「親 task の objective が先頭 1,500 字で切れる」不具合は、この木の run では再現していない。
- 参考（tsv には載せない）: prompt のある WU の run 150 件で、「このタスク全体の目的（全文）」の節の親 task の objective（1,168〜7,247 字）の末尾 200 字は 150/150 で見つかった。「最終レビューで確かめる Task の受け入れ条件（参考）」の節の task の受入条件は 849/849 件で見つかった。

## 切れた例

この audit の規則（WU は run の開始時の計画の版と比べる）では、cut は 0 件。

### 今の work_units 行と比べた場合（cut に見える 7 件。原因は WU の spec の書き換え）

WU の objective を今の `work_units.json` の値と比べると、次の 7 run が cut になる。7 run とも、prompt には計画 v1 の unit の objective（B checker 1,632 字・C risk-rule 1,256 字）と done_when が全部入っていた。計画の改訂（B は v2、C は v3）が同じ WU の行（同じ id）の objective・done_when を続きの run 用の文に書き換えたため、今の行は run が受け取った文と違う。prompt が切れたのではない。

| task | run | WU | run の開始（UTC） | 計画の版（開始時 → 今） | 今の行の objective のうち prompt に無い文の先頭 80 字 |
|---|---|---|---|---|---|
| 01M7MZTKAZQY72EX2R4RQ1WVWF（B） | 01M7NEJTXXMAYJ8ZAK15AWFRVH | checker | 2026-11-17T19:08:48 | v1 → v2（行の更新 20:35:59） | `この WorkUnit の実装（scripts/dev/repo-health/health/lang_deps.py・rules/deps.py・rules/` |
| 01M7MZTKAZQY72EX2R4RQ1WVWF（B） | 01M7NFZM9CZVEBYDD5SH0T8DQH | checker | 2026-11-17T19:33:16 | v1 → v2 | 同上 |
| 01M7MZTKAZQY72EX2R4RQ1WVWF（B） | 01M7NJ8WEHJH44YXWTZ2FZKTP6 | checker | 2026-11-17T20:13:16 | v1 → v2 | 同上 |
| 01M7MZTKAZQY72EX2R4RQ1WVWF（B） | 01M7NJS5EFYPYTA18TZR8WWXTA | checker | 2026-11-17T20:22:10 | v1 → v2 | 同上 |
| 01M7MZTKB29VF5TX7FCQ15XEMW（C） | 01M7NSR7W417YF02YD45B4SB7J | risk-rule | 2026-11-17T22:24:02 | v1 → v3（行の更新 2026-11-18T03:54:17） | ``の続きの run。owner は `crates/task-core/src/design_review.rs` の `assess_risk`（純粋関数）。D`` |
| 01M7MZTKB29VF5TX7FCQ15XEMW（C） | 01M7NW71ACNKFG51NQGJNHNFFX | risk-rule | 2026-11-17T23:07:01 | v1 → v3 | 同上 |
| 01M7MZTKB29VF5TX7FCQ15XEMW（C） | 01M7P8MW0SECD83HRSEPWFPWQ4 | risk-rule | 2026-11-18T02:44:18 | v1 → v3 | 同上 |

今の行と比べると、done_when の欠けは checker の 4 run で各 4 件、risk-rule の 3 run で各 3 件（計 25 件）に見える。開始時の版と比べると欠けは 0 件。
過去の run を監査するときは `work_units` 行ではなく `execution_plans.json` の `units[]` を run の開始時刻で引く必要がある（提案 2）。

### prompt の無い run（missing 4 件）

| task | run | WU | 開始（UTC） | 終了までの秒 | 受入条件（done_when）の数 | 記録された終わり方 |
|---|---|---|---|---:|---:|---|
| 01M7MZTKB29VF5TX7FCQ15XEMW（C） | 01M7PBH4HWDFMXSEMC5AKVKJVK | risk-rule | 2026-11-18T03:34:41 | 2.7 | 5 | `interrupted: aborted (task no longer running under this lease)`、usage null |
| 01M7MZTKB29VF5TX7FCQ15XEMW（C） | 01M7PBPTFQQG9N8WDMRW8M5435 | risk-rule | 2026-11-18T03:37:48 | 2.7 | 5 | 同上 |
| 01M7MZTKB29VF5TX7FCQ15XEMW（C） | 01M7PBWQKNTNHKM4E3S00DJ1AV | risk-rule | 2026-11-18T03:41:01 | 2.7 | 5 | 同上 |
| 01M7MZTKB4RYM80S6CT3RST2N9（D） | 01M7QC2E69H17AXEYD9AS8TK10 | adr-forms | 2026-11-18T13:03:22 | 2.5 | 3 | 同上 |

4 run とも `runs/<run id>/` の dir が無い（request.json・stdout.jsonl も無い）。原因の候補: worker が prompt を書く前に lease を失い、dispatcher が run を中止した（C の risk-rule の 3 run は計画 v2 の作成 03:31:39 と v3 の作成 03:54:15 の間、D の adr-forms は計画 v2 の作成 12:54:34 と v3 の作成 13:10:02 の間）。LLM は呼ばれていない（usage null）ので、作業に使われた prompt は無い。これらの run に何が渡るはずだったかは未確認。

## planner の受入条件（0/126）

planner の 24 run で、task の受入条件の text は 1 件も prompt に無い。prompt の見出しにも受入条件の節が無い（B の planner run 01M7ND92G1Q4CVS3YEYKQ9B6SF の見出しを確かめた）。

- 原因: `crates/task-worker/src/claude_code/prompt.rs:745` の `build_plan_prompt` は `prompt_header`（objective の全文を含む）に計画の指示を足すだけで、`task.acceptance` を出さない。`task.acceptance` を出すのは worker 用の `build_execute_prompt`（同 :649〜:675。WU の run では「最終レビューで確かめる Task の受け入れ条件（参考）」の節）と review の prompt（同 :1685）だけ。
- 影響: planner は task の受入条件の text を見ずに WU の done_when を書く。A〜H の objective は依頼の「共通受入条件」の文を含むが、受入条件の text にだけある条件は planner の prompt に入らない。
- この WU では直さない（objective の指示）。

## 提案（起票は人が決める）

1. planner の prompt に task の受入条件の節を足すか決める。owner は `crates/task-worker/src/claude_code/prompt.rs` の `build_plan_prompt`。足すなら、守る不変条件は「planner の prompt に task の `acceptance[].text` が全部入る」。今の planner の試験（`crates/task-worker/src/claude_code/tests.rs` の `planner_prompt_*`）は受入条件の有無を見ていないので、この欠けを検出できない。
2. 過去の WU の run の監査は、`work_units` 行ではなく、run の開始時に有効だった `execution_plans.json` の `units[]` を使う。今の行は計画の改訂で同じ id のまま書き換わる（この木で 7 run が該当）。run ごとの spec を残すかどうか（例: request.json の work_unit の写しを正本にする）は人が決める。
3. 回帰条件の候補: この audit を H と同じ規則で再実行し、worker・reviewer の `objective_tail` に cut が 0 件、prompt のある run の受入条件の欠けが 0 件であること。

## 再現

作業場所の外（`$TMPDIR`）に次の script を `audit.py` として置き、repo の root で `PA_DETAIL=$TMPDIR/detail.json python3 -I "$TMPDIR/audit.py" > agent-docs/progress/2026-11-19-arch-v4-h/prompt-audit.tsv` を実行した（exit 0、201 行と header）。DB は `mode=ro` の URI でだけ開く。`PA_DETAIL` は md の参考の数（親 task の objective・今の WU 行との比較）を出す JSON の出力先で、本番の外の一時 file。

```python
# H prompt-audit: arch-v4 の木の run の実 prompt に objective の末尾 200 字と受入条件が入ったかを数える（本番 DB は読むだけ）
import sqlite3, json, os, re, sys
DB = 'file:/local/celeris/data/db/celeris.sqlite3?mode=ro'      # mode=ro: 本番 DB に書かない
WS = '/local/celeris/data/workspaces'
ROOT = '01M7MZT8KX547464YYS76QNP4G'
norm = lambda s: re.sub(r'\s+', ' ', s or '').strip()            # 空白だけを正規化
texts = lambda xs: [x['text'] if isinstance(x, dict) else str(x) for x in (xs or [])]

c = sqlite3.connect(DB, uri=True)
ids = [r[0] for r in c.execute(
    "with recursive d(id) as (select id from tasks where parent_id=? "
    "union all select t.id from tasks t join d on t.parent_id=d.id) select id from d", (ROOT,))]
qm = ','.join('?' * len(ids))
task = {}
for tid, obj, js in c.execute(f"select id, objective, json from tasks where id in ({qm})", ids):
    j = json.loads(js)
    p = (j.get('workspace') or {}).get('path') or tid
    task[tid] = (obj or '', texts(j.get('acceptance')), p if os.path.isabs(p) else os.path.join(WS, p))
wu = {wid: (key, json.loads(js)) for wid, key, js in c.execute(f"select id, key, json from work_units where task_id in ({qm})", ids)}
plans = {}  # task -> [(created_at, version, {key: unit})]
for tid, ver, ca, js in c.execute(f"select task_id, version, created_at, json from execution_plans where task_id in ({qm})", ids):
    j = json.loads(js)
    plans.setdefault(tid, []).append((ca, ver, {u['key']: u for u in (j.get('units') or []) + (j.get('work_units') or [])}))

def wu_spec(tid, wid, started):
    """WU の run: run の開始時に有効だった計画の版（created_at <= started の最大の版）の unit の objective・done_when。"""
    key, cur = wu[wid]
    for ca, ver, units in sorted(plans.get(tid, []), key=lambda x: x[1], reverse=True):
        if ca <= started and key in units:
            u = units[key]
            return u.get('objective') or '', texts(u.get('done_when')), f'plan_v{ver}', cur
    return cur.get('objective') or '', texts(cur.get('done_when')), 'wu_row', cur

out = [['task_id', 'run_id', 'kind', 'objective_chars', 'objective_tail', 'acceptance_total', 'acceptance_found', 'prompt_path']]
detail = []
for run_id, tid, wid, role, started in c.execute(
        f"select run_id, task_id, work_unit_id, role, started_at from runs where task_id in ({qm}) "
        "and role in ('worker','planner','reviewer') order by started_at, run_id", ids):
    tobj, tacc, tws = task[tid]
    if wid:   # WorkUnit の run は WU の objective と done_when
        obj, acc, src, cur = wu_spec(tid, wid, started)
    else:     # planner・reviewer・WU の無い worker は task の objective と受入条件（reviewer は review する task）
        obj, acc, src, cur = tobj, tacc, 'task', None
    path = os.path.join(tws, 'runs', run_id, 'prompt.txt')
    d = dict(task=tid, run=run_id, kind=role, wu=wu[wid][0] if wid else None, src=src, started=started, path=path)
    if not os.path.isfile(path):
        tail, found = 'missing', 0
        d.update(dir=os.path.isdir(os.path.dirname(path)))
    else:
        ptxt = norm(open(path, encoding='utf-8', errors='replace').read())
        tail = 'found' if norm(obj)[-200:] in ptxt else 'cut'
        miss = [a for a in acc if norm(a) not in ptxt]
        found = len(acc) - len(miss)
        d.update(missing_acc=miss, plen=len(ptxt))
        if wid:   # 参考（md だけ）: 親 task の objective の末尾・task の受入条件・今の WU 行との比較
            d.update(parent_tail=norm(tobj)[-200:] in ptxt,
                     task_acc_found=sum(norm(a) in ptxt for a in tacc), task_acc_total=len(tacc),
                     cur_row_tail=norm(cur.get('objective'))[-200:] in ptxt,
                     cur_row_same=norm(cur.get('objective')) == norm(obj))
    d.update(tail=tail, obj_head=norm(obj)[:80])
    out.append([tid, run_id, role, str(len(obj)), tail, str(len(acc)), str(found), path if tail != 'missing' else ''])
    detail.append(d)

sys.stdout.write('\n'.join('\t'.join(r) for r in out) + '\n')
if os.environ.get('PA_DETAIL'):
    json.dump(detail, open(os.environ['PA_DETAIL'], 'w'), ensure_ascii=False, indent=1)
```

集計の command:

```sh
awk -F'\t' 'NR>1{k[$3" "$5]++; if($5!="missing"){t[$3]+=$6; f[$3]+=$7}} END{for(x in k) print x, k[x]; for(x in t) print x, "acceptance", f[x]"/"t[x]}' \
  agent-docs/progress/2026-11-19-arch-v4-h/prompt-audit.tsv | sort
```

## 検査（提出時の HEAD で実行）

基準は `$CELERIS_WU_BASE`（この WorkUnit の base）。commit は件名 `arch-v4 H prompt-audit: アーキv4 の木の実 prompt の objective と受入条件の監査` の 1 件。

- WU の check 1（tsv が 10 行以上、kind に worker と reviewer、md に mode=ro）: exit 0
- WU の check 2（`objective_tail` が found・cut・missing のどれか）: exit 0
- WU の check 3（crates・scripts・web・Cargo の差分 0）: exit 0
- WU の check 4（範囲。変えた path は prompt-audit.md と prompt-audit.tsv だけ）: exit 0
- `bash scripts/dev/test-affected.sh`・clippy・fmt は走らせていない（変えたのは agent-docs/progress の md と tsv だけで、Rust と web の変更は無い）。
