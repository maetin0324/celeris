# celeris HTTP API v1: 実行・計画・再実行

---
tasks: [01M3EDF3JEHRQCG6A2EJDRQMXJ, 01M3JXB3DHVBWKWKPW04DTG6SJ]
---

共通の base path は `/api/v1`。読み取りはトークン不要、変更系は管理トークンが必要（未設定でも 401）。JSON 本文の上限は 1 MiB（超過時は 413）。JSON の正本は [`api-v1.schema.json`](api/v1/api-v1.schema.json)、従来のエンドポイント一覧は [`gui/api.md`](gui/api.md)。以下の型名は同スキーマの `$defs` を指す。

### `GET /tasks/{id}/execution` → 200 `TaskExecutionView`

タスクの実行詳細。`gate` は `ExecutionGateDecision | null`、`phase` は `ExecutionPhase | null`、`plan` は `ExecutionPlanView | null`。`runs` は `RunSummary[]`、`metrics` は `ExecutionMetrics` で必須。`metrics.total_cache_read_tokens` は観測できた cached input tokens の合計で、未観測なら省略する。計画の無い atomic タスクの `plan` は `null`。不明なタスクは 404。クエリは受け付けない。

### `GET /tasks/{id}/execution-plan` → 200 `ExecutionPlanView`

有効な計画と `work_units[]` を返す。`versions[]` は `ExecutionPlanVersionView` の版履歴（`version` 昇順、superseded を含む）。有効な計画が無ければ 404 `execution_plan_not_found`。`ExecutionPlanView` は `id`、`task_id`、`version`、`origin`、`status`、`plan`、`created_at`、`work_units` が必須で、`versions` は既定 `[]`。`plan.schema` は `celeris.execution-plan/1` または `/2`（v2 は `phases` を持つ）。クエリは受け付けない。

### `POST /tasks/{id}/execution-plan` → 201 `ExecutionPlanView`（管理系）

本文は `ExecutionPlanSpec`。`schema`、`rationale`、`work_units` が必須。v2 は `phases` も必要で、`children` は現行では空配列のみ。人が提案した計画として検証・採用し、応答に `work_units` と `versions` を含む。クエリは受け付けない。管理トークンが無ければ 401、計画が無効なら検証エラー。

### `GET /metrics/execution?since=&group_by=` → 200 `ExecutionMetricsSummary`

`since` は RFC 3339 の時刻（省略可）。`group_by` は `gate_mode`（既定）、`genre`、`assignee`、`lane` のいずれか。応答は `group_by`、`total_tasks`、`groups[]` が必須で、`since` と `accounts_now[]` は省略可能。各 group は `ExecutionMetricsGroup`。不正な日時・group_by は 400。

### `POST /tasks/{id}/accept` → 200 `TransitionResult`（管理系）

`draft` を `ready` にする。本文は省略可能な `ReopenBody`（`expected_status` のみ。楽観的競合検出）。既に draft でない場合は状態競合。これは `approve`（人の承認チェック）とは異なる操作。クエリは受け付けない。

### `POST /tasks/{id}/retry` → 201 `RetryResult`（管理系）

`failed` または `cancelled` のタスクを複製し、新しいタスクの `task_id` と `rewired` を返す。本文は省略可能な `RetryBody`。`accept` の既定は **`true`** で、新しいタスクは `ready` で始まる。`false` の場合だけ `draft`。`workspace` を指定すると複製先の作業場所を差し替える。応答の `Location` は新しいタスクの URL。クエリは受け付けない。

### `GET /projects/{id}` → 200 `ProjectDetail`

案件詳細。`project`（`Project`）、`milestones[]`（`MilestoneView`）、`tasks[]`（`ProjectTaskView`）は必須で、`repos[]`（`ProjectRepo`）は既定で空配列。`project_plan` は案件計画がある場合だけ返す `ProjectPlanDagView` で、`nodes[]`（`PlanDagNode`）が必須、`current_version` と `pending`（`PlanDagProposal`）は省略可能。承認済みの版がまだ無ければ `current_version` は省略され、未決の提案があれば `pending` に入る。`project.auto_advance` は `boolean`、既定は `false`。`project.slug` は知識ベースでのこの案件の置き場 `projects/<slug>/`（Phase K-1。作るときに題名 → primary リポジトリの名前 → id の末尾から決まり、案件の間で一意）。管理トークンは不要。不明な案件は 404。クエリは受け付けない。

### `PATCH /projects/{id}` → 200 `Project`（管理系）

本文は `ProjectPatchBody`。`auto_advance?: boolean | null` は、案件計画のマイルストーン Task を依存先 Task の `done` で進めるかを指定する。`true` なら進め、`false`（既定）なら途中目標の `reached` を待つ。省略または `null` は変更しない（`null` だけの本文は変更項目が無いため 422）。同じ本文には `status` と `workspace` も指定できる。`slug?: string` は知識ベースの置き場 `projects/<slug>/` の slug を変える（小文字の `[a-z0-9-]`、1〜64 文字、先頭・末尾・連続の `-` と案件 ID の形は不可 → 422。他の案件が使っていれば 409 `project_slug_in_use`）。**KB のディレクトリは動かさない**（`projects/<旧>/` は人が動かす）。管理トークンが無ければ 401、JSON の構文・型が不正なら 400、空の変更指定や許されない状態変更は 422、不明な案件は 404。クエリは受け付けない。

### `POST /projects/{id}/plan` → 202 `ProjectPlanAccepted`（管理系）

案件の計画タスクを作り、`task_id` を返す。本文は `ProjectPlanBody`（空本文も可）。`mode` は `ProjectPlanMode` の `decompose`（既定）または `milestones`、`milestone_id` と `note` は省略可能。`milestones` は案件全体の DAG を提案するモードで、`milestone_id` を併用できない。管理トークンが無ければ 401、JSON の構文・型が不正なら 400、併用時は 422、不明な案件は 404、計画依頼が進行中なら 409 `project_plan_in_flight`。クエリは受け付けない。

### `POST /projects/{id}/project-plan/{version}/decide` → 202 `ProjectPlanDecided`（管理系）

案件計画の提案を判定する。本文は `ProjectPlanDecideBody` で、`decision`（`ProjectPlanDecisionInput`: `approve` または `reject`）が必須。`note` は `approve` では任意、`reject` では空白以外の文字が必要。応答の `decision`、`plan_task_id`、`milestones[]`（`MilestoneId`）、`tasks[]`（`TaskId`）は必須。`approve` は途中目標を `approved`、Task を `ready` にし、`reject` はそれぞれ `redesigned`、`cancelled` にする。管理トークンが無ければ 401、不正な版番号または JSON は 400、空の reject note は 422、不明な案件・提案は 404、既決または古くなった提案は 409。クエリは受け付けない。

### `POST /tasks/{id}/execution/phase-gate` → 200 `TransitionResult`（管理系）

途中確認中の Task を判定する。本文は `PhaseGateRequest` で、`action`（`PhaseGateAction`: `continue`、`replan`、`withdraw`）が必須。`note` は `continue` では任意の次工程への指示、`replan` では空白以外の文字が必要。応答の `id`、`from`、`to`、`reason` は必須で、`cascaded[]` は既定で空配列。管理トークンが無ければ 401、JSON の構文・型が不正なら 400、空の replan note は 422、不明な Task は 404、Task が `awaiting_human` でなければ 409 `invalid_transition`。クエリは受け付けない。
