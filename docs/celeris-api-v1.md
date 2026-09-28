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

`failed` または `cancelled` のタスクを複製し、新しいタスクの `task_id` と `rewired` を返す。本文は省略可能な `RetryBody`。`accept` の既定は **`true`** で、新しいタスクは `ready` で始まる。`false` の場合だけ `draft`。`workspace` を指定すると複製先の作業場所を差し替える。`execution?: "compound" | "atomic"`（ADR-0072「Phase F6 実装時の決定」P5）は複製先の実行の形の人の明示で、複製先に `execution_hint = {mode, explicit: true}` と `execution_hint_set` イベント（`source: "human"`）を残す。省略時は元の `execution_hint` を引き継ぐ。**どちらの場合も元の gate の判定（`routing.execution`）は引き継がず**、複製先の最初の dispatch で今の `[execution] gate` の設定で判定し直し、複製先自身の `execution_gated` を残す。gate の対象外のタスクに `execution` を書くと、複製せずに 422。応答の `Location` は新しいタスクの URL。クエリは受け付けない。

### `POST /tasks/{id}/execution/decompose` → 200 `DecomposeResult`（管理系）

起票済みのタスクの実行の形を人が決め直す（ADR-0072「Phase F6 実装時の決定」P1）。本文は `DecomposeRequest`: `mode`（`ExecutionMode`: `compound` = 計画を作らせる / `atomic` = 直接実行）が必須、`note` は任意（2,000 文字まで）。`routing.execution_hint = {mode, explicit: true}` を書き、前の gate の判定（`routing.execution`）を消し、`execution_hint_set` イベント（`source: "human"`、`previous`、`previous_decision`、`note`、`replan`）を残す。次の dispatch で gate が `human/explicit` として判定し直し（新しい `execution_gated`）、`compound` なら planner run が ExecutionPlan を作る（`gate = "shadow"` でも人の明示の compound は採用される。`gate = "off"` では効かない）。計画を既に持つタスクへの `compound` は replan の依頼（`replan: true`。次の dispatch が replan の planner run、`max_replans` の範囲。`note` は planner の「起こした理由」）。応答は `task`（`Task`）、`mode`、`replan` が必須、`previous_decision`（`ExecutionGateDecision`）は消した判定があるときだけ。受け付ける状態は `draft` / `ready` / `blocked`（`blocked` は `ready` に戻った次の dispatch から効く）。管理トークンが無ければ 401、JSON の構文・型が不正（知らない `mode` を含む）なら 400、gate の対象外（`kind != execute`・対話・support-task・`routing` の無い旧タスク・固定パイプラインの harness・`workspace_mode = shared`）と長すぎる `note` は 422、不明なタスクは 404、`running` / `reviewing`（走っている run は止めない）・終端（`retry` の `execution` を使う）・計画を持つタスクの `atomic` は 409 `invalid_transition`。クエリは受け付けない。

### `GET /projects/{id}` → 200 `ProjectDetail`

案件詳細。`project`（`Project`）、`milestones[]`（`MilestoneView`）、`tasks[]`（`ProjectTaskView`）は必須で、`repos[]`（`ProjectRepo`）は既定で空配列。`project_plan` は案件計画がある場合だけ返す `ProjectPlanDagView` で、`nodes[]`（`PlanDagNode`）が必須、`current_version` と `pending`（`PlanDagProposal`）は省略可能。承認済みの版がまだ無ければ `current_version` は省略され、未決の提案があれば `pending` に入る。`project.auto_advance` は `boolean`、既定は `false`。`project.slug` は知識ベースでのこの案件の置き場 `projects/<slug>/`（Phase K-1。作るときに題名 → primary リポジトリの名前 → id の末尾から決まり、案件の間で一意）。管理トークンは不要。不明な案件は 404。クエリは受け付けない。

### `PATCH /projects/{id}` → 200 `Project`（管理系）

本文は `ProjectPatchBody`。`auto_advance?: boolean | null` は、案件計画のマイルストーン Task を依存先 Task の `done` で進めるかを指定する。`true` なら進め、`false`（既定）なら途中目標の `reached` を待つ。省略または `null` は変更しない（`null` だけの本文は変更項目が無いため 422）。同じ本文には `status` と `workspace` も指定できる。`slug?: string` は知識ベースの置き場 `projects/<slug>/` の slug を変える（小文字の `[a-z0-9-]`、1〜64 文字、先頭・末尾・連続の `-` と案件 ID の形は不可 → 422。他の案件が使っていれば 409 `project_slug_in_use`）。**KB のディレクトリは動かさない**（`projects/<旧>/` は人が動かす）。`title?: string` は案件の名前、`request?: string` は案件の説明（依頼文。GUI の「依頼文」）を変える（ADR-0072「Phase F6 実装時の決定」P3。前後の空白を除いて保存し、空は 422、`title` は 200 文字・`request` は 20,000 文字まで）。値が変わった欄だけを書き、管理系のログに `op = "project_updated"` と変えた欄の名前を残す（案件には events の列が無い）。説明を変えても CoS への再依頼にはならない。管理トークンが無ければ 401、JSON の構文・型が不正なら 400、空の変更指定や許されない状態変更は 422、不明な案件は 404。クエリは受け付けない。

### `POST /projects/{id}/plan` → 202 `ProjectPlanAccepted`（管理系）

案件の計画タスクを作り、`task_id` を返す。本文は `ProjectPlanBody`（空本文も可）。仕事が止まった既存の案件（案件直下に done / failed のタスクがある）にも使える: `mode = "milestones"` は案件計画の版がまだ無ければ初回の提案（`project_plan.pending` → `decide`）、承認済みの版があれば replan になる。`mode` は `ProjectPlanMode` の `decompose`（既定）または `milestones`、`milestone_id` と `note` は省略可能。`milestones` は案件全体の DAG を提案するモードで、`milestone_id` を併用できない。管理トークンが無ければ 401、JSON の構文・型が不正なら 400、併用時は 422、不明な案件は 404、計画依頼が進行中なら 409 `project_plan_in_flight`。クエリは受け付けない。

### `POST /projects/{id}/project-plan/{version}/decide` → 202 `ProjectPlanDecided`（管理系）

案件計画の提案を判定する。本文は `ProjectPlanDecideBody` で、`decision`（`ProjectPlanDecisionInput`: `approve` または `reject`）が必須。`note` は `approve` では任意、`reject` では空白以外の文字が必要。応答の `decision`、`plan_task_id`、`milestones[]`（`MilestoneId`）、`tasks[]`（`TaskId`）は必須。`approve` は途中目標を `approved`、Task を `ready` にし、`reject` はそれぞれ `redesigned`、`cancelled` にする。管理トークンが無ければ 401、不正な版番号または JSON は 400、空の reject note は 422、不明な案件・提案は 404、既決または古くなった提案は 409。クエリは受け付けない。

### `POST /tasks/{id}/execution/phase-gate` → 200 `TransitionResult`（管理系）

途中確認中の Task を判定する。本文は `PhaseGateRequest` で、`action`（`PhaseGateAction`: `continue`、`replan`、`withdraw`）が必須。`note` は `continue` では任意の次工程への指示、`replan` では空白以外の文字が必要。応答の `id`、`from`、`to`、`reason` は必須で、`cascaded[]` は既定で空配列。管理トークンが無ければ 401、JSON の構文・型が不正なら 400、空の replan note は 422、不明な Task は 404、Task が `awaiting_human` でなければ 409 `invalid_transition`。クエリは受け付けない。
