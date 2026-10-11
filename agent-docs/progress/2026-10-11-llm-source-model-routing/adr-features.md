# adr-features: 判断 record・task 記述・export の ADR と本番 DB の埋まり具合

- task: 01M4M8D4AQHBC2KG3H1BPJ7Z0J / unit: adr-features
- status: done (2026-10-11)
- 成果: 新 ADR [2026-10-11-task-routing-features.md](../../adr/2026-10-11-task-routing-features.md)、主 ADR D2・D5・改訂履歴から参照
- コード変更: なし

## 人の訂正の反映

指示 3 の「固定の数値特徴量 schema」は、人の補足（decision model は自然言語の入力と自然言語の選択肢から各選択肢の確率を出す）と
人の補足 2（Input = 要求・context・constraints・candidates、Output = selected・action・score）で訂正された。ADR はこれに沿って、
`celeris.task-features/1` を「決定的に組み立てた自然言語の brief が主、数値・カテゴリの aux が補助」とし、選択肢は版つき builder の自然言語、
判断は `celeris.route-decision/1`（Input/Output の形）、結果は `celeris.route-outcome/1` に分け、`celerisctl routing export` の JSONL で結合して出す。
判定器は dispatcher・store の外で判定し `route_judgments` に書いたものを `RoutePolicy` が読む境界にした。decision model 本体は作らない。

## 本番 DB での埋まり具合（2026-10-11 実測、mode=ro）

単位: execute task 1,149 件（done 1,019）、WU（kind<>'task'）2,456 件、終わった worker run 3,555 件。欠測率 = 値が無い割合（%）。

| 欄（brief の節／aux） | 埋まる件数 | 欠測率 % |
|---|---|---|
| task.title | 1149 | 0.0 |
| task.objective | 1149 | 0.0 |
| task.acceptance ≥1 | 542 | 52.8 |
| task.acceptance に command check ≥1 | 361 | 68.6 |
| task.genre | 899 | 21.8 |
| task.worker_hint.tier（lane） | 1149 | 0.0 |
| task.parent_id（子 task） | 327 | 71.5（root は親なしが正常。depth は全件出せる） |
| task.project_id | 1000 | 13.0 |
| task.repos ≥1 | 525 | 54.3 |
| task.depends_on ≥1 | 39 | 96.6（依存なしが正常） |
| task.assignee | 1097 | 4.5 |
| task.budget | 1149 | 0.0 |
| task.execution_plan あり | 277 | 75.9 |
| task.review_verdict あり（outcome） | 514 | 55.3 |
| wu.objective | 2456 | 0.0 |
| wu.checks ≥1 | 1439 | 41.4 |
| wu.done_when ≥1 | 1411 | 42.5 |
| wu.context.paths ≥1（触る範囲） | 1418 | 42.3 |
| wu.depends_on ≥1 | 1794 | 27.0 |
| wu.phase | 2375 | 3.3 |
| wu.retries > 0 | 228 | — |
| wu.write_set（label: 変更範囲） | 559 | 77.2 |
| wu.head_commit（label: 変更行数を数える基） | 1314 | 46.5 |
| run.work_unit_id | 2553 | 28.2 |
| run.adapter | 3555 | 0.0 |
| run.model | 3403 | 4.3 |
| run.account | 3241 | 8.8 |
| run.usage.cost_usd（label） | 1524 | 57.1 |
| run.usage.context_tokens | 194 | 94.5 |
| run.metrics.wall_ms（label） | 3359 | 5.5 |
| run.write_set（label） | 1327 | 62.7 |
| run の routing_decided | 3529 | 0.7 |
| run の decision.features（ADR-0069 9 軸） | 3529 | 0.7 |
| run の routing_features_recorded（旧 optimizer） | 1196 | 66.4 |
| run の routing_outcome_recorded | 1226 | 65.5 |

