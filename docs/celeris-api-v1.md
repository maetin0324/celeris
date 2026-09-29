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

`since` は RFC 3339 の時刻（省略可）。`group_by` は `gate_mode`（既定）、`genre`、`assignee`、`lane`、`depth`（ADR-0079 R4a）のいずれか。応答は `group_by`、`total_tasks`、`groups[]` が必須で、`since` と `accounts_now[]` は省略可能。各 group は `ExecutionMetricsGroup`。不正な日時・group_by は 400。

`group_by=depth`（ADR-0079 D11 / U-R7「深さ別の review run 数と費用」）: `key` は task の層（`"1"` = root と木の無い task、`"2"` = 子、`"3"` = 孫）。各 group にだけ `rollup`（`RollupMetrics`）が付く: その深さの task の**自分の分**の和で、`tasks`、`runs_by_role`（`worker` / `planner` / `reviewer` / `wrap_up`。reviewer を含む）、`runs`（reviewer を除く）、`reviewer_runs`、`reviewer_cost_usd`、`runs_in_flight`、`input_tokens` / `output_tokens` / `tokens`（cache を除く）、`cost_usd`、`cost_usd_complete`、`quota[]`（`QuotaUse`。(source, account, window) ごと）、`first_run_started_at` / `last_run_finished_at` / `wall_ms`（壁時計: 最初の run の開始 → 最後の run の終わり）、`busy_ms`（終わった run の長さの和）、`leaves_total` / `leaves_done`、`child_tasks_total` / `child_tasks_done`、`open_decisions`。run・定価は `runs` の索引、quota は `quota_estimated` のイベントから決定的に数える（LLM は使わない）。他の `group_by` には `rollup` は出ない（互換）。

### `POST /tasks/{id}/accept` → 200 `TransitionResult`（管理系）

`draft` を `ready` にする。本文は省略可能な `ReopenBody`（`expected_status` のみ。楽観的競合検出）。既に draft でない場合は状態競合。これは `approve`（人の承認チェック）とは異なる操作。クエリは受け付けない。

### `POST /tasks/{id}/retry` → 201 `RetryResult`（管理系）

`failed` または `cancelled` のタスクを複製し、新しいタスクの `task_id` と `rewired` を返す。本文は省略可能な `RetryBody`。`accept` の既定は **`true`** で、新しいタスクは `ready` で始まる。`false` の場合だけ `draft`。`workspace` を指定すると複製先の作業場所を差し替える。`execution?: "compound" | "atomic"`（ADR-0072「Phase F6 実装時の決定」P5）は複製先の実行の形の人の明示で、複製先に `execution_hint = {mode, explicit: true}` と `execution_hint_set` イベント（`source: "human"`）を残す。省略時は元の `execution_hint` を引き継ぐ。**どちらの場合も元の gate の判定（`routing.execution`）は引き継がず**、複製先の最初の dispatch で今の `[execution] gate` の設定で判定し直し、複製先自身の `execution_gated` を残す。gate の対象外のタスクに `execution` を書くと、複製せずに 422。応答の `Location` は新しいタスクの URL。クエリは受け付けない。

### `POST /tasks/{id}/execution/decompose` → 200 `DecomposeResult`（管理系）

