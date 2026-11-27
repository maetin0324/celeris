---
tasks: [01M7XQA79910VEZ66HWTX4HZVT]
wu: metrics
status: done
completed: 2026-11-27
---
# 戦略層 v1 子 9 metrics: 基準 main から HEAD までの LoC 差分、子ごとのコスト・run 数、長時間 run

## 結論

- LoC（基準 main 7f69057907ad649829ec5a08b5ac7c892b9fe055 → 提出時の HEAD）: production 全言語 +18,105、production Rust +15,615、production TypeScript +2,481、試験を含む自前の総量 +55,179。子 0〜8 の進捗の production 全言語の差の和は +17,284 で、残りの +821 は子でない main の commit と子の base の隙間の分（下の「LoC」）。
- コスト（本番 DB、読み取り専用。2026-11-27T13:30:40.844316595Z 時点）: 子 0〜9 とその子孫の task の runs は計 253 件・639.0749 USD。cost_usd の無い run は 6 件（うち 2 件は子 9 の実行中の run）。親 task 自身の run は 0 件。
- 長時間 run: wall の最長は 4207.7 秒（子 3 の WU cite-attach）。上位 10 件はすべて 3,467 秒以上。上限で止まった run は 17 件（turn の上限 16 件、wall の上限と推定 1 件）。no-progress の上限（WU の blocked 理由 `limit`）で止まった WU は 0 件、Project Controller の判断（no-progress guard・予算の停止を含む）はこの案件で 0 件（状態の行も無い）。
- 子 8 の実 LLM の pilot（operator が worker の外で流した 34 run・8.6335 USD。子 8 の進捗の記載）は runs 表に無いので、上のコストに入っていない。

## LoC

再現の command（分類の規則を基準の commit に固定する。script と toml は基準の commit から取り出し、基準と HEAD の両方に当てる）:

```sh
B=7f69057907ad649829ec5a08b5ac7c892b9fe055
T=$TMPDIR/loc; mkdir -p $T/scripts/dev
git show $B:scripts/dev/source-size-report.py   > $T/scripts/dev/source-size-report.py
git show $B:scripts/dev/source-size-report.toml > $T/scripts/dev/source-size-report.toml
python3 -I $T/scripts/dev/source-size-report.py --root . --config $T/scripts/dev/source-size-report.toml --exclusive-classes --rev $B   --format json > $T/base.json
python3 -I $T/scripts/dev/source-size-report.py --root . --config $T/scripts/dev/source-size-report.toml --exclusive-classes --rev HEAD --format json > $T/head.json
# 欄: views.product_only.lines・views.product_only.rust_lines・matrix.production.typescript.lines・views.own_total_with_tests.lines
```

toml は repo の根ではなく `scripts/dev/source-size-report.toml` にある（基準の commit に `source-size-report.toml` は無い）。2 回とも exit 0。出力の `config_sha256` は基準と HEAD で同じ（同じ toml を当てた）。HEAD は提出時の HEAD で、測定時は main の先端と同じ commit（子 8 の統合の後）。

| 欄（metrics.json の名） | 基準 | HEAD | 差 |
|---|---:|---:|---:|
| production 全言語（`views.product_only.lines`） | 376,018 | 394,123 | +18,105 |
| production Rust（`views.product_only.rust_lines`） | 331,327 | 346,942 | +15,615 |
| production TypeScript（`matrix.production.typescript.lines`） | 33,557 | 36,038 | +2,481 |
| 試験を含む自前の総量（`views.own_total_with_tests.lines`） | 1,034,966 | 1,090,145 | +55,179 |

子 1〜8（と子 0）の進捗の LoC の節の値（写し。base は各子の `git merge-base HEAD main` で、子ごとに違う）:

