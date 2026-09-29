# celeris HTTP API v1: 実行・計画・再実行

---
tasks: [01M3EDF3JEHRQCG6A2EJDRQMXJ, 01M3JXB3DHVBWKWKPW04DTG6SJ]
---

共通の base path は `/api/v1`。読み取りはトークン不要、変更系は管理トークンが必要（未設定でも 401）。JSON 本文の上限は 1 MiB（超過時は 413）。JSON の正本は [`api-v1.schema.json`](api/v1/api-v1.schema.json)、従来のエンドポイント一覧は [`gui/api.md`](gui/api.md)。以下の型名は同スキーマの `$defs` を指す。

### `GET /tasks/{id}/execution` → 200 `TaskExecutionView`

タスクの実行詳細。`gate` は `ExecutionGateDecision | null`、`phase` は `ExecutionPhase | null`、`plan` は `ExecutionPlanView | null`。`runs` は `RunSummary[]`、`metrics` は `ExecutionMetrics` で必須。`metrics.total_cache_read_tokens` は観測できた cached input tokens の合計で、未観測なら省略する。計画の無い atomic タスクの `plan` は `null`。不明なタスクは 404。クエリは受け付けない。

### `GET /tasks/{id}/execution-plan` → 200 `ExecutionPlanView`

有効な計画と `work_units[]` を返す。`versions[]` は `ExecutionPlanVersionView` の版履歴（`version` 昇順、superseded を含む）。有効な計画が無ければ 404 `execution_plan_not_found`。`ExecutionPlanView` は `id`、`task_id`、`version`、`origin`、`status`、`plan`、`created_at`、`work_units` が必須で、`versions` は既定 `[]`。`plan.schema` は `celeris.execution-plan/1` または `/2`（v2 は `phases` を持つ）。クエリは受け付けない。

### `POST /tasks/{id}/execution-plan`・`PUT /tasks/{id}/execution-plan` → 201 `ExecutionPlanView`（管理系）

本文は `ExecutionPlanSpec`。`schema`、`rationale`、`work_units` が必須。v2 は `phases` も必要で、`children` は現行では空配列のみ。人が提案した計画（origin `human`）として検証・採用し、応答に `work_units` と `versions` を含む。`POST` は新規だけで、既に有効な計画があれば 409。`PUT` は有効な計画が無ければ `POST` と同じ（201）、**有効な計画があれば人の replan**（200。ADR-0079 付記「R5b-fix1」）: 本文は新しい版の計画の**全体**（差分の形は受け付けない）で、`task_ops::execution::replan`（origin human）を通す。done の WU は同じ key・`kind`・`phase`（/3 は段階）・`depends_on` で残す必要があり（消す・構造を変えると 422）、spec のほかの欄（`checks` など）は上書きできる（状態は `done` のまま、`work_unit_spec_overridden` の event）。応答には `replan`（`added` / `changed` / `removed` / `overridden_done`）が付く。クエリは受け付けない。管理トークンが無ければ 401、計画が無効なら 422。

`celeris.execution-plan/3`（ADR-0079 D2。`stages` / `units` / `decisions`）は daemon と同じ実効の上限（`[execution.tree]`）で検証する。`[execution.tree] enabled = false`（既定）なら 422（本文に `[execution.tree] enabled = true` を案内する `TreeDisabled`）。有効なら planner の計画と同じ経路を 1 トランザクションで通す（ADR-0079 付記「R5b-prep 実装時の逸脱・明確化」）:

- unit の gate（D4 (3)）: leaf ↔ kind task の上げ下げを採用する spec に当て、食い違いは `unit_gate_overridden` に残す（`adopt` の unit は構造上の理由で `kept_task`）。
- 採用の直後の止め: 計画の決定（計画と unit の `decisions`）は決定の要求（origin `human`、`raised_by.run_id` なし、path は root から）になり、`GET /decisions`・受信箱・Discord（人の計画の版ごとに `plan:<plan_id>:decisions` の 1 通）に出る。答えの無い決定を待つ leaf は `blocked(decision)`、kind task の unit は子を作らずに待つ。木の上限（`max_tree_leaves` など）を超える unit は `kind: limit` の決定で止まる。計画の上限（段階の数・段階あたりの unit・子 task・`max_depth`）の違反は人の計画では 422（planner の最後の試行のように緩めて採用しない）。**`adopt` の unit は `max_child_tasks_per_plan` に数えない**（子を作らない）。
- kind task の unit の `adopt: <task_id>`（D15、人の計画だけ）は同じトランザクションで結ぶ（下の `POST /tasks/{id}/tree/adopt` と同じ条件）。対象が `done` / `failed` なら unit は `done`（`child_adopted`）、まだ終端でなければ unit は結ばれずに待つ（後で `tree/adopt`）。条件に合わない unit があれば計画全体を 409 / 422 で拒否し、何も書かない（`code` は `adopt_*`）。
- root の計画の承認（D8 の `PlanGate`）は挟まない（書いた人の承認とみなす）。報告の流れに「計画を採用して進めます: …」を 1 件残し、承認が要る形（決定・`review: human`・上限に近い）だったなら理由も本文に書く。部をまたぐ子の認可の質問（ADR-0074 F4b）も出さない。

応答（`PUT` / `POST` のときだけ）には `adoptions[]`（`AdoptionOutcome`: `plan_id`、`unit_key`、`stage`、`task_id`、`adopted`、`task_status`、`unit_status`、`detail`。`adopt` の unit があるときだけ）と `decisions_raised`（出した計画の決定の数。0 なら省略）が付く。`GET` では出ない。`celerisctl execution plan set|put <task> --file <json> [--config <config.toml>]`（`--config` 省略時は `CELERIS_CONFIG`。どちらも無ければ木は無効）は同じ関数を呼ぶ。有効な計画の人の replan は `celerisctl execution plan replan <task> --file <json> [--reason <text>] [--config <config.toml>]`（`set|put` は新規だけ）。

### `POST /tasks/{id}/tree/adopt` → 200 `AdoptionOutcome`（管理系。ADR-0079 D15 / Phase R5b-prep）

採用済みの /3 の計画の kind task の unit に、既存の task を木の子として後から結ぶ（人の計画の `adopt` の対象がその時点で終端でなかったとき）。本文は `AdoptRequest`: `task_id`（採用する task）、`stage`（unit の段階）、`unit_key`（unit の key）がすべて必須、知らない欄は拒否。条件: `[execution.tree] enabled`（無ければ 422 `tree_disabled`）、`{id}` が有効な /3 の計画を持つ（422 `adopt_no_tree_plan`）、unit があり（422 `adopt_unit_not_found`）kind task で（422 `adopt_unit_not_task`）同じ段階で（422 `adopt_stage_mismatch`）`adopt` にこの `task_id` が書かれている（422 `adopt_id_mismatch`）、対象は同じ案件（422 `adopt_other_project`）、`{id}` 自身でも祖先でもない（422 `adopt_ancestor`）、execute の仕事の task（対話・裏方でない。422 `adopt_target_kind`）、他の木に属さず自分も木の root でない（409 `adopt_target_in_tree`）、`done` か `failed`（`cancelled` は 409 `adopt_target_cancelled`、終端でなければ 409 `adopt_target_not_terminal`）、unit の行が `pending` / `ready` で子を持たない（409 `adopt_unit_not_open`）、`{id}` が終端でない（409 `adopt_owner_terminal`）。結ぶと 1 トランザクションで unit を `done`（`child_task_id` = 対象、`work_unit_transitioned{reason: child_adopted}` と `child_adopted{plan_id, unit_key, stage, child_task_id}` を `{id}` に）、依存が満たされた unit を `ready` に、対象の `tree` = `{root_id, depth, parent_unit}`（`base_commit` なし）と、`parent_id` が無ければ `{id}`（あれば書き換えない）を書き、対象に `edited{fields: ["tree", ("parent_id")], by: "human"}` を積む。対象の状態・履歴・ブランチ・作業場所は変えない。段階の統合は対象のブランチ `celeris/<task_id>` を任意の項目として扱い、既に main か親のブランチに入っていれば `skipped`（R5b の Phase 1 / 2 はこれ）、無ければ飛ばす。競合（同時の変更）は 409 `adopt_conflict`、無い task は 404、トークン無しは 401。`celerisctl tree adopt <root> --task <id> --stage <s> --unit <key> [--config <config.toml>]` も同じ。MCP には出していない。

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