起票済みのタスクの実行の形を人が決め直す（ADR-0072「Phase F6 実装時の決定」P1）。本文は `DecomposeRequest`: `mode`（`ExecutionMode`: `compound` = 計画を作らせる / `atomic` = 直接実行）が必須、`note` は任意（2,000 文字まで）。`routing.execution_hint = {mode, explicit: true}` を書き、前の gate の判定（`routing.execution`）を消し、`execution_hint_set` イベント（`source: "human"`、`previous`、`previous_decision`、`note`、`replan`）を残す。次の dispatch で gate が `human/explicit` として判定し直し（新しい `execution_gated`）、`compound` なら planner run が ExecutionPlan を作る（`gate = "shadow"` でも人の明示の compound は採用される。`gate = "off"` では効かない）。計画を既に持つタスクへの `compound` は replan の依頼（`replan: true`。次の dispatch が replan の planner run、`max_replans` の範囲。`note` は planner の「起こした理由」）。応答は `task`（`Task`）、`mode`、`replan` が必須、`previous_decision`（`ExecutionGateDecision`）は消した判定があるときだけ。受け付ける状態は `draft` / `ready` / `blocked`（`blocked` は `ready` に戻った次の dispatch から効く）。管理トークンが無ければ 401、JSON の構文・型が不正（知らない `mode` を含む）なら 400、gate の対象外（`kind != execute`・対話・support-task・`routing` の無い旧タスク・固定パイプラインの harness・`workspace_mode = shared`）と長すぎる `note` は 422、不明なタスクは 404、`running` / `reviewing`（走っている run は止めない）・終端（`retry` の `execution` を使う）・計画を持つタスクの `atomic` は 409 `invalid_transition`。クエリは受け付けない。

### `GET /tasks/{id}/task-tree?root=` → 200 `TaskTreeView`

ADR-0079 D11（Phase R4a）: 再帰的な task の木と roll-up（読み取り。トークン不要）。ADR の `GET /tasks/{id}/tree` は ADR-0043 D6 の作業ツリーの閲覧（`TreeView`）が既に使っているため、パスは `task-tree`（ADR-0079 付記「R4a 実装時の逸脱・明確化」）。既定は問い合わせた task を根にした subtree、`root=true` なら木の root から。応答は `root_id`（木の root）、`subtree_root`（この view の根）、`tree_enabled`（`[execution.tree] enabled`）、`nodes[]`（`TaskTreeNode`。前順 = 親が子より先、先頭が view の根）、`totals`（view の根の subtree の合計。`nodes[0].subtree` と同じ）が必須で、`limits`（`TreeLimitsUsage`: `leaves` / `max_leaves`、`runs` / `max_runs`〈reviewer を除く〉、`replans` / `max_replans`、`tokens` / `max_tokens?`、`open_decisions` / `max_open_decisions`。`max_*` は `raise-once` / `replan` の回答の余裕を当てた値）は view の根が木の root のときだけ。各節点は `id`、`title`、`status`、`phase?`（`TreeNodePhase`: `planning` / `executing` / `repairing` / `verifying` / `awaiting_human` / `awaiting_children` / `awaiting_plan_approval` / `held_on_decision`〈節点の `self` の決定、または答えを待つ `blocked(decision)` の unit〉/ `blocked_infra`〈子の基盤の失敗の unit〉。終端・待ちの無い task は省略）、`depth`（root = 1）、`parent_id?`（view の根では省略）、`parent_unit_key?` / `parent_stage?`（この節点を作った親の unit）、`plan_version?`、`open_decisions`（この節点が出した未回答の決定）、`children[]`（作られた順）、`units[]`（`TreeUnitView`: `key`、`stage?`、`kind`、`title`、`status`、`blocked_reason?`、`child_task_id?`。統合 WU と superseded を含む履歴）、`own`（自分の分）、`subtree`（自分と子孫の合計）。`own` / `subtree` は `RollupMetrics`（上の `group_by=depth` と同じ形）で、件数・トークン・定価・quota は和、`cost_usd_complete` は論理積、壁時計は最小の開始と最大の終わりなので、root の `subtree` は各節点の `own` の和と一致する。木の無い task（`[execution.tree] enabled = false` の旧い task を含む）は 1 節点（深さ 1）の木。不明な task は 404 `task_not_found`、知らないクエリ・真偽値でない `root` は 400。`GET /tasks/{id}/execution` の `metrics` は自分の分のまま（互換）。

### `GET /projects/{id}` → 200 `ProjectDetail`