| 子 | 出典（進捗 file の節） | production 全言語 | production Rust | production TS | 自前の総量 |
|---|---|---|---|---|---|
| 0 | `agent-docs/progress/2026-11-21-strategy-v1-0.md` ## LoC の差分（値は「証拠」の節） | 376,018 → 376,018（0） | 節に無い | 節に無い | 1,034,966 → 1,034,966（0） |
| 1 | `agent-docs/progress/2026-11-21-strategy-v1-1.md` ## 3. LoC（受入条件 2） | 376,018 → 376,359（+341） | 331,327 → 331,668（+341） | 節に無い | 1,034,966 → 1,037,789（+2,823） |
| 2 | `agent-docs/progress/2026-11-21-strategy-v1-2.md` ## 3. LoC | 376,359 → 378,484（+2,125） | 331,668 → 333,793（+2,125） | 節に無い | 1,037,789 → 1,041,768（+3,979） |
| 3 | `agent-docs/progress/2026-11-21-strategy-v1-3.md` ## LoC | 378,597 → 381,294（+2,697） | 333,906 → 335,764（+1,858） | 33,557 → 34,396（+839） | 1,042,742 → 1,049,667（+6,925） |
| 4 | `agent-docs/progress/2026-11-22-strategy-v1-4.md` ## LoC | 381,294 → 384,090（+2,796） | 335,764 → 338,560（+2,796） | 節に無い | 1,049,562 → 1,056,466（+6,904） |
| 5 | `agent-docs/progress/2026-11-22-strategy-v1-5.md` ## LoC | 381,294 → 382,436（+1,142） | 335,764 → 336,906（+1,142） | 節に無い | 1,049,562 → 1,053,886（+4,324） |
| 6 | `agent-docs/progress/2026-11-22-strategy-v1-6.md` ## LoC | 382,436 → 385,396（+2,960） | 336,906 → 339,739（+2,833） | 節に無い | 1,053,886 → 1,064,487（+10,601） |
| 7 | `agent-docs/progress/2026-11-23-strategy-v1-7.md` ## LoC | 388,192 → 393,338（+5,146） | 342,535 → 346,168（+3,633） | 34,523 → 36,035（+1,512） | 1,071,391 → 1,082,712（+11,321） |
| 8 | `agent-docs/progress/2026-11-24-strategy-v1-8.md` ## LoC | 393,338 → 393,415（+77） | 346,168 → 346,245（+77） | 36,035 → 36,035（0） | 1,082,712 → 1,087,451（+4,739） |

子ごとの注（metrics.json の `loc.children[].note`）:

- 子 0: production・test の差 0。Rust・TS の欄は節に数が無い（production の差 0 とだけある）
- 子 1: TS の欄は節に無い（production の差は Rust だけと書いてある）
- 子 2: TS の欄は節に無い
- 子 3: own_total_with_tests は dagbench-flake の evaluation_infra +134 を含む（子 3 だけなら +6,791）
- 子 4: TS は節で「production TypeScript ほか 差 0」（base・HEAD の値は書かれていない）
- 子 5: TS の欄は節に無い
- 子 6: TS の欄は節に無い（production 全言語 − Rust = +127）

- 子 0〜8 の production 全言語の差の和は +17,284。基準から HEAD までの差 +18,105 との差 +821 は、基準から HEAD までの差には、件名に strategy-v1 を含まない merge 以外の main の commit（82 件。子の WU の commit が一部入りうる）の差と、子の base の間の隙間（例: 子 2 の HEAD 378,484 と子 3 の base 378,597）が入る。子ごとの値は各子の base が違うので、足し算は近似。この差を子ごとに割り付ける測定はしていない（未確認）。
- 依頼は LoC の純減を求めていない。純増の理由は各子の進捗の LoC の節にある（新しい契約の追加。子 8 は評価器の入力の修正 +77）。

## コスト

数え方: 子 0〜9 の task（親 01M7XQ9PS0AFKKMRJ7P5ZMKQQE の直下）ごとに、その task と `parent_id` を辿った子孫の task の `runs` の行を数え、`usage_json` の `cost_usd` を足す。`cost_usd` が無い（NULL）run は件数で別に数える。run の role（worker・planner・reviewer）はすべて入れる。

```sql
WITH RECURSIVE tree(root, id) AS (
  SELECT id, id FROM tasks WHERE parent_id = '01M7XQ9PS0AFKKMRJ7P5ZMKQQE'
  UNION ALL SELECT tree.root, t.id FROM tasks t JOIN tree ON t.parent_id = tree.id)
SELECT tree.root, count(DISTINCT tree.id), count(r.run_id),
  coalesce(sum(json_extract(r.usage_json,'$.cost_usd')), 0),
  sum(CASE WHEN r.run_id IS NOT NULL AND json_extract(r.usage_json,'$.cost_usd') IS NULL THEN 1 ELSE 0 END),
  sum(CASE WHEN r.status = 'running' THEN 1 ELSE 0 END)
FROM tree LEFT JOIN runs r ON r.task_id = tree.id GROUP BY tree.root;
```