ADR-0079 D11（Phase R4a）: 再帰的な task の木と roll-up（読み取り。トークン不要）。ADR の `GET /tasks/{id}/tree` は ADR-0043 D6 の作業ツリーの閲覧（`TreeView`）が既に使っているため、パスは `task-tree`（ADR-0079 付記「R4a 実装時の逸脱・明確化」）。既定は問い合わせた task を根にした subtree、`root=true` なら木の root から。応答は `root_id`（木の root）、`subtree_root`（この view の根）、`tree_enabled`（`[execution.tree] enabled`）、`nodes[]`（`TaskTreeNode`。前順 = 親が子より先、先頭が view の根）、`totals`（view の根の subtree の合計。`nodes[0].subtree` と同じ）が必須で、`limits`（`TreeLimitsUsage`: `leaves` / `max_leaves`、`runs` / `max_runs`〈reviewer を除く〉、`replans` / `max_replans`、`tokens` / `max_tokens?`、`open_decisions` / `max_open_decisions`。`max_*` は `raise-once` / `replan` の回答の余裕を当てた値）は view の根が木の root のときだけ。各節点は `id`、`title`、`status`、`phase?`（`TreeNodePhase`: `planning` / `executing` / `repairing` / `verifying` / `awaiting_human` / `awaiting_children` / `awaiting_plan_approval` / `held_on_decision`〈節点の `self` の決定、または答えを待つ `blocked(decision)` の unit〉/ `blocked_infra`〈子の基盤の失敗の unit〉。終端・待ちの無い task は省略）、`depth`（root = 1）、`parent_id?`（view の根では省略）、`parent_unit_key?` / `parent_stage?`（この節点を作った親の unit）、`plan_version?`、`open_decisions`（この節点が出した未回答の決定）、`stall?`（Phase R4b。`TreeNodeStall`: `reason`、`since?`、`detail`。節点の最後の event が `StallDetected` で終端でないときだけ = D10 の「理由なく止まっています」。何か event が積まれれば消える）、`children[]`（作られた順）、`units[]`（`TreeUnitView`: `key`、`stage?`、`kind`、`title`、`status`、`blocked_reason?`、`child_task_id?`。統合 WU と superseded を含む履歴）、`own`（自分の分）、`subtree`（自分と子孫の合計）。`own` / `subtree` は `RollupMetrics`（上の `group_by=depth` と同じ形）で、件数・トークン・定価・quota は和、`cost_usd_complete` は論理積、壁時計は最小の開始と最大の終わりなので、root の `subtree` は各節点の `own` の和と一致する。木の無い task（`[execution.tree] enabled = false` の旧い task を含む）は 1 節点（深さ 1）の木。不明な task は 404 `task_not_found`、知らないクエリ・真偽値でない `root` は 400。`GET /tasks/{id}/execution` の `metrics` は自分の分のまま（互換）。

### `GET /projects/{id}?include_frozen=` → 200 `ProjectDetail`

案件詳細。`project`（`Project`）、`milestones[]`（`MilestoneView`）、`tasks[]`（`ProjectTaskView`）は必須で、`repos[]`（`ProjectRepo`）は既定で空配列。

**ADR-0079 D13 / U-R8（Phase R5a）: 途中目標は凍結した履歴**。既定（`include_frozen` 省略・`false`）では `milestones` は空配列で、`project_plan` も出ない。`milestones_frozen`（`u32`、既定 0）はこの案件の途中目標の行の数（隠していても数える。GUI の「以前の途中目標 N 件」用）。`?include_frozen=true` のときだけ全行を読み取り専用で返し（`MilestoneView`: 秘書のレビューの返事と提案を添えたもの）、案件計画の版があれば `project_plan`（`ProjectPlanDagView`: `nodes[]`〈`PlanDagNode`〉が必須、`current_version` と `pending`〈`PlanDagProposal`〉は省略可能）も返す。行は消さず状態も変えない（書き込みの入口は下の 410）。真偽値でない `include_frozen` と知らないクエリは 400。