補足（同じ DB、python sqlite3 で測定）:
- 文の長さ: task.objective は p50 1,596 字・p90 28,804 字・最大 62,005 字、WU objective は p50 571 字・p90 1,406 字 → brief に字数上限と省略記法が要る（ADR D2）。
- review_verdict 4,005 件はすべて reason を持つが、reason に stdout の tail が入るので brief には criterion と pass・exit だけを入れる。
- 完了でない run を持つ WU は 485 件（「過去の試行」節が初回以外になる対象）。
- routing_decided の lane: standard 3,854・cheap 990・frontier 258。

読み: brief の主な節（要求・lane・予算・ADR-0069 の性質）は 0〜1% の欠測で全件に出せる。期待する成果物（check）と触る範囲は WU の約 6 割、
task の acceptance は約半分にしか無く、無いことを brief に明記して aux の `missing` に入れる設計で受ける。label のうち費用（57% 欠測）・変更範囲
（63〜77% 欠測）・context_tokens（94.5% 欠測）は今の記録が薄いので、build 段 `task-features` で判断ごとの outcome として取り直す。
過去分は backfill しない（ADR D7）。

## 使った SQL

`sqlite3 "file:/local/celeris/data/db/celeris.sqlite3?mode=ro" < features-coverage.sql`（実行 3.7 秒、exit 0）。

