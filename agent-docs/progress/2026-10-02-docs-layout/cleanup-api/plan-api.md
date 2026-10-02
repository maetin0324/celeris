---
tasks: [01M3YNFVFZG5AFFM0RS05X22ZT]
status: done
updated: 2026-10-02
completed: 2026-10-02
---

# cleanup-api / plan-api: gui-api.md §3.125（実行・計画・木・決定）を task-api/task-core の実装に照らして直す

final review の指摘（§3.125.2 が plan.schema を /1・/2 に限定、§3.125.3 が `work_units` を必須と書く）を直し、
§3.125 の他の小節も route・ハンドラ・型（serde 属性）と 1 つずつ照合した。`docs/api/v1/api-v1.schema.json` と
crates/・web/・gui/・scripts/ は変えていない。

## 処理

| 処理(削除/統合/移動/修正) | 旧パス | 新パス or - | 理由 | 最後の commit |
|---|---|---|---|---|
| 修正 | docs/api/v1/gui-api.md | - | §3.125.1・§3.125.2・§3.125.3・§3.125.5 を実装に合わせた（下記）。crates から参照される正本なので場所はそのまま | cfffd0f7 |

## 小節ごとの結論と根拠

- §3.125.1 `GET /tasks/{id}/execution` — **修正**。`gate` / `phase` / `plan` を「`| null`」と書いていたが、3 つとも
  `skip_serializing_if = "Option::is_none"` で値が無ければ省略（`crates/task-api/src/types.rs:1945-1951`）。
  記載の無かった `phase_checkpoint` / `awaiting_children` / `plan_approval`（`types.rs:1955-1963`）を足した。
  `runs`・`metrics` 必須、no_query（`crates/task-api/src/execution.rs:302` 付近）は一致。
- §3.125.2 `GET /tasks/{id}/execution-plan` — **修正**。`plan.schema` に `/3` を加え、`plan` は採用した spec を
  そのまま返すと書いた。根拠: `EXECUTION_PLAN_SCHEMA_V3`（`crates/task-core/src/execution_plan.rs:23`）、
  `ExecutionPlanView::new` が `plan: plan.spec`（`types.rs:1447-1459`）、handler（`execution.rs:102-126`）、
  採用時に保存する spec は `validated.spec`（/3 のまま。`crates/task-ops/src/execution.rs:117-125`）、
  `stages` / `units` / `decisions` は空なら出力しない（`execution_plan.rs:191-198`）、WU 行は /3 も段階を工程に
  写して作る（`crates/task-core/src/execution_plan/scheduling.rs:884-893`）。
- §3.125.3 `POST`/`PUT /tasks/{id}/execution-plan` — **修正**。必須欄を schema ごとに書き分けた。型の必須は
  `schema`・`rationale` だけで他は `serde(default)`、`deny_unknown_fields`（`execution_plan.rs:174-199`）→ 不明欄は
  `read_json` の 400（`crates/task-api/src/handlers.rs:268-280`）。/1・/2: `work_units` 1 件以上（`NoWorkUnits`。
  `crates/task-core/src/execution_plan/validation.rs:892-894`）、/2 は `phases` 1 件以上（`:918-921`）と WU の
  `phase`、/1 は `phases`・`children` 空（`:909`・`:938`）、/1・/2 は `stages`/`units`/`decisions` 空（`:963-970`）。
  /3: `phases`・`work_units`・`children` を書くと `V2FieldInV3`（`:1329-1337`）、`stages`・`units` 1 件以上
  （`:1340`・`:1370`）、`[execution.tree]` 無効なら `TreeDisabled`（`:1324-1326`）。旧記述「`children` は現行では
  空配列のみ」は不正確（v2 の検証は `children` を受ける。`:913-915`）ので、「書けるが人の計画では子 Task を
  作らない」（`adopt_plan` が子なしで `adopt_plan_with_children` を呼ぶ。`crates/task-ops/src/execution.rs:40-58`）に直した。
  POST の既存計画 409 は `StoreError::InUse{kind:"execution_plan"}`（`crates/task-api/src/problem.rs:632-635`）、
  PUT の 201/200 の分岐と `replan` は `execution.rs:182-257`、`adoptions` / `decisions_raised` / `replan` の省略条件は
  `types.rs:1428-1438` で一致。