案件詳細。`project`（`Project`）、`milestones[]`（`MilestoneView`）、`tasks[]`（`ProjectTaskView`）は必須で、`repos[]`（`ProjectRepo`）は既定で空配列。`project_plan` は案件計画がある場合だけ返す `ProjectPlanDagView` で、`nodes[]`（`PlanDagNode`）が必須、`current_version` と `pending`（`PlanDagProposal`）は省略可能。承認済みの版がまだ無ければ `current_version` は省略され、未決の提案があれば `pending` に入る。`root_totals`（`ProjectRootTotals`。ADR-0079 D11 / Phase R4a）は案件の root task（`parent_id` が無く、木の子でも対話でも裏方でもない task）の `root_tasks`（数）、`by_status`（状態ごとの数。0 件の状態は出ない）、`totals`（root task ごとの subtree の roll-up の和。`RollupMetrics`。run・reviewer の run・トークン・定価・leaf・未回答の決定・壁時計。**quota は数えない**〈events を読まない。quota は `task-tree` と `metrics/execution` で見る〉）。範囲は `tasks[]` と同じ上限（2,000 件）。`project.auto_advance` は `boolean`、既定は `false`。`project.slug` は知識ベースでのこの案件の置き場 `projects/<slug>/`（Phase K-1。作るときに題名 → primary リポジトリの名前 → id の末尾から決まり、案件の間で一意）。管理トークンは不要。不明な案件は 404。クエリは受け付けない。

### `PATCH /projects/{id}` → 200 `Project`（管理系）

本文は `ProjectPatchBody`。`auto_advance?: boolean | null` は、案件計画のマイルストーン Task を依存先 Task の `done` で進めるかを指定する。`true` なら進め、`false`（既定）なら途中目標の `reached` を待つ。省略または `null` は変更しない（`null` だけの本文は変更項目が無いため 422）。同じ本文には `status` と `workspace` も指定できる。`slug?: string` は知識ベースの置き場 `projects/<slug>/` の slug を変える（小文字の `[a-z0-9-]`、1〜64 文字、先頭・末尾・連続の `-` と案件 ID の形は不可 → 422。他の案件が使っていれば 409 `project_slug_in_use`）。**KB のディレクトリは動かさない**（`projects/<旧>/` は人が動かす）。`title?: string` は案件の名前、`request?: string` は案件の説明（依頼文。GUI の「依頼文」）を変える（ADR-0072「Phase F6 実装時の決定」P3。前後の空白を除いて保存し、空は 422、`title` は 200 文字・`request` は 20,000 文字まで）。値が変わった欄だけを書き、管理系のログに `op = "project_updated"` と変えた欄の名前を残す（案件には events の列が無い）。説明を変えても CoS への再依頼にはならない。管理トークンが無ければ 401、JSON の構文・型が不正なら 400、空の変更指定や許されない状態変更は 422、不明な案件は 404。クエリは受け付けない。

### `POST /projects/{id}/plan` → 202 `ProjectPlanAccepted`（管理系）

案件の計画タスクを作り、`task_id` を返す。本文は `ProjectPlanBody`（空本文も可）。仕事が止まった既存の案件（案件直下に done / failed のタスクがある）にも使える: `mode = "milestones"` は案件計画の版がまだ無ければ初回の提案（`project_plan.pending` → `decide`）、承認済みの版があれば replan になる。`mode` は `ProjectPlanMode` の `decompose`（既定）または `milestones`、`milestone_id` と `note` は省略可能。`milestones` は案件全体の DAG を提案するモードで、`milestone_id` を併用できない。管理トークンが無ければ 401、JSON の構文・型が不正なら 400、併用時は 422、不明な案件は 404、計画依頼が進行中なら 409 `project_plan_in_flight`。クエリは受け付けない。

### `POST /projects/{id}/project-plan/{version}/decide` → 202 `ProjectPlanDecided`（管理系）

案件計画の提案を判定する。本文は `ProjectPlanDecideBody` で、`decision`（`ProjectPlanDecisionInput`: `approve` または `reject`）が必須。`note` は `approve` では任意、`reject` では空白以外の文字が必要。応答の `decision`、`plan_task_id`、`milestones[]`（`MilestoneId`）、`tasks[]`（`TaskId`）は必須。`approve` は途中目標を `approved`、Task を `ready` にし、`reject` はそれぞれ `redesigned`、`cancelled` にする。管理トークンが無ければ 401、不正な版番号または JSON は 400、空の reject note は 422、不明な案件・提案は 404、既決または古くなった提案は 409。クエリは受け付けない。