| 子 | task | 子孫を含む task 数 | runs | cost_usd の和 | cost の無い run | 実行中 |
|---|---|---:|---:|---:|---:|---:|
| 0 | 01M7XQ9PZQ3DQZMYKNFNPTPBQJ（done） | 1 | 3 | 7.4932 | 0 | 0 |
| 1 | 01M7XQ9Q4P4ZQ38Y509J9XYAQ0（done） | 3 | 48 | 110.6162 | 0 | 0 |
| 2 | 01M7XQ9Q9EB3CH59TSNYM9HBC8（done） | 1 | 11 | 26.3848 | 1 | 0 |
| 3 | 01M7XQ9QE8STND208NXG90F743（done） | 1 | 24 | 69.3147 | 0 | 0 |
| 4 | 01M7XQ9QK1KC34Q2A6PJ960TJB（done） | 2 | 21 | 55.4509 | 0 | 0 |
| 5 | 01M7XQ9QQXY1DY56EXE0TT8X6W（done） | 1 | 19 | 52.9726 | 0 | 0 |
| 6 | 01M7XQ9QWK58ESMW42Q1TGT3WZ（done） | 1 | 57 | 145.4111 | 0 | 0 |
| 7 | 01M7XQ9R1EHA8GQ0TZVRM4FBVX（done） | 3 | 32 | 86.6218 | 0 | 0 |
| 8 | 01M7XQA74FP70G3T3KSTP02S0W（done） | 2 | 35 | 82.8550 | 3 | 0 |
| 9 | 01M7XQA79910VEZ66HWTX4HZVT（running） | 1 | 3 | 1.9546 | 2 | 2 |
| 計 | | 16 | 253 | 639.0749 | 6 | 2 |

cost の無い run 6 件の内訳: 子 2 の worker 1 件（budget_exhausted、claude-code。usage に cost_usd が無い）、子 8 の worker 1 件（codex・gpt-6.1-sol、completed。input_tokens は約 395 万で、価格が付いていない）と planner 2 件（harness_error、token 0）、子 9 の worker 2 件（この集計の時点で実行中）。子 9 の値は集計の時点のもので、この後の run で増える。

## 長時間 run

数え方: wall は `finished_at − started_at`（秒。`julianday` の差 × 86400）。`finished_at` の無い run（実行中）は除く。上限で止まった run は `runs.status = 'budget_exhausted'` の行で、上限の種類は同じ run の `worker_finished` の event の `end.kind` から取る。`worker_finished` が無い run は `metrics_json.wall_ms` を見て wall の上限と推定する。

```sql
-- 上位 10 件（tree は上と同じ CTE）
SELECT tree.root, r.run_id, w.key, r.role, r.status,
  round((julianday(r.finished_at) - julianday(r.started_at)) * 86400, 1) AS wall_s,
  json_extract(r.usage_json,'$.cost_usd')
FROM tree JOIN runs r ON r.task_id = tree.id LEFT JOIN work_units w ON w.id = r.work_unit_id
WHERE r.finished_at IS NOT NULL ORDER BY wall_s DESC LIMIT 10;
-- 上限で止まった run と上限の種類
SELECT tree.root, r.run_id, w.key,
  (SELECT json_extract(e.json,'$.end.kind') FROM events e WHERE e.task_id = r.task_id
     AND json_extract(e.json,'$.type') = 'worker_finished' AND json_extract(e.json,'$.run_id') = r.run_id),
  json_extract(r.metrics_json,'$.wall_ms')
FROM tree JOIN runs r ON r.task_id = tree.id LEFT JOIN work_units w ON w.id = r.work_unit_id
WHERE r.status = 'budget_exhausted';
-- WU の blocked・failed の理由（no-progress の上限は reason 'limit'。crates/task-dispatch/src/execution_scheduler.rs）
SELECT json_extract(e.json,'$.reason'), count(*), count(DISTINCT json_extract(e.json,'$.work_unit_id'))
FROM events e WHERE e.task_id IN (SELECT id FROM tree)
  AND json_extract(e.json,'$.type') = 'work_unit_transitioned' AND json_extract(e.json,'$.to') IN ('blocked','failed')
GROUP BY 1;
-- task の blocked・failed・cancelled の理由（親を含む）
SELECT json_extract(e.json,'$.reason'), count(*), count(DISTINCT e.task_id)
FROM events e WHERE e.task_id IN (SELECT id FROM tree UNION SELECT '01M7XQ9PS0AFKKMRJ7P5ZMKQQE')
  AND json_extract(e.json,'$.type') = 'transitioned' AND json_extract(e.json,'$.to') IN ('blocked','failed','cancelled')
GROUP BY 1;
-- Project Controller（no-progress guard・予算の停止はここに判断として残る）
SELECT (SELECT count(*) FROM project_controller_state WHERE project_id = '01M2WTS3DKNZBSZ2JMVB4CZMBW'),
       (SELECT count(*) FROM project_controller_decisions WHERE project_id = '01M2WTS3DKNZBSZ2JMVB4CZMBW');
```