`tasks[]` の各行には `is_root_task`（`boolean`、既定 `false`。`task_core::is_root_task`: 案件直下〈`parent_id` なし〉・木の子〈`tree.parent_unit`〉でない・対話でも裏方〈`support_kind`〉でもない）が付く。案件ページの root task の一覧はこれで絞る。`root_totals`（`ProjectRootTotals`。ADR-0079 D11 / Phase R4a）は同じ述語の root task の `root_tasks`（数）、`by_status`（状態ごとの数。0 件の状態は出ない）、`totals`（root task ごとの subtree の roll-up の和。`RollupMetrics`。run・reviewer の run・トークン・定価・leaf・未回答の決定・壁時計。**quota は数えない**〈events を読まない。quota は `task-tree` と `metrics/execution` で見る〉）。範囲は `tasks[]` と同じ上限（2,000 件）。`project.auto_advance` は `boolean` で常に読める（R5a からは書けず、読まない列）。`project.slug` は知識ベースでのこの案件の置き場 `projects/<slug>/`（Phase K-1。作るときに題名 → primary リポジトリの名前 → id の末尾から決まり、案件の間で一意）。管理トークンは不要。不明な案件は 404。

### `PATCH /projects/{id}` → 200 `Project`（管理系）

本文は `ProjectPatchBody`。`auto_advance` は **ADR-0079 D13（Phase R5a）で廃止**: 値が `true` でも `false` でも（他の欄と一緒でも）422 `validation`（`field: "auto_advance"`。列 `projects.auto_advance` は残すが書かない・読まない）。同じ本文には `status` と `workspace` も指定できる。`slug?: string` は知識ベースの置き場 `projects/<slug>/` の slug を変える（小文字の `[a-z0-9-]`、1〜64 文字、先頭・末尾・連続の `-` と案件 ID の形は不可 → 422。他の案件が使っていれば 409 `project_slug_in_use`）。**KB のディレクトリは動かさない**（`projects/<旧>/` は人が動かす）。`title?: string` は案件の名前、`request?: string` は案件の説明（依頼文。GUI の「依頼文」）を変える（ADR-0072「Phase F6 実装時の決定」P3。前後の空白を除いて保存し、空は 422、`title` は 200 文字・`request` は 20,000 文字まで）。値が変わった欄だけを書き、管理系のログに `op = "project_updated"` と変えた欄の名前を残す（案件には events の列が無い）。説明を変えても CoS への再依頼にはならない。管理トークンが無ければ 401、JSON の構文・型が不正なら 400、空の変更指定や許されない状態変更は 422、不明な案件は 404。クエリは受け付けない。

### 撤去した入口 → 410 `removed_by_adr_0079`（ADR-0079 D13 / U-R6、Phase R5a）

案件は計画を持たず、途中目標は root task の段階で表す（既存の途中目標の行は凍結）。次の入口は**本文も id も読まずに** 410 Gone を返す（管理系のまま: トークンが無ければ先に 401）。problem は `type: "urn:celeris:problem:removed_by_adr_0079"`、`code: "removed_by_adr_0079"`、`detail`（例 `ADR-0079: 案件は計画を持たない。root task を作る`）に、`adr: "ADR-0079"` と `instead`（代わりの入口の短い説明）を添える。要求・応答の型（`ProjectPlanBody` / `ProjectPlanDecided` / `MilestoneCreateBody` / `MilestonePatchBody` / `MilestoneDecideBody` / `MilestoneLifecycle` / `NewPlanSpec` など）は `api-v1.schema.json` の互換のためにだけ残す。