### `POST /tasks/{id}/execution/phase-gate` → 200 `TransitionResult`（管理系）

途中確認中の Task を判定する。本文は `PhaseGateRequest` で、`action`（`PhaseGateAction`: `continue`、`replan`、`withdraw`）が必須。`note` は `continue` では任意の次工程への指示、`replan` では空白以外の文字が必要。応答の `id`、`from`、`to`、`reason` は必須で、`cascaded[]` は既定で空配列。管理トークンが無ければ 401、JSON の構文・型が不正なら 400、空の replan note は 422、不明な Task は 404、Task が `awaiting_human` でなければ 409 `invalid_transition`。クエリは受け付けない。

### `POST /tasks/{id}/execution/plan-gate` → 200 `TransitionResult`（管理系。ADR-0079 D8 / Phase R3b）

root の /3 の計画が人の承認を待っている Task（`blocked` で直前の遷移の reason が `awaiting_plan_approval`。`GET /tasks/{id}/execution` の `phase = awaiting_plan_approval`、`plan_approval`: `PlanApprovalView`〈`plan_id`、`reasons`、`summary`〉）を判定する。承認を求めるのは、計画の採用の時点で決定を含む（`decisions:<key>,…`）・`review: human` の段階がある（`review_human:<stage>`）・上限の `approval_near_limit_ratio`（既定 0.8）以上（`near_limit:<設定名>:<値>/<上限>`。段階数・段階あたりの unit・子 task・見込みの leaf〈leaf + 子 task × 4〉・見込みの木の run）のどれかのとき（root の replan の版にも同じ規則。子の計画は求めない）。本文は `PlanGateRequest`: `action`（`PlanGateAction`: `approve` / `replan` / `withdraw`。`decision` は `action` の別名）が必須、`note` は `approve` では任意（次の run に「計画の承認（ADR-0079 D8）」として渡る）、`replan` では空白以外の文字が必要（planner への指示。2,000 文字まで）。`approve` は `PhaseResume{plan_approve}`（reason `plan_approved`）で unit が起き始める（決定への回答は別。答えの無い決定に依存する unit は待つ）、`replan` は reason `plan_replan` と `ExecutionHintSet{replan: true, source: "human (plan-gate)"}` で次の dispatch が replan の planner run（`max_replans` に数える。新しい版にも同じ承認の規則）、`withdraw` は `Cancel`（subtree に連鎖）。応答の `id`、`from`、`to`、`reason` は必須。管理トークンが無ければ 401、JSON の構文・型が不正（知らない `action`）なら 400、空の replan note・長すぎる note は 422、不明な Task は 404、Task が `awaiting_plan_approval` でなければ 409 `invalid_transition`。承認待ちの Task への `POST /tasks/{id}/answer` も 409。クエリは受け付けない。MCP では `task_plan_gate`（scope `tasks:interact`、`by = mcp:<client_id>`）。

`GET /inbox`: 承認待ちの root は `questions` ではなく `attention[]` の `type: "plan_approval"`（`task`〈`actions` に `plan_gate`、`answer` は無い〉、`plan_id`、`plan_version`、`reasons`、`summary`、`stages[]`〈`key`・`title`・`review_human`・`units[]`〉、`decision_ids[]`〈同じ節点の未回答の決定。`decisions[]` の節にも出る〉、`at`）に出る（`counts.attention` に数える）。通知は `plan_approval`（key `plan:<plan_id>:approval`、その計画の決定を 1 通に束ねる。`decision_requested` の `plan:<plan_id>:decisions` は鳴らさない）。承認の要らない root の計画は報告の流れ（`GET /reports`）に `kind: progress` の「計画を採用して進めます: <段階の一覧>」を 1 件残すだけで、通知しない。

### 決定の要求（ADR-0079 D7 / Phase R3a）