wall の上位 10 件:

| 順 | 子 | WU | role | status | wall（秒） | cost_usd |
|---:|---:|---|---|---|---:|---:|
| 1 | 3 | cite-attach | worker | completed | 4207.7 | 4.4334 |
| 2 | 7 | proposal-api | worker | failed | 4151.7 | 4.0212 |
| 3 | 6 | blind-eval-runs | worker | completed | 3980.5 | 5.1652 |
| 4 | 5 | shadow-case | worker | failed | 3923.2 | 5.3023 |
| 5 | 3 | edit-retract | worker | failed | 3769.0 | 5.6657 |
| 6 | 2 | request-prompt | worker | budget_exhausted | 3609.8 | — |
| 7 | 7 | dedup-api | worker | completed | 3590.7 | 3.0393 |
| 8 | 7 | publish-api | worker | completed | 3553.3 | 5.6785 |
| 9 | 5 | shadow-case | worker | completed | 3509.5 | 1.1772 |
| 10 | 1 | settle-leaf | worker | completed | 3467.6 | 5.9527 |

上位 10 件の終わり方（`worker_finished` の `end`）: completed 6 件、failed（retryable。WU の check の不合格でやり直し）3 件、budget_exhausted 1 件。run の id は metrics.json の `long_runs.top10_by_wall` にある。

上限で止まった run（17 件）:

| 子 | WU | role | 上限 | wall（秒） |
|---:|---|---|---|---:|
| 1 | prompt-fix | worker | turns | 1416.5 |
| 1 | root-artifacts | worker | turns | 982.8 |
| 2 | request-prompt | worker | wall（推定: worker_finished が無く wall_ms=3609679） | 3609.8 |
| 2 | strategy-gate | worker | turns | 1062.8 |
| 3 | mode-gate | worker | turns | 1080.7 |
| 3 | edit-retract | worker | turns | 1067.3 |
| 3 | web-cite | worker | turns | 480.2 |
| 6 | review-launcher | worker | turns | 1279.2 |
| 6 | strategy-read | worker | turns | 1247.0 |
| 6 | cos-skill-checkpoint | worker | turns | 747.0 |
| 6 | checkpoint-act-fix | worker | turns | 1771.3 |
| 6 | controller-launch | worker | turns | 2233.7 |
| 6 | close-docfix | worker | turns | 1006.2 |
| 7 | proposal-api | worker | turns | 1072.3 |
| 7 | web-publish | worker | turns | 748.7 |
| 7 | drilldown-e2e | worker | turns | 718.9 |
| 8 | artifact-name | worker | turns | 2094.4 |

run の status の内訳（子 0〜9 の木）: budget_exhausted 17、completed 178、failed 25、harness_error 5、question 2、running 2、yielded 24。

停止の件数:

- WU の blocked・failed（遷移の数 / WU の数）: `decision` 13 / 12、`integration_failed` 1 / 1、`plan_issue` 19 / 18、`question` 2 / 1。no-progress の上限（`limit`）は 0。
- task の blocked（遷移の数 / task の数）: `awaiting_human` 5 / 2、`awaiting_plan_approval` 12 / 6、`worker_question` 2 / 1。failed・cancelled への遷移は 0。
- Project Controller: 案件 01M2WTS3DKNZBSZ2JMVB4CZMBW の `project_controller_state` の行 0 件、`project_controller_decisions` 0 件。この案件で controller は動いておらず（行が無いので mode は既定の off）、no-progress guard・予算の停止の判断は起きていない。
- 予算の停止: run の turn の上限 16 件・wall の上限（推定）1 件のほかに、予算（USD の上限・quota）で止まった task・WU の記録は見つからなかった（上の task・WU の理由に budget・quota の類が無い）。

