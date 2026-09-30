use std::time::Duration as StdDuration;

use time::OffsetDateTime;

use crate::comment::TaskComment;
use crate::execution_plan::{
    ExecutionPlanRow, RunIndexStatus, RunRow, WorkUnitRow, WorkUnitStatus,
};
use crate::instance::{DaemonInstance, InstanceRole};
use crate::integrations::{IntegrationId, TaskIntegration};
use crate::message::Message;
use crate::model::{Event, Status, Task, TaskId, WorkspaceSpec};
use crate::org::{
    Milestone, MilestoneId, MilestoneStatus, OrgNode, Project, ProjectId, ProjectStatus,
};
use crate::repos::{ProjectRepo, RepoId};
use crate::transition::{Outcome, Trigger};

use super::query::{ListFilter, ListOrder, Page};
use super::{
    ClusterConnectionRecord, ClusterSettings, EventRow, ExecutionMetricsTaskRow, ProjectPlanApply,
    StoreError, TreeAdoption,
};

/// ADR-0033 D3: 報告（`reports`）の読み書きは `crate::report::ReportStore` にあり、`TaskStore` はそれを
/// supertrait として要求する（ディスパッチャの `Arc<dyn TaskStore>` から報告を追記できるようにするため。
/// 実装は `report.rs` にあり、この表の SQL はここには無い）。
/// ADR-0033 D5（Phase 26）: 認可（`approvals` / `standing_rules`）も同じ形で `crate::approval::ApprovalStore`
/// にある。
/// ADR-0037 D1（Phase 39）: 通知の台帳（`notifications`）も同じ形で `crate::notify::NotificationStore` にある。
pub trait TaskStore:
    Send
    + Sync
    + crate::report::ReportStore
    + crate::approval::ApprovalStore
    + crate::notify::NotificationStore
    + crate::knowledge_run::KnowledgeRunStore
    + crate::delivery::DeliveryStore
    + crate::node_session::NodeSessionStore
    + crate::mcp::McpClientStore
    + crate::mcp::McpCallStore
    + crate::browser_wait::BrowserWaitStore
    + crate::cluster_job::ClusterJobWaitStore
{
    fn insert(&self, task: &Task) -> Result<(), StoreError>;
    fn get(&self, id: TaskId) -> Result<Option<Task>, StoreError>;
    fn list(&self, filter: Option<Status>) -> Result<Vec<Task>, StoreError>;
    /// イベントを追記し、割り当てられた `seq`（0始まり、task_id 内で単調増加）を返す。
    fn append_event(&self, task_id: TaskId, event: &Event) -> Result<u64, StoreError>;
    /// `task_id` に紐づく全イベントを `seq` 昇順で返す。**`seq` はタスクごとに 0 始まりで単調増加する
    /// ローカルな連番**であり、別のタスクの `seq` と大小比較することはできない（Phase 45 実機バグ:
    /// ADR-0021 D3 の「一度扱った失敗は数え直さない」判定で、親の `seq` と子の `seq` を比較していたため、
    /// 子の失敗が毎回「新規」と判定され、同じ質問が繰り返し出た）。タスクをまたいでイベントの前後関係を
    /// 比較したい場合は `events_for_with_global_ids` を使うこと。
    fn events_for(&self, task_id: TaskId) -> Result<Vec<(u64, Event)>, StoreError>;
    /// `task_id` に紐づく全イベントを、`events` テーブルの**グローバルに単調増加する id**（`seq` ではない）
    /// と一緒に id 昇順で返す（Phase 45）。この id は全タスクを横断して単調増加するため、
    /// 異なるタスクのイベント同士の前後関係を比較してよい（`events_for` の `seq` はタスクごとにローカルなので
    /// 比較できない）。
    fn events_for_with_global_ids(&self, task_id: TaskId) -> Result<Vec<(u64, Event)>, StoreError>;
    /// 排他的にリースを取得する。成功したら true を返し、task の status を Running にし、
    /// lease = Some{worker_run_id, expires_at: now + ttl} をDBに書く。
    /// 既にリースされている／status != Ready の場合は false を返す（エラーではない）。
    fn acquire_lease(
        &self,
        task_id: TaskId,
        worker_run_id: &str,
        ttl: StdDuration,
    ) -> Result<bool, StoreError>;
    /// worker_run_id が現在のリースと一致する場合のみ lease を None にする。status は変更しない。
    fn release_lease(&self, task_id: TaskId, worker_run_id: &str) -> Result<(), StoreError>;
    /// status=Ready かつ depends_on が全て Done かつ（親が存在し kind=Approval の場合は親が Done）
    /// を満たすタスクを priority DESC, created_at ASC で最大 limit 件返す。
    fn ready_tasks(&self, limit: usize) -> Result<Vec<Task>, StoreError>;
    /// ADR-0079 D13（Phase R5a）: `task` が subtree の一時停止（自分か祖先の `paused_at`）か、祖先の案件の
    /// 停止（paused / cancelled / archived）で止まっているか。`ready_tasks` と同じ判定を、既に `running` の
    /// task の並列 WU の 2 本目以降（`dispatch_parallel_work_units`）にも当てるために出す。対話は常に `false`。
    fn halted_by_pause(&self, task: &Task) -> Result<bool, StoreError>;
    /// `transition::transition()` で検証した任意のトリガーを適用する汎用の書き込み口
    /// （ADR-0004 D1）。タスクの取得・`transition()` の呼び出し・`tasks` 行の更新・
    /// `Event::Transitioned` の追記（と任意の `extra_event`）を単一トランザクションで行う。
    /// タスクが存在しない場合は `StoreError::Invalid`、遷移が無効な場合は
    /// `StoreError::InvalidTransition` を返し、いずれもタスクの状態は変更しない。
    fn apply_transition(
        &self,
        task_id: TaskId,
        trigger: Trigger,
        extra_event: Option<Event>,
    ) -> Result<Outcome, StoreError> {
        self.apply_transition_with_events(task_id, trigger, extra_event.into_iter().collect())
    }
    /// `apply_transition` の一般形（ADR-0005 D4）。`extra_events` を順に、`Event::Transitioned`
    /// の直後に同一トランザクションで追記する。ディスパッチャが `WorkerFinished` や
    /// 条件ごとの `ReviewVerdict` を遷移と原子的に記録するために使う。
    fn apply_transition_with_events(
        &self,
        task_id: TaskId,
        trigger: Trigger,
        extra_events: Vec<Event>,
    ) -> Result<Outcome, StoreError>;

    /// ADR-0007 D3: Plan の子タスク群を挿入（`Created`、`accept_children` なら続けて `Accept`）し、
    /// 親に `ReviewPass` を適用して `verdict_events` を追記する。全体が 1 トランザクション。
    fn complete_plan(
        &self,
        plan_id: TaskId,
        verdict_events: Vec<Event>,
        children: Vec<Task>,
        accept_children: bool,
    ) -> Result<Outcome, StoreError>;

    /// ADR-0010 D2: `insert` + `Event::Created` + `extra_events` を 1 トランザクションで行う。
    fn create_task(&self, task: &Task, extra_events: Vec<Event>) -> Result<(), StoreError>;

    /// ADR-0074 D3.3（Phase F4a (c)）: 案件計画の提案の決定を **1 トランザクションで**適用する。
    /// `milestones` を全て `milestone_status` にし、`tasks` のうちまだ `draft` のもの（`Trigger::Cancel`
    /// のカスケードで既に終端になったもの等は飛ばす）に `trigger` を適用し、最後に `decided_event`
    /// を `plan_task_id` の events に積む。途中で失敗すれば何も書かない。無い途中目標・Task は
    /// `StoreError::Invalid`。
    fn project_plan_decide_apply(
        &self,
        plan_task_id: TaskId,
        milestones: &[MilestoneId],
        milestone_status: MilestoneStatus,
        tasks: &[TaskId],
        trigger: Trigger,
        decided_event: Event,
    ) -> Result<(), StoreError>;

    /// ADR-0074 D3.4（Phase F4b (e)）: 案件計画の決定（replan の差分を含む）を **1 トランザクションで**
    /// 適用する。順に: 途中目標の状態・題名の更新 → Task の書き換え（`draft` / `ready` で lease の無い
    /// ものだけ。dispatch 済みなら `StoreError::Invalid` で何も書かない）→ 遷移（`Accept` は `draft` の
    /// ときだけ、それ以外は終端でないときだけ適用。`Cancel` のカスケードで先に終端になったものは飛ばす）
    /// → `decided_event` を `plan_task_id` の events に積む。
    fn project_plan_apply(&self, apply: &ProjectPlanApply) -> Result<(), StoreError>;

    /// ADR-0016 D2 / M2: 実行中の委譲。子タスク群を挿入（`Created` → `Accept` で `ready`）し、親に
    /// `Event::Delegated{run_id, task_ids}` を追記する。全体が 1 トランザクション。親の状態は変えない。
    /// 子の `parent_id` が `parent_id` と違えば `StoreError::Invalid`。
    fn delegate_children(
        &self,
        parent_id: TaskId,
        run_id: &str,
        children: Vec<Task>,
    ) -> Result<Vec<TaskId>, StoreError>;

    /// `parent_id` を親に持つタスク（終端を含む）を `created_at` 昇順（同時刻は挿入順）で返す（ADR-0016 M5 / M6）。
    fn children(&self, parent_id: TaskId) -> Result<Vec<Task>, StoreError>;

    /// ADR-0010 D2 / D7（P-7）: `status = running` かつリースの run_id が一致するときだけ `expires_at = now + ttl` に
    /// 延長して true を返す。状態遷移ではないのでイベントは追記しない。
    fn renew_lease(
        &self,
        task_id: TaskId,
        worker_run_id: &str,
        ttl: StdDuration,
    ) -> Result<bool, StoreError>;

    /// ADR-0013 D6: `events` を `id` 昇順で `after_id` より後、最大 `limit` 件返す。
    fn events_since(&self, after_id: u64, limit: usize) -> Result<Vec<EventRow>, StoreError>;
    /// ADR-0013 D6: `events` の現在の最大 `id`。行が無ければ 0。
    fn latest_event_id(&self) -> Result<u64, StoreError>;
    /// ADR-0013（Phase 9b）: 1 タスクのイベントを `seq` 昇順で、`after_seq` より後（`None` なら最初から）最大 `limit` 件、
    /// グローバル `id` と `ts` 付きで返す（API の `GET /tasks/{id}/events` と run の要約が使う）。
    fn event_rows_for(
        &self,
        task_id: TaskId,
        after_seq: Option<u64>,
        limit: usize,
    ) -> Result<Vec<EventRow>, StoreError>;

    /// ADR-0013 D10: `filter` に一致する `tasks` を `order` で keyset ページングして返す。`cursor` は
    /// 前回の `Page::next_cursor`（不透明な文字列）。不正な `cursor` は `StoreError::Invalid`。
    fn list_page(
        &self,
        filter: &ListFilter,
        order: ListOrder,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Page<Task>, StoreError>;
    /// ADR-0013 D10: `tasks` の件数を `status` ごとに集計する（0 件の status は含まない）。
    fn count_by_status(&self) -> Result<Vec<(Status, u64)>, StoreError>;

    // ---- ADR-0033 D1: 組織（`org_nodes`）。DB が正で、設定は空のときの種蒔きにしか使わない ----

    /// 全ノードを `position`、同値なら `id` の昇順で返す（木は呼び出し側が `parent_id` で組む）。
    fn org_list(&self) -> Result<Vec<OrgNode>, StoreError>;
    /// 1 ノード。無ければ `None`。
    fn org_get(&self, id: &str) -> Result<Option<OrgNode>, StoreError>;
    /// 挿入または更新。`crate::org::validate_upsert` を通してから書く（secretary は 1 つ、親は既存、
    /// 自分を祖先にできない、secretary > department > section）。`created_at` は既存行のものを保つ。
    fn org_upsert(&self, node: &OrgNode) -> Result<OrgNode, StoreError>;
    /// 監査 D-4: 設定からの種蒔き専用。`nodes` を渡された順に検証しながら**1 トランザクション**で書く
    /// （後の要素は前の要素を `existing` に含めて検証できるので、`secretary` → `department` → `section`
    /// の順に並んでいれば通る）。途中の 1 件でも `crate::org::validate_upsert` に落ちたら、それより前の
    /// 分も含めて何も書かない（部分的に蒔かれた組織が残ると、次回起動時は `org_list` が空でなくなり
    /// 補完されないため）。
    fn org_seed(&self, nodes: &[OrgNode]) -> Result<(), StoreError>;
    /// 削除。そのノードを `assignee` に持つ未終了タスクがあれば `StoreError::InUse`、
    /// 子ノードがあっても `StoreError::InUse`（木を宙ぶらりんにしない）。無い id は `Ok(false)`。
    fn org_delete(&self, id: &str) -> Result<bool, StoreError>;

    // ---- ADR-0033 D2: 案件（`projects`）と途中目標（`milestones`）----

    /// 案件を作る（`status` は呼び出し側が決める。API は `proposed`）。
    fn project_create(&self, project: &Project) -> Result<(), StoreError>;
    fn project_get(&self, id: ProjectId) -> Result<Option<Project>, StoreError>;
    /// `created_at` の降順（新しい案件が先）。
    fn project_list(&self) -> Result<Vec<Project>, StoreError>;
    /// 状態だけを変える（`updated_at` も更新）。無い案件は `Ok(false)`。
    fn project_set_status(&self, id: ProjectId, status: ProjectStatus) -> Result<bool, StoreError>;
    /// ADR-0039 D1: 作業場所だけを変える（`None` で消す。`updated_at` も更新）。無い案件は `Ok(false)`。
    fn project_set_workspace(
        &self,
        id: ProjectId,
        workspace: Option<&WorkspaceSpec>,
    ) -> Result<bool, StoreError>;
    /// ADR-0044 D6（Phase 55）: 状態と `paused_from` を**同時に**書く（`pause` / `resume` / `cancel`）。
    /// `paused_from` は `Some(None)` で消し、`None` なら触らない。無い案件は `Ok(false)`。
    fn project_set_lifecycle(
        &self,
        id: ProjectId,
        status: ProjectStatus,
        paused_from: Option<Option<ProjectStatus>>,
    ) -> Result<bool, StoreError>;
    /// ADR-0044 D6（Phase 55）: アーカイブの時刻を書く（`None` で解除）。無い案件は `Ok(false)`。
    fn project_set_archived_at(
        &self,
        id: ProjectId,
        at: Option<OffsetDateTime>,
    ) -> Result<bool, StoreError>;
    /// ADR-0044 D7 追記（Phase K-1）: `projects.slug` を書く。綴りが違えば `StoreError::Invalid`
    /// （API は 422）、他の案件が使っていれば `StoreError::InUse`（409）。無い案件は `Ok(false)`。
    /// **知識ベースのディレクトリは動かさない**（`projects/<旧>/` を `projects/<新>/` へ動かすのは人）。
    fn project_set_slug(&self, id: ProjectId, slug: &str) -> Result<bool, StoreError>;
    /// ADR-0072「Phase F6 実装時の決定」P3: 案件の題名（`title`）と説明（依頼文 `request`）を
    /// 人が書き換える。`None` の欄は変えない。検証（空でない・長さの上限）は呼び出し側（API）が行う。
    /// 無い案件は `Ok(false)`。
    fn project_set_text(
        &self,
        id: ProjectId,
        title: Option<&str>,
        request: Option<&str>,
    ) -> Result<bool, StoreError>;

    // ---- ADR-0043 D1（Phase 52）: 案件のリポジトリ（`project_repos`）----

    /// 案件のリポジトリを 1 件作る。`repos::validate_upsert` に落ちれば `StoreError::Repo`（API は 422）。
    /// `is_primary` を立てた行を作ると、同じ案件の他の行の `is_primary` は落ちる（1 案件に 1 つ）。
    fn repo_create(&self, repo: &ProjectRepo) -> Result<(), StoreError>;
    fn repo_get(&self, id: RepoId) -> Result<Option<ProjectRepo>, StoreError>;
    /// その案件のリポジトリ（primary が先、あとは作った順）。
    fn repo_list(&self, project_id: ProjectId) -> Result<Vec<ProjectRepo>, StoreError>;
    /// 既存の 1 件を差し替える（`id` と `project_id` は変えない）。無い id は `Ok(false)`。
    fn repo_update(&self, repo: &ProjectRepo) -> Result<bool, StoreError>;
    /// 消す。未終端のタスクが参照していれば `StoreError::InUse`（API は 409）。無い id は `Ok(false)`。
    fn repo_delete(&self, id: RepoId) -> Result<bool, StoreError>;
    /// その案件の primary をこの行にする。無い id は `Ok(false)`。
    fn repo_set_primary(&self, id: RepoId) -> Result<bool, StoreError>;
    /// そのリポジトリを参照している未終端のタスク（`DELETE` の 409 の理由に使う）。
    fn repo_active_tasks(&self, id: RepoId) -> Result<Vec<TaskId>, StoreError>;

    // ---- ADR-0043 D5（Phase 54）: 変更の取り込み（`task_integrations`）----

    /// 取り込みの記録を 1 件書く（同じ `id` があれば差し替える。`updated_at` は呼び出し側が入れる）。
    /// **人（管理系 API）だけが呼ぶ**。組織の「人」がここに届く経路は無い（SPEC §3.6）。
    fn integration_put(&self, integration: &TaskIntegration) -> Result<(), StoreError>;
    fn integration_get(&self, id: IntegrationId) -> Result<Option<TaskIntegration>, StoreError>;
    /// そのタスクの記録（新しい順）。ADR-0044 B1 の timeline はこれを読めばよい。
    fn integration_list_for_task(
        &self,
        task_id: TaskId,
    ) -> Result<Vec<TaskIntegration>, StoreError>;
    /// そのタスクのそのリポジトリの**最新の** 1 件（`GET /tasks/{id}/changes` の `integration`）。
    fn integration_latest(
        &self,
        task_id: TaskId,
        repo: &str,
    ) -> Result<Option<TaskIntegration>, StoreError>;
    /// 案件のタスクの取り込み（**タスク × リポジトリごとに最新の 1 件**。新しい順、`limit` 件まで）。
    /// 案件画面の「PR と取り込み」（ADR-0043 D5）。
    fn integration_list_for_project(
        &self,
        project_id: ProjectId,
        limit: usize,
    ) -> Result<Vec<TaskIntegration>, StoreError>;

    /// 途中目標を作る。`seq` はその案件の最大 + 1 をストアが採番し、確定した行を返す。
    /// 案件が無ければ `StoreError::Invalid`。
    fn milestone_create(
        &self,
        project_id: ProjectId,
        title: &str,
        description: &str,
        status: MilestoneStatus,
    ) -> Result<Milestone, StoreError>;
    /// ADR-0074 D3.3（Phase F4b）: 案件計画から作った途中目標に key を結ぶ（`milestones.plan_key`）。
    /// 無い途中目標は `Ok(false)`。
    fn milestone_set_plan_key(&self, id: MilestoneId, plan_key: &str) -> Result<bool, StoreError>;
    /// その案件の途中目標を `seq` 昇順で返す。
    fn milestone_list(&self, project_id: ProjectId) -> Result<Vec<Milestone>, StoreError>;
    /// ADR-0044 D6（Phase 55）: 途中目標を id 1 つで引く（案件を知らなくてよい）。
    fn milestone_get(&self, id: MilestoneId) -> Result<Option<Milestone>, StoreError>;
    /// 状態だけを変える。無い途中目標は `Ok(false)`。
    fn milestone_set_status(
        &self,
        id: MilestoneId,
        status: MilestoneStatus,
    ) -> Result<bool, StoreError>;
    /// 現在の状態が `from` のときだけ更新する。dispatch と自動到達が人の状態変更を上書きしないために使う。
    fn milestone_transition_status(
        &self,
        id: MilestoneId,
        from: MilestoneStatus,
        to: MilestoneStatus,
    ) -> Result<bool, StoreError>;
    /// ADR-0044 D6（Phase 55）: 状態と `paused_from` を**同時に**書く（`pause` / `resume` / `cancel`）。
    /// `paused_from` は `Some(None)` で消し、`None` なら触らない。無い途中目標は `Ok(false)`。
    fn milestone_set_lifecycle(
        &self,
        id: MilestoneId,
        status: MilestoneStatus,
        paused_from: Option<Option<MilestoneStatus>>,
    ) -> Result<bool, StoreError>;

    // ---- ADR-0033 D4: 対話（`messages`）----

    /// 1 行を追記する（対話は追記専用。更新も削除もしない）。
    fn message_append(&self, message: &Message) -> Result<(), StoreError>;
    /// `node_id` とのやり取りを**古い順**（`created_at` 昇順、同時刻は `id` 昇順）で最大 `limit` 件返す。
    /// `project_id` が `Some` ならその案件の行だけ、`None` なら案件に紐づかない行だけ（雑談）。
    /// 件数が `limit` を超えるときは**新しい方**を残す（直近のやり取りを渡すため）。
    fn message_list(
        &self,
        node_id: &str,
        project_id: Option<ProjectId>,
        limit: usize,
    ) -> Result<Vec<Message>, StoreError>;

    /// ADR-0048 D1（Phase 60a）: Console の一本の流れ用。`message_list` と違い **絞り込みは任意**で、
    /// `node_id` / `project_id` が `None` なら「その軸では絞らない」（`message_list` の `project_id: None`
    /// は「案件に紐づかない行だけ」なので意味が違う）。
    ///
    /// `after` は RFC 3339 の `created_at`。`Some` なら **その時刻以降（`>=`、閉区間）を古い順**に最大
    /// `limit` 件（同じ時刻に複数行あっても取りこぼさないため閉区間。呼び出し側がカーソルで重複を落とす）。
    /// `None` なら **いちばん新しい `limit` 件**を古い順に返す（Console の初期表示）。
    fn message_page(
        &self,
        node_id: Option<&str>,
        project_id: Option<ProjectId>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Message>, StoreError>;

    // ---- ADR-0048 D3（Phase 60b）: CoS の actions の冪等性 ----

    /// この `run_id` の actions をまだ実行していなければ記録して `true`、既に実行済みなら
    /// 何もせず `false`（`console_action_runs.run_id` は一意。ADR-0048 D3「Idempotent per run」）。
    fn console_action_run_claim(
        &self,
        run_id: &str,
        task_id: TaskId,
        now: OffsetDateTime,
    ) -> Result<bool, StoreError>;

    // ---- Phase 31: 失敗した仕事をやり直す（実機の事故、2026-09-18）----

    /// `original` は `failed` または `cancelled` でなければ `StoreError::InvalidTransition`（`trigger = "retry"`）。
    /// `new_task` を挿入し（`Event::Created` + `Event::Retried{from: original}`)、`original` に依存していた
    /// 「未終端」または「`dependency_failed` で `cancelled` になった」タスクの `depends_on` を `new_task.id` に
    /// 張り替える（後者は `draft` に戻し、`Event::Transitioned{from: cancelled, to: draft, reason: "retried"}`
    /// を追記する）。全体を単一トランザクションで行い、張り替えたタスクの id を返す。
    fn retry_task(&self, original: TaskId, new_task: &Task) -> Result<Vec<TaskId>, StoreError>;

    // ---- ADR-0044 D1/D2（Phase 53）: 人の編集とタスク単位のコメント ----

    /// ADR-0044 D1: 人の編集を書き戻す（`json` と絞り込みの列、`Event::Edited` を単一トランザクションで）。
    /// **状態機械は通らない**: `status` / `attempts` / `lease` は渡された `task` の値を**使わず**、
    /// トランザクションの中で読み直した現在の行の値を書く（編集を組み立てている間にディスパッチャが
    /// リースを取っていても、その run を壊さないため。`acquire_lease` / `release_lease` と同じ規律）。
    /// 書き込んだ後の `Task`（= 読み直した状態を持つもの）を返す。無いタスクは `StoreError::Invalid`。
    fn update_task(&self, task: &Task, event: Event) -> Result<Task, StoreError>;

    /// ADR-0044 D2: コメントを 1 件追記する。`transition` が `Some((trigger, extra_events))` なら
    /// **同じトランザクション**で状態遷移も行い、その結果を返す（人のコメントの割り込みが
    /// 「コメントは残ったが run は止まらなかった」状態にならないようにするため）。
    fn comment_add(
        &self,
        comment: &TaskComment,
        transition: Option<(Trigger, Vec<Event>)>,
    ) -> Result<Option<Outcome>, StoreError>;

    /// ADR-0044 D2: そのタスクのコメントを古い順（`created_at`、同時刻は `id` 昇順）で返す。
    fn comments_for(&self, task_id: TaskId) -> Result<Vec<TaskComment>, StoreError>;

    // ---- ADR-0040 D4（Phase 47）: celeris のインスタンスの役割（`daemon_instances`）----
    //
    // ここにあるのは「行を読み書きする」だけの操作で、役割を決める規則（誰が active になるか、いつ
    // drain するか）は celeris 側（`celeris::instance`）にある。`verify` のインスタンスはこの表に触れない。

    /// 自分の行を作る（既にあれば上書きする＝同じ `instance_id` で起動し直したとき）。
    /// `handoff_requested_at` / `drained_at` は NULL に戻る。
    fn instance_register(&self, instance: &DaemonInstance) -> Result<(), StoreError>;
    /// 自分の行の `heartbeat_at` を更新する。行が無ければ `Ok(false)`（呼び出し側は登録し直す）。
    fn instance_heartbeat(&self, instance_id: &str, at: OffsetDateTime)
    -> Result<bool, StoreError>;
    /// `instance_id` の行に `handoff_requested_at` を書く（既に入っていれば**上書きしない**。
    /// 引き継ぎの要求は 1 回だけ）。書いたら `Ok(true)`。
    fn instance_request_handoff(
        &self,
        instance_id: &str,
        at: OffsetDateTime,
    ) -> Result<bool, StoreError>;
    /// 役割を変える（`heartbeat_at` も同時に更新する）。行が無ければ `Ok(false)`。
    fn instance_set_role(
        &self,
        instance_id: &str,
        role: InstanceRole,
        at: OffsetDateTime,
    ) -> Result<bool, StoreError>;
    /// 手元の run が 0 になったので `drained_at` を書く（役割は `draining` のまま）。行が無ければ `Ok(false)`。
    fn instance_mark_drained(
        &self,
        instance_id: &str,
        at: OffsetDateTime,
    ) -> Result<bool, StoreError>;
    /// 全インスタンスを `started_at` 昇順（同時刻は `instance_id` 昇順）で返す。
    fn instance_list(&self) -> Result<Vec<DaemonInstance>, StoreError>;
    /// 1 行消す。無い id は `Ok(false)`。
    fn instance_delete(&self, instance_id: &str) -> Result<bool, StoreError>;
    /// 終わった・死んだ他のインスタンスの行を消す（`keep` は消さない）。対象は `drained_at` が入っている
    /// 行と、`heartbeat_at` が `heartbeat_before` より古い行。消した `instance_id` を昇順で返す。
    fn instance_delete_stale(
        &self,
        keep: &str,
        heartbeat_before: OffsetDateTime,
    ) -> Result<Vec<String>, StoreError>;

    // ---- ADR-0059 D6（Phase 99）: クラスタの作業ディレクトリの DB 上書き ----

    /// 1 クラスタ分の上書き。無ければ `Ok(None)`（設定ファイルの値を使う）。
    fn cluster_settings_get(&self, cluster_id: &str)
    -> Result<Option<ClusterSettings>, StoreError>;
    /// 全クラスタの上書き一覧（`cluster_id` 昇順。`GET /clusters` が一括で使う）。
    fn cluster_settings_list(&self) -> Result<Vec<ClusterSettings>, StoreError>;
    /// `work_dir = Some(..)` なら upsert、`None` なら行を消す（上書きの解除）。
    fn cluster_settings_set(
        &self,
        cluster_id: &str,
        work_dir: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<(), StoreError>;
    /// ADR-0078 D5: クラスタの接続・切断・鍵認証の試みを 1 行追記する。
    fn cluster_connection_record(&self, record: &ClusterConnectionRecord)
    -> Result<(), StoreError>;
    /// ADR-0078 D5: `since` 以降（含む）の記録（古い順）。`GET /clusters` の `stats` が数える。
    fn cluster_connection_list_since(
        &self,
        since: OffsetDateTime,
    ) -> Result<Vec<ClusterConnectionRecord>, StoreError>;

    // ---- ADR-0072 D5（Phase E2）: 実行層の派生索引（execution_plans / work_units / runs）----
    //
    // 正本は `events`（`Event::ExecutionPlanned` / `WorkUnitTransitioned` / `WorkerStarted` /
    // `WorkerFinished` / `CheckpointSaved`）。この 3 表は対応する Event と同じトランザクションで
    // 書く。`task_ops::execution::rebuild_from_events` で events から作り直して一致を確かめられる。

    /// D14: 新しい計画を採用する（`Event::ExecutionPlanned` と `execution_plans` / `work_units` の
    /// 行を同じトランザクションで書く）。E2 は新規のみ: そのタスクに既に `active` な計画があれば
    /// `StoreError::InUse`（replan は E4。`PlanOrigin::Repair` 等で明示的に旧版を `superseded` にした
    /// 上で採用する経路は別に用意する）。
    /// `extra_events`（ADR-0074 D2.1（Phase F3 途中確認）: `Event::PausePointsResolved` 用）は
    /// `event`（`ExecutionPlanned`）より先に、同じトランザクションで書く（`execution_plan_replan` と
    /// 同じ規律）。
    fn execution_plan_adopt(
        &self,
        task_id: TaskId,
        plan: ExecutionPlanRow,
        work_units: Vec<WorkUnitRow>,
        extra_events: Vec<Event>,
        event: Event,
    ) -> Result<(), StoreError>;

    /// ADR-0074 D3.7（Phase F4b (f)）: `execution_plan_adopt` と同じことを行い、**同じトランザクションで**
    /// planner の `children` から作った子 Task（`parent_id = task_id`、draft なら `Accept`）を挿入し、
    /// `Event::Delegated{run_id: <planner run>, task_ids}` を親に残す（`delegate_children` と同じ形）。
    #[allow(clippy::too_many_arguments)]
    fn execution_plan_adopt_delegating(
        &self,
        task_id: TaskId,
        plan: ExecutionPlanRow,
        work_units: Vec<WorkUnitRow>,
        extra_events: Vec<Event>,
        event: Event,
        run_id: &str,
        children: Vec<Task>,
    ) -> Result<Vec<TaskId>, StoreError>;

    /// そのタスクの `active` な計画（無ければ `None`）。
    fn execution_plan_active(
        &self,
        task_id: TaskId,
    ) -> Result<Option<ExecutionPlanRow>, StoreError>;
    /// そのタスクの計画（`version` 昇順。履歴を含む）。
    fn execution_plan_list(&self, task_id: TaskId) -> Result<Vec<ExecutionPlanRow>, StoreError>;

    /// そのタスクの WorkUnit（`seq` 昇順）。
    fn work_units_for(&self, task_id: TaskId) -> Result<Vec<WorkUnitRow>, StoreError>;
    /// 1 件。無ければ `None`。
    fn work_unit_get(&self, id: &str) -> Result<Option<WorkUnitRow>, StoreError>;

    /// ADR-0079 D7（Phase R1a / migration 0031）: 決定の要求の行（`created_at` 昇順、同順位は id）。
    /// `root_id` を渡せばその木だけ。行は `Event::DecisionRequested` / `DecisionAnswered` /
    /// `DecisionWithdrawn` の追記と同じトランザクションで store が書く（派生）。
    fn decisions_list(
        &self,
        root_id: Option<TaskId>,
    ) -> Result<Vec<crate::decision::DecisionRow>, StoreError>;
    /// ADR-0079 D7: 1 件（無ければ `None`）。
    fn decision_get(&self, id: &str) -> Result<Option<crate::decision::DecisionRow>, StoreError>;
    /// ADR-0079 D15: `decisions` の全行を渡した集合でまるごと置き換える（`events` は変えない）。
    /// replay の再構築（`task_ops::replay::rebuild_decisions`）を書き戻すのに使う。
    fn decisions_replace(&self, rows: Vec<crate::decision::DecisionRow>) -> Result<(), StoreError>;
    /// ADR-0079 D7（Phase R3a）: 決定への回答・取り下げ・revise の 1 トランザクション。`decisions` の行
    /// `decision_id` の状態が今も `expect` であることを同じトランザクションで確かめ（違えば何も書かず
    /// `Ok(false)` = API の 409）、`task_id`（決定を出した節点）の WU の行 `updated` を書き換え、`events`
    /// （`DecisionAnswered` / `DecisionWithdrawn`・`WorkUnitTransitioned` など）を積む。
    fn decision_resolve_apply(
        &self,
        task_id: TaskId,
        decision_id: &str,
        expect: crate::decision::DecisionStatus,
        updated: Vec<WorkUnitRow>,
        events: Vec<Event>,
    ) -> Result<bool, StoreError>;

    /// D6: WorkUnit の行を書き換え、`Event::WorkUnitTransitioned`（`event`）を同じトランザクションで
    /// 追記する。`updated.id` の行を丸ごと差し替える（呼び出し側が新しい状態・カウンタを計算済み）。
    fn work_unit_transition(
        &self,
        task_id: TaskId,
        updated: WorkUnitRow,
        event: Event,
    ) -> Result<(), StoreError>;

    /// ADR-0074 D1.5（Phase F2）: WU の lease を取る。1 トランザクションで「Task が `Running`」
    /// 「WU が `ready` / `needs_continuation`」を確かめ、WU を `running` にし（`runs + 1`、
    /// `last_run_id = run_id`、lease 列）、`WorkUnitTransitioned{to: running, reason: "dispatch"}` を
    /// 追記し、**Task の lease の期限を WU の lease の期限まで延ばす**（短くはしない）。
    /// `branch` / `base_commit` が `Some` なら WU の行にも書く（WU の worktree を切ったとき）。
    /// 条件に合わなければ `Ok(false)`（何も書かない）。
    #[allow(clippy::too_many_arguments)]
    fn acquire_work_unit_lease(
        &self,
        task_id: TaskId,
        work_unit_id: &str,
        run_id: &str,
        ttl: StdDuration,
        branch: Option<String>,
        base_commit: Option<String>,
    ) -> Result<bool, StoreError>;

    /// ADR-0074 D1.4（Phase F2b）: WU の行の追加（`inserted`）と書き換え（`updated`）と `events` を
    /// 1 トランザクションで書く（統合の repair WU を足し、統合 WU の依存を付け足す、など）。
    /// Task の状態は変えない。
    fn work_units_apply(
        &self,
        task_id: TaskId,
        inserted: Vec<WorkUnitRow>,
        updated: Vec<WorkUnitRow>,
        events: Vec<Event>,
    ) -> Result<(), StoreError>;

    /// ADR-0079 §7 R1b: 終わっていない kind task の unit（`pending` / `ready` / `running` /
    /// `needs_continuation` / `blocked`）を持つ task の id（重複なし、昇順）。dispatcher の tick が
    /// 子 task の生成と状態の写しの照合に使う（木が無ければ空で、何もしない）。
    fn tasks_with_open_task_units(&self) -> Result<Vec<TaskId>, StoreError>;

    /// ADR-0079 D3（Phase R2a）: 木の節点（`root_id` の task 自身と、`tasks.root_id = root_id` の子孫）。
    /// `created_at` 昇順（同順位は rowid）。木の上限の数え上げ（`task_ops::tree::tree_counters`）に使う。
    /// root 自身は `tree` を持たない（`root_id` 列は NULL。R1b 付記 6.）ので id で拾う。
    fn tree_tasks(&self, root_id: TaskId) -> Result<Vec<Task>, StoreError>;

    /// ADR-0079 D4 (4)（Phase R1b）: kind task の unit から子 task を作る 1 トランザクション。
    /// 親が終端でなく、unit の行（`unit.id`）が今も `ready` で `child_task_id` を持たないことを確かめ、
    /// 子を挿入して `Event::Created{origin: plan_unit}` を積み、unit の行を `unit`（呼び出し側が
    /// `running`・`child_task_id` を書いたもの）に差し替え、親の events に `parent_events`
    /// （`WorkUnitTransitioned` と `ChildTaskCreated`）を積む。条件に合わなければ何も書かず `Ok(false)`。
    ///
    /// ADR-0079 D9（Phase R2b）: `replaces = Some(prev)` は基盤の失敗の作り直し（同じ unit から新しい子）。
    /// unit の行が `running` で `child_task_id = prev` であることを確かめる（`ready`・子なしの代わりに）。
    fn tree_child_create(
        &self,
        parent_id: TaskId,
        child: &Task,
        unit: WorkUnitRow,
        parent_events: Vec<Event>,
        replaces: Option<&str>,
    ) -> Result<bool, StoreError>;

    /// ADR-0079 D15（Phase R5b-prep）: 人の計画（origin human）の採用と、その計画の unit の `adopt` による既存の
    /// task の採用を 1 トランザクションで書く。`execution_plan_adopt` と同じことをした後に `after_events`
    /// （`WorkUnitTransitioned`・`ChildAdopted`・`UnitGateOverridden`・`DecisionRequested` など。`work_units` の行は
    /// 呼び出し側がこれらを当てた後の値で渡す）を積み、`adoptions` の各 task を書き換える。採用する task が今も
    /// `expect_status` で木に属していない（`tree` が無い）ことを確かめ、違えば何も書かずに `Ok(false)`。
    #[allow(clippy::too_many_arguments)]
    fn execution_plan_adopt_tree(
        &self,
        task_id: TaskId,
        plan: ExecutionPlanRow,
        work_units: Vec<WorkUnitRow>,
        extra_events: Vec<Event>,
        event: Event,
        after_events: Vec<Event>,
        adoptions: Vec<TreeAdoption>,
    ) -> Result<bool, StoreError>;

    /// ADR-0079 D15（Phase R5b-prep）: 採用済みの計画の kind task の unit（`unit_id`）に既存の task を後から採用する
    /// 1 トランザクション（`POST /tasks/{id}/tree/adopt`）。計画を持つ task が終端でなく、unit の行が今も
    /// `expect_unit_status` の kind task で `child_task_id` を持たず、採用する task が `expect_status` で `tree` を
    /// 持たないことを確かめる。`updated` の行を書き換え、計画を持つ task に `events` を積み、採用する task を書き換える。
    /// 条件に合わなければ何も書かず `Ok(false)`。
    #[allow(clippy::too_many_arguments)]
    fn tree_adopt_apply(
        &self,
        owner_id: TaskId,
        unit_id: &str,
        expect_unit_status: WorkUnitStatus,
        updated: Vec<WorkUnitRow>,
        events: Vec<Event>,
        adoption: TreeAdoption,
    ) -> Result<bool, StoreError>;

    /// ADR-0074 D1.4（Phase F2）: Task の lease（工程の保持者）の期限を `ttl` 先まで延ばす
    /// （短くはしない）。`Running` でなければ `Ok(false)`。統合の間の延長に使う。
    fn extend_task_lease(&self, task_id: TaskId, ttl: StdDuration) -> Result<bool, StoreError>;

    /// ADR-0074 D1.3 3.（Phase F2）: `Running` で、`phase` のある（v2 の）WU に `ready` /
    /// `needs_continuation` のものを持つ Task（`created_at` 昇順、最大 `limit` 件）。
    /// 公平性: `dispatch_ready` はこの Task たちの 2 本目以降を、Ready の Task の 1 本目の後に回す。
    fn running_tasks_with_runnable_work_units(&self, limit: usize)
    -> Result<Vec<Task>, StoreError>;

    /// D5: run 開始時に `runs` の行を 1 件作る（`status = running`）。`Event::WorkerStarted` と同じ
    /// トランザクションでは**ない**（既存コードの慣習に合わせ、`append_event` の直後に呼ぶ。監査上の
    /// 実害は無い: 再起動時の照合は `work_units.status` を正とする）。
    fn run_index_start(&self, row: RunRow) -> Result<(), StoreError>;
    /// D5: run 終了時に `runs` の行を更新する。無ければ `Ok(false)`。
    #[allow(clippy::too_many_arguments)]
    fn run_index_finish(
        &self,
        run_id: &str,
        status: RunIndexStatus,
        checkpoint: Option<crate::execution::Checkpoint>,
        usage: Option<crate::model::Usage>,
        metrics: Option<crate::model::RunMetrics>,
        finished_at: OffsetDateTime,
    ) -> Result<bool, StoreError>;
    fn run_index_get(&self, run_id: &str) -> Result<Option<RunRow>, StoreError>;
    /// そのタスクの run（`started_at` 昇順）。
    fn runs_for_task(&self, task_id: TaskId) -> Result<Vec<RunRow>, StoreError>;
    /// その WorkUnit の run（`started_at` 昇順）。
    fn runs_for_work_unit(&self, work_unit_id: &str) -> Result<Vec<RunRow>, StoreError>;
    /// ADR-0079 付記「R6-1」D4: task が終端（done / failed / cancelled）なのに `runs` 索引で `running` のままの行を、
    /// task ごとに 1 トランザクションで閉じる（`WorkerFinished{end: Cancelled, outcome: "interrupted: …"}` を積む）。
    /// 終端への遷移は同じトランザクションで閉じるので、ここで見つかるのは R6-1 より前に残った行だけ。閉じた
    /// `(task, run_id)` を返す（dispatcher の起動時と定期の照合が 1 行ずつログに残す）。
    fn close_runs_of_terminal_tasks(&self) -> Result<Vec<(TaskId, String)>, StoreError>;

    /// `updated_at >= since` のタスク別実行集計を派生索引から 1 回の SQL で読む。
    ///
    /// | API の欄 | events 版 | 索引版 |
    /// |---|---|---|
    /// | tasks / done / failed / other | Task と最終 status | tasks.status（API が集約） |
    /// | completion_rate | done / (done + failed) | 同じ計算（API） |
    /// | continuations | Transitioned `continue` | work_units.continuations の和 |
    /// | max_turn_failures | WorkerFinished `BudgetExhausted(Turns)` | runs.status には種類が無く、最新 run の events だけでは過去 run 分も復元できない |
    /// | repairs | repair WorkUnit の key 数 | work_units.kind = repair の行数 |
    /// | replans | supersedes のある ExecutionPlanned 数 | execution_plans.version > 1 の行数 |
    /// | gate_mode / genre / assignee | Task.routing / Task の欄 | tasks.json.routing / 列 |
    /// | lane | routing_audit の最後の lane | runs に lane 列が無く、最新 run の events だけでは過去の監査結果を保証できない |
    ///
    /// Atomic task には work_units 行が無く、その `continue` と `worker_error` retry は索引から
    /// 復元できない。古い run（E2 より前）も runs に埋め戻していない。API が補完する場合は
    /// 該当タスクに限って events を読む必要があり、全タスクの events 走査へ戻さないこと。
    /// `budget_exhausted_runs` は種類を問わない数で、turns の数として扱ってはならない。
    fn execution_metrics_task_rows(
        &self,
        since: Option<OffsetDateTime>,
    ) -> Result<Vec<ExecutionMetricsTaskRow>, StoreError>;

    /// ADR-0072 D5/D15（Phase E2b）: `task_id` の `work_units` 行を、渡した集合でまるごと置き換える
    /// （既存行を全て削除してから挿入。1 トランザクション）。`events` は変えない。
    /// `celerisctl replay --apply`（`task_ops::replay::check_and_apply_execution`）が
    /// `rebuild_work_units_and_runs` の再構築結果を書き戻すのに使う（events が正本、索引は派生。D5）。
    fn work_units_replace(&self, task_id: TaskId, rows: Vec<WorkUnitRow>)
    -> Result<(), StoreError>;
    /// 同上。`task_id` の `runs` 行を渡した集合でまるごと置き換える。
    fn runs_replace(&self, task_id: TaskId, rows: Vec<RunRow>) -> Result<(), StoreError>;
    /// ADR-0072 D5/D17（Phase E4b 項目5）: 同上。`task_id` の `execution_plans` 行（版の履歴）を
    /// 渡した集合でまるごと置き換える。`task_ops::replay::check_and_apply_execution` が
    /// `rebuild_execution_plans`（`Event::ExecutionPlanned` からの再構築。replan で複数版になった
    /// 履歴を含む）の結果を書き戻すのに使う。
    fn execution_plans_replace(
        &self,
        task_id: TaskId,
        rows: Vec<ExecutionPlanRow>,
    ) -> Result<(), StoreError>;

    /// ADR-0072 D16（Phase E4）: reviewer repair の採用。`Trigger::ReviewRepair`
    /// （`Reviewing → Ready`、attempts 据え置き）と、`new_plan`（`Some` のときだけ挿入。atomic な
    /// Task で暗黙の WorkUnit を初めて実体化するときの `execution_plans` 行、D5）・`work_units`
    /// （repair WU。atomic なら `main`（done）も含む）・`extra_events`（`WorkerFinished{role:
    /// reviewer}` / `ReviewVerdict` / repair WU の `WorkUnitTransitioned` 等）を同じトランザクションで
    /// 書く。呼び出し側（`dispatcher.rs`）が D16 の上限（`max_repairs`/`max_repairs_per_class`）を
    /// 検査済みであることが前提（ここでは検査しない）。
    fn review_repair_apply(
        &self,
        task_id: TaskId,
        extra_events: Vec<Event>,
        new_plan: Option<ExecutionPlanRow>,
        work_units: Vec<WorkUnitRow>,
    ) -> Result<Outcome, StoreError>;

    /// 配送の局所修復。Reopen と repair WU を同一トランザクションで保存する。
    fn delivery_repair_apply(
        &self,
        task_id: TaskId,
        extra_events: Vec<Event>,
        new_plan: Option<ExecutionPlanRow>,
        work_units: Vec<WorkUnitRow>,
    ) -> Result<Outcome, StoreError>;

    /// ADR-0072 D17（Phase E4）: replan の採用。旧 `active` な計画を `superseded` にし、新しい版
    /// （`new_plan.version = old.version + 1`）を挿入する。`updated_work_units` は既存行の書き換え
    /// （spec が変わった未完了 WU、または `superseded` にする削除された WU）、`new_work_units` は
    /// 新しい key の追加。`extra_events`（削除された WU の `WorkUnitTransitioned` 等）→
    /// `plan_event`（`Event::ExecutionPlanned{supersedes: Some(old_plan_id), ..}`）の順で同じ
    /// トランザクションに追記する。旧版が `active` でなければ `StoreError::InUse`
    /// （並行 replan の検出）。
    #[allow(clippy::too_many_arguments)]
    fn execution_plan_replan(
        &self,
        task_id: TaskId,
        old_plan_id: String,
        new_plan: ExecutionPlanRow,
        updated_work_units: Vec<WorkUnitRow>,
        new_work_units: Vec<WorkUnitRow>,
        extra_events: Vec<Event>,
        plan_event: Event,
    ) -> Result<(), StoreError>;
}