```sql
.mode list
.separator |
-- 単位: execute task（kind='execute'）
WITH t AS (SELECT * FROM tasks WHERE kind='execute')
SELECT 'task.n', count(*), '' FROM t
UNION ALL SELECT 'task.title', sum(length(trim(title))>0), printf('%.1f', 100.0*sum(length(trim(title))=0)/count(*)) FROM t
UNION ALL SELECT 'task.objective', sum(length(trim(coalesce(objective,'')))>0), printf('%.1f', 100.0*sum(length(trim(coalesce(objective,'')))=0)/count(*)) FROM t
UNION ALL SELECT 'task.acceptance>=1', sum(json_array_length(json,'$.acceptance')>0), printf('%.1f', 100.0*sum(json_array_length(json,'$.acceptance')=0)/count(*)) FROM t
UNION ALL SELECT 'task.acceptance.command>=1', sum(EXISTS(SELECT 1 FROM json_each(t.json,'$.acceptance') a WHERE json_extract(a.value,'$.check.type')='command')), printf('%.1f', 100.0*sum(NOT EXISTS(SELECT 1 FROM json_each(t.json,'$.acceptance') a WHERE json_extract(a.value,'$.check.type')='command'))/count(*)) FROM t
UNION ALL SELECT 'task.genre', sum(genre IS NOT NULL AND genre<>''), printf('%.1f', 100.0*sum(genre IS NULL OR genre='')/count(*)) FROM t
UNION ALL SELECT 'task.worker_hint.tier', sum(json_extract(json,'$.worker_hint.tier') IS NOT NULL), printf('%.1f', 100.0*sum(json_extract(json,'$.worker_hint.tier') IS NULL)/count(*)) FROM t
UNION ALL SELECT 'task.parent_id(子)', sum(parent_id IS NOT NULL), printf('%.1f', 100.0*sum(parent_id IS NULL)/count(*)) FROM t
UNION ALL SELECT 'task.project_id', sum(project_id IS NOT NULL), printf('%.1f', 100.0*sum(project_id IS NULL)/count(*)) FROM t
UNION ALL SELECT 'task.repos>=1', sum(coalesce(json_array_length(repos_json),0)>0), printf('%.1f', 100.0*sum(coalesce(json_array_length(repos_json),0)=0)/count(*)) FROM t
UNION ALL SELECT 'task.depends_on>=1', sum(json_array_length(json,'$.depends_on')>0), printf('%.1f', 100.0*sum(json_array_length(json,'$.depends_on')=0)/count(*)) FROM t
UNION ALL SELECT 'task.assignee', sum(assignee IS NOT NULL), printf('%.1f', 100.0*sum(assignee IS NULL)/count(*)) FROM t
UNION ALL SELECT 'task.budget', sum(json_extract(json,'$.budget') IS NOT NULL), printf('%.1f', 100.0*sum(json_extract(json,'$.budget') IS NULL)/count(*)) FROM t
UNION ALL SELECT 'task.labels>=1', sum(coalesce(json_array_length(labels_json),0)>0), printf('%.1f', 100.0*sum(coalesce(json_array_length(labels_json),0)=0)/count(*)) FROM t
UNION ALL SELECT 'task.execution_plan', sum(EXISTS(SELECT 1 FROM execution_plans p WHERE p.task_id=t.id)), printf('%.1f', 100.0*sum(NOT EXISTS(SELECT 1 FROM execution_plans p WHERE p.task_id=t.id))/count(*)) FROM t
UNION ALL SELECT 'task.review_verdict(outcome)', sum(EXISTS(SELECT 1 FROM events e WHERE e.task_id=t.id AND json_extract(e.json,'$.type')='review_verdict')), printf('%.1f', 100.0*sum(NOT EXISTS(SELECT 1 FROM events e WHERE e.task_id=t.id AND json_extract(e.json,'$.type')='review_verdict'))/count(*)) FROM t
UNION ALL SELECT 'task.status=done', sum(status='done'), '' FROM t;
-- 単位: work unit（kind<>'task' の葉）
WITH w AS (SELECT * FROM work_units WHERE kind<>'task')
SELECT 'wu.n', count(*), '' FROM w
UNION ALL SELECT 'wu.objective', sum(length(coalesce(json_extract(json,'$.objective'),''))>0), printf('%.1f', 100.0*sum(length(coalesce(json_extract(json,'$.objective'),''))=0)/count(*)) FROM w
UNION ALL SELECT 'wu.checks>=1', sum(json_array_length(json,'$.checks')>0), printf('%.1f', 100.0*sum(coalesce(json_array_length(json,'$.checks'),0)=0)/count(*)) FROM w
UNION ALL SELECT 'wu.done_when>=1', sum(json_array_length(json,'$.done_when')>0), printf('%.1f', 100.0*sum(coalesce(json_array_length(json,'$.done_when'),0)=0)/count(*)) FROM w
UNION ALL SELECT 'wu.context.paths>=1', sum(json_array_length(json,'$.context.paths')>0), printf('%.1f', 100.0*sum(coalesce(json_array_length(json,'$.context.paths'),0)=0)/count(*)) FROM w
UNION ALL SELECT 'wu.depends_on>=1', sum(json_array_length(depends_on_json)>0), printf('%.1f', 100.0*sum(coalesce(json_array_length(depends_on_json),0)=0)/count(*)) FROM w
UNION ALL SELECT 'wu.phase', sum(phase IS NOT NULL), printf('%.1f', 100.0*sum(phase IS NULL)/count(*)) FROM w
UNION ALL SELECT 'wu.retries>0', sum(retries>0), '' FROM w
UNION ALL SELECT 'wu.write_set(label)', sum(EXISTS(SELECT 1 FROM work_unit_write_sets s WHERE s.work_unit_id=w.id)), printf('%.1f', 100.0*sum(NOT EXISTS(SELECT 1 FROM work_unit_write_sets s WHERE s.work_unit_id=w.id))/count(*)) FROM w
UNION ALL SELECT 'wu.head_commit(label)', sum(head_commit IS NOT NULL), printf('%.1f', 100.0*sum(head_commit IS NULL)/count(*)) FROM w;
-- 単位: worker run（終わったもの）
WITH r AS (SELECT * FROM runs WHERE role='worker' AND finished_at IS NOT NULL)
SELECT 'run.n', count(*), '' FROM r
UNION ALL SELECT 'run.work_unit_id', sum(work_unit_id IS NOT NULL), printf('%.1f', 100.0*sum(work_unit_id IS NULL)/count(*)) FROM r
UNION ALL SELECT 'run.adapter', sum(adapter IS NOT NULL), printf('%.1f', 100.0*sum(adapter IS NULL)/count(*)) FROM r
UNION ALL SELECT 'run.model', sum(model IS NOT NULL AND model<>''), printf('%.1f', 100.0*sum(model IS NULL OR model='')/count(*)) FROM r
UNION ALL SELECT 'run.account', sum(account IS NOT NULL), printf('%.1f', 100.0*sum(account IS NULL)/count(*)) FROM r
UNION ALL SELECT 'run.usage.cost_usd', sum(json_extract(usage_json,'$.cost_usd') IS NOT NULL), printf('%.1f', 100.0*sum(json_extract(usage_json,'$.cost_usd') IS NULL)/count(*)) FROM r
UNION ALL SELECT 'run.usage.context_tokens', sum(json_extract(usage_json,'$.context_tokens') IS NOT NULL), printf('%.1f', 100.0*sum(json_extract(usage_json,'$.context_tokens') IS NULL)/count(*)) FROM r
UNION ALL SELECT 'run.metrics.wall_ms', sum(json_extract(metrics_json,'$.wall_ms') IS NOT NULL), printf('%.1f', 100.0*sum(json_extract(metrics_json,'$.wall_ms') IS NULL)/count(*)) FROM r
UNION ALL SELECT 'run.write_set(label)', sum(EXISTS(SELECT 1 FROM run_write_sets s WHERE s.run_id=r.run_id)), printf('%.1f', 100.0*sum(NOT EXISTS(SELECT 1 FROM run_write_sets s WHERE s.run_id=r.run_id))/count(*)) FROM r;
-- routing events と run の結合
WITH r AS (SELECT run_id FROM runs WHERE role='worker' AND finished_at IS NOT NULL),
d AS (SELECT DISTINCT json_extract(json,'$.run_id') rid, json_extract(json,'$.record.decision.features') f FROM events WHERE json_extract(json,'$.type')='routing_decided'),
o AS (SELECT DISTINCT json_extract(json,'$.run_id') rid FROM events WHERE json_extract(json,'$.type')='routing_outcome_recorded'),
fr AS (SELECT DISTINCT json_extract(json,'$.run_id') rid FROM events WHERE json_extract(json,'$.type')='routing_features_recorded')
SELECT 'run.routing_decided', (SELECT count(*) FROM r WHERE run_id IN (SELECT rid FROM d)), printf('%.1f', 100.0*(SELECT count(*) FROM r WHERE run_id NOT IN (SELECT rid FROM d WHERE rid IS NOT NULL))/(SELECT count(*) FROM r))
UNION ALL SELECT 'run.decision.features(ADR-0069 9軸)', (SELECT count(*) FROM r WHERE run_id IN (SELECT rid FROM d WHERE f IS NOT NULL)), printf('%.1f', 100.0*(SELECT count(*) FROM r WHERE run_id NOT IN (SELECT rid FROM d WHERE f IS NOT NULL AND rid IS NOT NULL))/(SELECT count(*) FROM r))
UNION ALL SELECT 'run.routing_features_recorded', (SELECT count(*) FROM r WHERE run_id IN (SELECT rid FROM fr)), printf('%.1f', 100.0*(SELECT count(*) FROM r WHERE run_id NOT IN (SELECT rid FROM fr WHERE rid IS NOT NULL))/(SELECT count(*) FROM r))
UNION ALL SELECT 'run.routing_outcome_recorded', (SELECT count(*) FROM r WHERE run_id IN (SELECT rid FROM o)), printf('%.1f', 100.0*(SELECT count(*) FROM r WHERE run_id NOT IN (SELECT rid FROM o WHERE rid IS NOT NULL))/(SELECT count(*) FROM r));
```

長さの分位は python で `select length(objective) from tasks where kind='execute'` と
`select length(json_extract(json,'$.objective')) from work_units where kind<>'task'` を並べて取った。

## 未解決

- ADR「未解決」節の 3 点（secret 置換の実装元、brief 各節の字数上限、changed_lines の数え方の置き場所）は build 段 `task-features` で決める。

## 提案

- build 段 `task-features` の objective は「固定 schema の決定的な抽出」から、この ADR の brief（自然言語）・options・decision・outcome・judgments の表と
  `routing export` の置き換えへ書き直す（integrate-redesign 後の replan で）。