## 限界・未確認

- コストは runs 表の `usage_json.cost_usd` の和で、cost の無い 6 run の費用は入っていない。operator が worker の外で流した子 8 の pilot（34 run・8.6335 USD）も入っていない。
- wall の上限の 1 件は `worker_finished` が無いので、`metrics_json.wall_ms`（3,609,679）から推定した。その run の wall の上限の値は DB から確かめていない（未確認）。
- 上位 10 件のうち 6 件は 3,600 秒を超えて終わっている。各 run の wall の上限（task の `max_wall_secs` 等）との照合はしていない（未確認）。
- LoC の基準から HEAD までの差のうち、子でない main の commit の分（約 +821 行、production 全言語）を commit ごとに割り付けていない（未確認）。

## 再現の全体

DB の集計は次の script（stdlib だけ。`mode=ro` で開く）を `python3 -I metrics_db.py > db.json` で流した。LoC は上の「LoC」の command。metrics.json は 2 つの出力と、子 0〜8 の進捗の LoC の節の値（手で写した）を合わせたもの。

```python
import json, sqlite3, sys
DB = "file:/local/celeris/data/db/celeris.sqlite3?mode=ro"
PARENT = "01M7XQ9PS0AFKKMRJ7P5ZMKQQE"
con = sqlite3.connect(DB, uri=True)
TREE = """WITH RECURSIVE tree(root, id) AS (
  SELECT id, id FROM tasks WHERE parent_id = :parent
  UNION ALL SELECT tree.root, t.id FROM tasks t JOIN tree ON t.parent_id = tree.id)"""
children = con.execute("""SELECT id, json_extract(json,'$.title'), status FROM tasks
  WHERE parent_id = :parent ORDER BY created_at, id""", {"parent": PARENT}).fetchall()
COST = TREE + """
SELECT tree.root, count(DISTINCT tree.id), count(r.run_id),
  coalesce(sum(json_extract(r.usage_json,'$.cost_usd')), 0),
  sum(CASE WHEN r.run_id IS NOT NULL AND json_extract(r.usage_json,'$.cost_usd') IS NULL THEN 1 ELSE 0 END),
  sum(CASE WHEN r.status = 'running' THEN 1 ELSE 0 END)
FROM tree LEFT JOIN runs r ON r.task_id = tree.id GROUP BY tree.root"""
rows = {r[0]: r for r in con.execute(COST, {"parent": PARENT})}
out_children = []
for i, (tid, title, status) in enumerate(children):
    _, ntasks, nruns, cost, nocost, running = rows[tid]
    out_children.append({"child": i, "task_id": tid, "title": title, "status": status,
        "tasks_in_subtree": ntasks, "runs": nruns, "cost_usd": round(cost, 4),
        "runs_without_cost": nocost, "runs_running_at_snapshot": running})
own = con.execute("""SELECT count(*), coalesce(sum(json_extract(usage_json,'$.cost_usd')),0),
  sum(CASE WHEN json_extract(usage_json,'$.cost_usd') IS NULL THEN 1 ELSE 0 END)
  FROM runs WHERE task_id = :parent""", {"parent": PARENT}).fetchone()
ALLRUNS = TREE + """, labeled AS (SELECT tree.root, r.* FROM tree JOIN runs r ON r.task_id = tree.id)"""
idx = {c["task_id"]: c["child"] for c in out_children}
TOP = ALLRUNS + """
SELECT l.root, l.run_id, l.task_id, w.key, l.role, l.status,
  round((julianday(l.finished_at) - julianday(l.started_at)) * 86400, 1) AS wall_s,
  json_extract(l.usage_json,'$.cost_usd'), l.started_at
FROM labeled l LEFT JOIN work_units w ON w.id = l.work_unit_id
WHERE l.finished_at IS NOT NULL ORDER BY wall_s DESC LIMIT 10"""
top = [{"child": idx[r[0]], "run_id": r[1], "task_id": r[2], "work_unit": r[3], "role": r[4],
        "status": r[5], "wall_s": r[6], "cost_usd": r[7], "started_at": r[8]} for r in con.execute(TOP, {"parent": PARENT})]
LIM = ALLRUNS + """
SELECT l.root, l.run_id, l.task_id, w.key, l.role,
  round((julianday(l.finished_at) - julianday(l.started_at)) * 86400, 1),
  (SELECT json_extract(e.json,'$.end.kind') FROM events e WHERE e.task_id = l.task_id
     AND json_extract(e.json,'$.type') = 'worker_finished' AND json_extract(e.json,'$.run_id') = l.run_id),
  json_extract(l.metrics_json,'$.wall_ms'), json_extract(l.usage_json,'$.cost_usd')
FROM labeled l LEFT JOIN work_units w ON w.id = l.work_unit_id
WHERE l.status = 'budget_exhausted' ORDER BY l.started_at"""
lim = []
for r in con.execute(LIM, {"parent": PARENT}):
    kind = r[6] if r[6] else ("wall（推定: worker_finished が無く wall_ms=%s）" % r[7])
    lim.append({"child": idx[r[0]], "run_id": r[1], "task_id": r[2], "work_unit": r[3], "role": r[4],
                "wall_s": r[5], "limit": kind, "cost_usd": r[8]})
STATUS = ALLRUNS + " SELECT status, count(*) FROM labeled GROUP BY status ORDER BY status"
WUBLK = TREE + """
SELECT json_extract(e.json,'$.reason'), count(*), count(DISTINCT json_extract(e.json,'$.work_unit_id'))
FROM events e WHERE e.task_id IN (SELECT id FROM tree)
  AND json_extract(e.json,'$.type') = 'work_unit_transitioned' AND json_extract(e.json,'$.to') IN ('blocked','failed')
GROUP BY 1 ORDER BY 1"""
TASKBLK = TREE + """
SELECT json_extract(e.json,'$.reason'), count(*), count(DISTINCT e.task_id)
FROM events e WHERE e.task_id IN (SELECT id FROM tree UNION SELECT :parent)
  AND json_extract(e.json,'$.type') = 'transitioned' AND json_extract(e.json,'$.to') IN ('blocked','failed','cancelled')
GROUP BY 1 ORDER BY 1"""
pc = con.execute("""SELECT (SELECT count(*) FROM project_controller_state WHERE project_id = :p),
  (SELECT count(*) FROM project_controller_decisions WHERE project_id = :p)""", {"p": "01M2WTS3DKNZBSZ2JMVB4CZMBW"}).fetchone()
wublk = {r[0]: {"transitions": r[1], "work_units": r[2]} for r in con.execute(WUBLK, {"parent": PARENT})}
taskblk = {r[0]: {"transitions": r[1], "tasks": r[2]} for r in con.execute(TASKBLK, {"parent": PARENT})}
snap = con.execute("SELECT max(ts) FROM events").fetchone()[0]
print(json.dumps({
  "snapshot_max_event_ts": snap,
  "cost": {"children": out_children,
    "total_runs": sum(c["runs"] for c in out_children),
    "total_cost_usd": round(sum(c["cost_usd"] for c in out_children), 4),
    "total_runs_without_cost": sum(c["runs_without_cost"] for c in out_children),
    "parent_own": {"task_id": PARENT, "runs": own[0], "cost_usd": round(own[1], 4), "runs_without_cost": own[2] or 0}},
  "long_runs": {"top10_by_wall": top, "limit_stopped_runs": lim,
    "limit_stopped_counts": {"turns": sum(1 for x in lim if x["limit"] == "turns"),
                             "wall_estimated": sum(1 for x in lim if x["limit"] != "turns")},
    "run_status_counts": {r[0]: r[1] for r in con.execute(STATUS, {"parent": PARENT})},
    "work_unit_blocked_or_failed_by_reason": wublk,
    "work_unit_no_progress_limit": wublk.get("limit", {"transitions": 0, "work_units": 0}),
    "task_blocked_failed_cancelled_by_reason": taskblk,
    "project_controller": {"state_rows": pc[0], "decisions": pc[1]}}
}, ensure_ascii=False, indent=2))
```