| 入口 | 以前 | 代わり |
|---|---|---|
| `POST /projects/{id}/plan`（`mode: decompose` / `milestones` とも） | 202 `ProjectPlanAccepted`（案件の分解 task / 案件計画 run） | `POST /tasks` に `project_id`（root task）。段階の名指しは `stages_hint` |
| `POST /projects/{id}/project-plan/{version}/decide` | 202 `ProjectPlanDecided` | 木の中の決定は `POST /decisions/{id}/answer`、root 計画の承認は `POST /tasks/{id}/execution/plan-gate` |
| `POST /projects/{id}/milestones` | 201 `Milestone` | root task の段階（`stages_hint`、計画の `review: human`） |
| `PATCH /milestones/{id}` | 200 `Milestone` | 同上 |
| `POST /milestones/{id}/decide` | 202 `MilestoneDecided`（ADR-0038 の判定） | 段階の `review: human`（`POST /tasks/{id}/execution/phase-gate`） |
| `POST /milestones/{id}/{cancel,pause,resume}` | 200 `MilestoneLifecycle` | `POST /tasks/{id}/{cancel,pause,resume}`（subtree）・`POST /projects/{id}/{cancel,pause,resume}` |
| `POST /plans`（ADR-0028 の Plan kind） | 201 `Task`（`kind = plan`） | `POST /tasks`（root task。分解は gate と planner） |

既存の `kind = plan` の行とその子、途中目標の行、`tasks.milestone_id` はそのまま読める（`GET /projects/{id}?include_frozen=true`、`GET /tasks?milestone=`）。`celerisctl projects plan approve|reject` は削除した。

### `POST /tasks` の `stages_hint`（ADR-0079 D12、Phase R5a）

`NewTaskSpec.stages_hint?: StageHint[]`（`{title: string, scope?: string}`。未知の欄は 400）。人（API・CLI）と CoS（`create_task.stages_hint`）が名指しした段階の名前と範囲で、そのまま `Task.routing.stages_hint` に入り、root の planner への入力になる（構造の強制ではない。子は継がない）。16 件まで、`title` は空白以外の 1〜120 文字、`scope` は 2,000 文字まで（違反は 422）。省略時は空で、`routing` の JSON にも出ない。

### `POST /tasks/{id}/pause` / `POST /tasks/{id}/resume` → 200 `TaskPauseResult`（管理系。ADR-0079 D13、Phase R5a）

task の **subtree の一時停止**。本文は `{}` か空（未知の欄は 400）。`pause` は task に `paused_at` を入れ `Event::Edited{fields: ["paused_at"], by: "human"}` を残す（状態機械は触らない。replay の状態・attempts は変わらない）。以後、その task と子孫（`parent_id` の鎖と、採用で `parent_id` を書き換えない木の子〈`tree.parent_unit`〉）は `ready_tasks` に返らず dispatch されない（一時停止の後に作られた子も止まる）。**走っている run は終わるまで走る**（案件の一時停止と同じ意味）: その後 `ready` に戻っても起きず、`running` の task の並列 WU の 2 本目以降も起きない。最終レビュー（`reviewing`）と人の操作（回答・承認・中止）は止めない。`resume` は `paused_at` を消す（祖先がまだ一時停止中なら子孫は止まったまま）。応答は `task`（`TaskRef`）、`paused_at?`（RFC 3339。`resume` の後は省略）、`subtree[]`（非終端の子孫の `TaskRef`。自分は含まない）。終端の task・既に一時停止中・対話 task の `pause` と、一時停止中でない task の `resume` は 409 `invalid_transition`、不明な task は 404、トークンが無ければ 401。

案件の `POST /projects/{id}/pause|cancel` も root task の subtree に効く: 案件が paused / cancelled / archived なら、`project_id` を持たない子孫も祖先の案件で止まる（`ready_tasks` と並列 WU の判定）。案件の中止は属する task を `project_cancelled` で中止し、木の子へは `parent_cancelled` で連鎖する（ADR-0079 R1b）。

`TaskSummary`（`GET /tasks` の `items[]`）には `is_root_task` と `paused`（この task 自身の `paused_at` の有無）、`TaskDetail`（`GET /tasks/{id}`）には `is_root_task` と `paused_by?`（dispatch を止めている task: 自分か `paused_at` を持つ一番近い祖先）が付く。`Task.paused_at` は `Task` の JSON にも出る（無ければ省略）。

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