- §3.125.4 `POST /tasks/{id}/tree/adopt` — 一致（据え置き）。`AdoptRequest` 3 欄必須・deny_unknown
  （`crates/task-ops/src/tree_adopt.rs:39-48`）、`AdoptionOutcome`（`:52-65`）、エラー code（`:118-190`・`:385-530`）、
  handler `execution.rs:265-292`。
- §3.125.5 `GET /metrics/execution` — **修正（小）**。`accounts_now` は `#[serde(default)]` だけで常に出る
  （`types.rs:2028-2030`）ので「省略可能」を「schema 上は省略可能だが常に出る」に直した。`since` は
  skip_if_none（`types.rs:2022-2023`）。`group_by` の値と 400 は一致（`crates/task-api/src/stats.rs:300-301`、
  `execution.rs:473-495`）、`RollupMetrics` の欄も一致（`crates/task-core/src/tree_metrics.rs:29-71`）。
- §3.125.6 accept / retry — 一致。`ReopenBody`（`types.rs:100-105`）、`RetryBody` の `accept` 既定 true
  （`types.rs:225-243`）、201 + `Location`（`crates/task-api/src/handlers/task_actions.rs:146-184`）。
- §3.125.7 `POST .../execution/decompose` — 一致。`DecomposeRequest` / `DecomposeResult`
  （`crates/task-ops/src/regate.rs:31-54`）、handler `execution.rs:445-471`。
- §3.125.8 撤去した入口（410）— 一致。表の 7 入口すべてが `require_admin` の後に `ApiProblem::gone`
  （例 `crates/task-api/src/project_plan.rs:53-62`、`handlers/system.rs:93-99`）、problem の形は `problem.rs:61-65`。
- §3.125.9 pause / resume — 一致。`TaskPauseResult`（`crates/task-ops/src/lifecycle.rs:220-229`）、409
  `invalid_transition`（`:328`・`:342`）。
- §3.125.10 `GET /tasks/{id}/task-tree?root=` — 一致。クエリは `root` だけ（`execution.rs:76-92`）、型は
  `crates/task-ops/src/tree_view.rs:23-171`。
- §3.125.11 phase-gate — 一致。action `continue`/`replan`/`withdraw`（`crates/task-ops/src/phase_gate.rs:85-112`）、
  handler `execution.rs:373-402`。
- §3.125.12 plan-gate — 一致。action `approve`/`replan`/`withdraw`（`crates/task-ops/src/plan_gate.rs:75-104`）、
  handler `execution.rs:408-437`。
- §3.125.13 決定の要求 — 一致。クエリ `open`・`root_id`（`crates/task-api/src/decisions.rs:41-73`）、本文と
  `DecisionOutcome`（`crates/task-ops/src/decision.rs:40-111`）、404/409（`problem.rs:596-601`）。
- §3.125.14 案件の詳細と PATCH — 一致。`include_frozen`（`crates/task-api/src/handlers/projects.rs:125-146`）、
  PATCH の 422（`:239-258`）。
- §3.125.15 `stages_hint` — 一致。`StageHint`（`crates/task-core/src/tree.rs:61-65`）、上限 16 件・120・2,000 文字
  （`crates/task-ops/src/add.rs:536-565`）。

照合は、§3.125.2/3 は自分で、他の小節は読み取り専用の調査 agent で行い、修正した 3 か所の根拠は自分でも
ソースで確かめた。

## 実行した check と結果

- `sh scripts/dev/check-doc-links.sh` → exit 0（`check-doc-links: ok`）
- `git diff --quiet 9a53606d -- crates/ web/ gui/ scripts/ CLAUDE.md .claude/ docs/api/v1/api-v1.schema.json docs/api/v1/event.schema.json docs/protocol/` → exit 0
- docs のみの変更のため `cargo test` / `clippy` は実行していない（コードに差分なし）。

## 未解決事項

- なし。

## 提案

- なし。