人への決定の要求（`DecisionRequest`。計画の `decisions`・worker の `result.json` の `decisions`・daemon の `leaf_too_large` / `limit` / `plan_invalid`）の一覧と回答。`[execution.tree] enabled = false`（既定）では決定が作られないので、一覧は空（404 ではない）、回答は 404 になる。効き目（選択肢 → 効き目の表）は ADR-0079 付記「R3a 実装時の逸脱・明確化」。MCP では `decision_list` / `decision_answer`（scope `tasks:interact`、`docs/mcp.md`）。

### `GET /decisions?open=&root_id=` → 200 `DecisionList`

`items[]` は `DecisionView`（`decision`: `DecisionRequest`、`task_id` = 決定を出した節点、`root_id`、`created_at`、`answered_at?`、`effect?` = 回答済みならその効き目 `DecisionEffect`: `resume` / `raise_once` / `replan` / `atomic` / `withdraw`）。`created_at` 昇順。`open=true` は未回答だけ、`false` は回答済み・取り下げ済みだけ、省略は全件。`root_id` で 1 つの木（root task の id）に絞る。不正な `open` / `root_id` は 400。

### `GET /tasks/{id}/decisions?open=` → 200 `DecisionList`

その task の subtree（その task が出した決定と、`path` にその task を含む子孫の決定）。不明な Task は 404 `task_not_found`。

### `POST /decisions/{id}/answer` → 200 `DecisionOutcome`（管理系）

本文は `DecisionAnswerBody`: `option?`（決定の `options[].key` のどれか）、`note?`（2,000 文字まで。依存する仕事の入力に固定の書式で入る）。`kind = choice` の決定だけ `option` を省いて `note` に自由記述で答えられる（記録される `option` は `other`）。1 トランザクションで `DecisionAnswered{by: "human"}`、表の更新、待っていた unit の再評価（`blocked(decision)` → `pending` / `ready`、取り下げなら `cancelled`）、効き目の event（replan の依頼 = `ExecutionHintSet{replan: true}`、atomic の run の `self` への答え = `Answered`）を書く。`needed_before: [self]` の取り下げは続けて節点を中止する。応答は `decision`（回答後の `DecisionView`）、`effect`、`resumed[]`、`cancelled[]`（unit の key）、`replan_requested`、`cancelled_task?`。管理トークンが無ければ 401、JSON が不正なら 400、無い id は 404 `decision_not_found`、`open` でない・決定を出した節点が終端なら 409 `decision_not_open`（`decision_status` を添える）、選択肢の外・daemon の決定で `option` 無し・note が長すぎるなら 422。

### `POST /decisions/{id}/withdraw` → 200 `DecisionOutcome`（管理系）

人が決定を取り下げる（`DecisionWithdrawn`）。本文は省略可能な `DecisionWithdrawBody`（`reason?`）。効き目は選択肢の `withdraw` と同じ（止めていた unit〈`needed_before` の unit・`stage:<key>` の unit・その決定を `needs_decisions` に持つ unit〉と、それに依存する未着手の unit を `cancelled`。`needed_before: [self]` なら節点を中止）。`open` でなければ 409、無い id は 404。

### `POST /decisions/{id}/revise` → 200 `DecisionOutcome`（管理系）

回答済みの `choice` の決定の答えを変える（新しい `DecisionAnswered`。最後の回答が有効）。本文は `DecisionAnswerBody`。これから作られる子・これから走る leaf は新しい答えを読む。既に作られた非終端の子は作り直さず、node のコメント（人を起こさない）で新しい答えを届け、応答の `notified_children[]` に並ぶ。未回答・取り下げ済み・daemon の決定（回答の時点で効き目を当てたもの）は 409。

### `GET /inbox` の `decisions` と `counts.decisions`

受信箱に `decisions[]`（`DecisionInboxItem`: `id`、`key`、`kind`、`task_id`、`root_id`、`path`〈パンくず〉、`question`、`options`、`recommended`、`cost_of_reversal`、`cost_note?`、`needed_before`、`origin`、`created_at`、`age_secs`）と `counts.decisions` が付く。未回答で、決定を出した節点が終端でないものだけ（古い順）。`GET /daemon` の `snapshot.decisions_open` は同じ件数（API が応答を組むときに埋める）。
