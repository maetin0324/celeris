//! 表示用のビュー型とその組み立て（ADR-0013 D7 / D12、`docs/api/v1/gui-api.md` §3.3 / §3.5 / §5.2〜§5.4 / §6.2）。
//!
//! `celerisctl show --json`、API の `GET /tasks` / `GET /tasks/{id}` / `GET /tasks/{id}/runs` が同じ関数を使う。GUI は結果を表示するだけで
//! 再計算しない。I/O はストアの読み取りだけで、ファイル（`runs/<run_id>/` の存在確認など）は呼び出し側（task-api）が埋める。
//! 時刻は RFC 3339 の文字列。

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Duration;

use schemars::JsonSchema;
use serde::Serialize;
use task_core::{
    Check, Event, EventRow, ListFilter, ListOrder, RunRole, Status, Task, TaskId, TaskKind,
    TaskStore, Tier, Usage, WorkspaceSpec,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::derive::{self, AnswerNote, ReviewNote};
use crate::error::OpsError;

/// `event_rows_for` に渡す「実質無制限」の件数上限（タスク 1 件分の全イベントを読む用途）。
pub(crate) const ALL_EVENTS: usize = usize::MAX;

/// ビューの組み立てに必要な設定値（celeris の設定から呼び出し側が詰める）。
#[derive(Debug, Clone, PartialEq)]
pub struct ViewContext {
    /// `WorkspaceSpec::Local` の相対パスの基準。
    pub workspace_root: PathBuf,
    pub retry_backoff_base: Duration,
    pub retry_backoff_max: Duration,
    pub max_requeues: u32,
    /// ADR-0019 D2: `[[clusters]]` のうちビューに要る分（worktree のパスとブランチを出すため）。id → 設定。
    pub clusters: std::collections::HashMap<String, ClusterViewInfo>,
}

/// ADR-0019 D2 / ADR-0032 D1: `TaskDetail.worktree` を組み立てる（`sync` / `worktree_root`）のと、クラスタの
/// 認証方式（`auth`）を運ぶのに要るクラスタの設定。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClusterViewInfo {
    /// `"worktree"` のときだけ `TaskDetail.worktree` が出る（`"rsync"` / `"none"` では `null`）。
    pub sync: String,
    /// worktree を置く親ディレクトリ。`None` なら `<project>/.celeris-worktrees`。
    pub worktree_root: Option<PathBuf>,
    /// ADR-0032 D1: `"manual"` / `"publickey"` / `"totp"`（既定 `"manual"`）。
    pub auth: String,
}

/// worktree のブランチ名の接頭辞（ADR-0019 D2）。`task_worker::WorktreeSettings::default().branch_prefix` と同じ値。
pub const WORKTREE_BRANCH_PREFIX: &str = "celeris/";

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct TaskRef {
    pub id: TaskId,
    pub title: String,
    pub kind: TaskKind,
    pub status: Status,
    /// 今この状態で許される操作（ADR-0015 D4。GUI は §5.4 の規則を再実装しない）。
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct TaskSummary {
    pub id: TaskId,
    pub parent_id: Option<TaskId>,
    pub kind: TaskKind,
    pub status: Status,
    pub title: String,
    pub priority: i32,
    pub tier: Tier,
    pub adapter: Option<String>,
    pub attempts: u32,
    pub max_retries: u32,
    pub depends_on: Vec<TaskId>,
    pub created_at: String,
    pub updated_at: String,
    pub lease_expires_at: Option<String>,
    pub backoff_until: Option<String>,
    pub children: u32,
    pub pending_children: u32,
    /// ADR-0016 D1 の `Task.role`（GUI-R2: 一覧の各行に役割のラベルを出すため。`TaskDetail.role` と同じ値）。
    pub role: Option<String>,
    /// ADR-0027 D1 の `Task.genre`（`role` と同じ理由で一覧に出す。`TaskDetail.genre` と同じ値）。
    pub genre: Option<String>,
    /// ADR-0033 D2 の `Task.assignee`（組織のノード id。GUI-R3: 一覧に「誰の仕事か」を出すため）。
    pub assignee: Option<String>,
    /// 対話用タスク（人への返事のための run）か（GUI-R3: 仕事の木や一覧から隠せるように）。
    pub conversation: bool,
    /// GUI 監査 H4（Phase 29）: 裏方タスクの印（`"conversation"` | `"compaction"` | `"approval"` |
    /// `"review"` | `null`）。`task_core::support_kind` の決定的な判定。GUI はこれで仕事の木から裏方を外せる。
    pub support: Option<String>,
    /// ADR-0079 D13（Phase R5a）: 案件の root task か（`task_core::is_root_task`）。
    #[serde(default)]
    pub is_root_task: bool,
    /// ADR-0079 D13（Phase R5a）: この task 自身が subtree の一時停止中か（`Task.paused_at` がある。祖先の
    /// 一時停止で止まっているかは `TaskDetail.paused_by`）。
    #[serde(default)]
    pub paused: bool,
    /// 今この状態で許される操作（ADR-0015 D4）。
    pub actions: Vec<Action>,
    // ---- ADR-0044 D3/D4（Phase 53）: ボードのカードが要るもの。ここから ----
    /// ADR-0044 D3 の `Task.labels`。
    pub labels: Vec<String>,
    /// ADR-0044 D3 の `Task.category`。
    pub category: task_core::TaskCategory,
    /// ADR-0044 D3: `priority` を P0〜P3 に丸めたもの（`i32` は互換のため残す）。
    pub priority_label: String,
    /// ADR-0033 D2 の `Task.project_id`（ボードは案件で絞るので一覧にも出す）。
    pub project_id: Option<task_core::ProjectId>,
    /// ADR-0033 D2 の `Task.milestone_id`（カードに途中目標を出すため）。
    pub milestone_id: Option<task_core::MilestoneId>,
    // ---- ADR-0044 D3/D4（Phase 53）: ここまで ----
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct TaskList {
    pub items: Vec<TaskSummary>,
    pub next_cursor: Option<String>,
    pub total: u64,
    /// status 名 → 件数（フィルタに関係なく DB 全体。0 件の status は現れない）。
    pub counts_by_status: BTreeMap<String, u64>,
}

/// `celerisctl show --json` と `GET /api/v1/tasks/{id}` の本体。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct TaskDetail {
    pub task: Task,
    /// ADR-0130: effective task hint, including inheritance from a parent unit.
    pub expected_write_paths: Option<Vec<String>>,
    /// Committed diffs for this task's runs, with snapshot status and Git SHAs.
    pub actual_run_write_sets: Vec<ActualWriteSetView>,
    /// Cumulative committed diffs for completed work units.
    pub actual_work_unit_write_sets: Vec<ActualWriteSetView>,
    /// Last observed target snapshot; reading the detail does not run Git.
    pub behind_target: task_core::behind_target::BehindTarget,
    /// 手元の作業ディレクトリ（絶対パス）。`WorkspaceSpec::Remote` では写し `workspace_root/<task_id>`（run のログはここ。ADR-0018 D1）。
    pub workspace_dir: Option<String>,
    /// ADR-0018: `WorkspaceSpec::Remote` のクラスタ（`[[clusters]] id`）。ローカルのタスクは `null`。
    pub cluster: Option<String>,
    /// ADR-0016 D1: `Task.role`（GUI の表示用に最上位にも出す）。
    pub role: Option<String>,
    /// ADR-0027 D1: `Task.genre`（`role` と同じ理由で最上位にも出す）。
    pub genre: Option<String>,
    /// ADR-0044 D3（Phase 53）: `task.priority` を P0〜P3 に丸めたもの（GUI の編集フォーム用）。
    pub priority_label: String,
    /// ADR-0016 D2: 各 run が `delegate` で作った子（`Event::Delegated` の順）。
    pub delegated: Vec<DelegatedView>,
    pub timers: Timers,
    pub criteria: Vec<CriterionView>,
    pub runs: Vec<RunSummary>,
    pub prior_review: Vec<ReviewNote>,
    pub answers: Vec<AnswerNote>,
    pub latest_question: Option<String>,
    pub approvals: Vec<ApprovalLink>,
    pub dependencies: Vec<TaskRef>,
    pub dependents: Vec<TaskRef>,
    pub children: Vec<TaskRef>,
    pub actions: Vec<Action>,
    pub worker_run_hint: Option<String>,
    /// ADR-0019 D2: `sync = "worktree"` のクラスタで動くタスクの worktree。人はここを見て diff / commit する。
    pub worktree: Option<WorktreeView>,
    /// ADR-0070 D1（Phase 116）: `task.status == Failed` のときだけ `Some`（分類・理由・配送済みの release）。
    /// GUI のタスク詳細の赤いバナーの材料。
    pub failure: Option<FailureSummary>,
    /// ADR-0072 D20（Phase E5）: Execution 節。events に E-phase 由来の活動（gate の判定・計画・
    /// checkpoint・WorkUnit の遷移）が 1 件も無ければ `None`（D23: 既存の古いタスクの詳細を壊さない）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<ExecutionView>,
    /// ADR-0079 D13（Phase R5a）: 案件の root task か（`task_core::is_root_task`）。
    #[serde(default)]
    pub is_root_task: bool,
    /// ADR-0079 D13（Phase R5a）: subtree の一時停止でこの task の dispatch を止めている task（自分か、
    /// `paused_at` を持つ一番近い祖先）。止まっていなければ省略。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused_by: Option<TaskId>,
    /// ADR-0090 D5: この task が待っているクラスタ job（`waiting` の wait。無ければ省略）。GUI の 1 行
    /// 「クラスタ job を待っています: 42634 (R) 42635 (Q) …」の材料。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_job_wait: Option<ClusterJobWaitView>,
    /// ADR-0120 D5: review 前同期の衝突解消（IntegrationRepair）の現在の状況。履歴が無い task では
    /// 省略する。`failure`（実装失敗・レビュー不合格）とは別の欄: review を妨げず成果を保って衝突を
    /// 解消する試みであり、`exhausted` でも task を直接 `failed` にはしない（従来経路へ落ちるだけ）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integration_repair: Option<IntegrationRepairView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ActualWriteSetView {
    pub owner_id: String,
    pub repo_id: String,
    pub base_sha: Option<String>,
    pub head_sha: Option<String>,
    pub paths: Vec<String>,
    pub status: String,
    pub reason: Option<String>,
    pub recorded_at: String,
}

impl From<task_core::write_set::WriteSetRecord> for ActualWriteSetView {
    fn from(record: task_core::write_set::WriteSetRecord) -> Self {
        Self {
            owner_id: record.owner_id,
            repo_id: record.repo_id.to_string(),
            base_sha: record.base_sha,
            head_sha: record.head_sha,
            paths: record.paths,
            status: record.status.as_str().to_string(),
            reason: record.reason,
            recorded_at: record.recorded_at,
        }
    }
}

/// ADR-0120 D5: `TaskDetail.integration_repair` / 受信箱 `AttentionItem::Failed.integration_repair`
/// が共有する形。最後の integration repair event と対応する scheduled event から決定的に組み立てる
/// （`task_core::integration_repair_status`）。`reason` / `rollback_to_sha` / `fallback` は `exhausted`
/// のときだけ値を持つ。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct IntegrationRepairView {
    pub state: task_core::IntegrationRepairState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_unit_id: Option<String>,
    pub attempt: u32,
    /// `task_ops::delivery::MAX_INTEGRATION_REPAIRS`。
    pub max_attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_ref: Option<String>,
    pub target_sha: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflict_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<task_core::IntegrationRepairExhaustReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_to_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<bool>,
}

/// ADR-0120 D5: `events`（古い順）から [`IntegrationRepairView`] を組み立てる。履歴が無ければ `None`。
pub(crate) fn integration_repair_view(events: &[(u64, Event)]) -> Option<IntegrationRepairView> {
    let event_list: Vec<Event> = events.iter().map(|(_, e)| e.clone()).collect();
    let status = task_core::integration_repair_status(&event_list)?;
    Some(IntegrationRepairView {
        state: status.state,
        work_unit_id: status.work_unit_id,
        attempt: status.attempt,
        max_attempts: crate::delivery::MAX_INTEGRATION_REPAIRS,
        target_ref: status.target_ref,
        target_sha: status.target_sha,
        before_sha: status.before_sha,
        conflict_files: status.conflict_files,
        reason: status.reason,
        rollback_to_sha: status.rollback_to_sha,
        fallback: status.fallback,
    })
}

/// ADR-0090 D5: 待っているクラスタ job（`cluster_job_waits` の `waiting` の行）。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ClusterJobWaitView {
    pub wait_id: String,
    /// 待っている WorkUnit（atomic の run なら省略）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_unit_id: Option<String>,
    pub run_id: String,
    pub cluster: String,
    pub scheduler: task_core::cluster_job::ClusterScheduler,
    /// job ごとの直近の状態（申告の順。まだ poll していない job は `unknown`）。
    pub jobs: Vec<task_core::cluster_job::ClusterJobStatus>,
    /// `42634 (R) 42635 (Q)` の形。
    pub status_line: String,
    pub poll_secs: u64,
    pub created_at: String,
    pub deadline: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_polled_at: Option<String>,
    /// 次の poll の目安（`last_polled_at + poll_secs`。まだ poll していなければ省略 = 次の tick）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_poll_at: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
}

/// ADR-0090 D5: task の `waiting` の wait（新しいもの）を GUI・API の形にする。
pub fn active_cluster_job_wait(
    store: &dyn TaskStore,
    task_id: TaskId,
) -> Result<Option<ClusterJobWaitView>, OpsError> {
    let Some(wait) = store
        .cluster_job_waits_for_task(task_id)?
        .into_iter()
        .rev()
        .find(|w| w.state == task_core::cluster_job::ClusterJobWaitState::Waiting)
    else {
        return Ok(None);
    };
    let jobs = wait.job_statuses();
    let next_poll_at = wait.last_polled_at.as_deref().and_then(|t| {
        let last =
            time::OffsetDateTime::parse(t, &time::format_description::well_known::Rfc3339).ok()?;
        let next = last + time::Duration::seconds(i64::try_from(wait.poll_secs).ok()?);
        next.format(&time::format_description::well_known::Rfc3339)
            .ok()
    });
    Ok(Some(ClusterJobWaitView {
        status_line: task_core::cluster_job::status_line(&jobs),
        wait_id: wait.wait_id,
        work_unit_id: wait.work_unit_id,
        run_id: wait.run_id,
        cluster: wait.cluster,
        scheduler: wait.scheduler,
        jobs,
        poll_secs: wait.poll_secs,
        created_at: wait.created_at,
        deadline: wait.deadline,
        last_polled_at: wait.last_polled_at,
        next_poll_at,
        summary: wait.summary,
    }))
}

/// ADR-0072 D20（Phase E5）: タスクの状態バッジの横に出す、今どの段階かの導出値（D6 の R3 の代替。
/// 状態機械そのものには足さない）。`Task.status` から次のとおり決める（[`build_execution_view`] 参照）:
/// `reviewing` は常に `verifying`。`running` は、進行中の WorkUnit があれば `repairing`
/// （`kind = repair`）か `executing`、無ければ（計画はあるのに走っている WU が無い）planner run が
/// 動いていると見なして `planning`。それ以外（計画が無い・終端）は `None`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionPhase {
    Planning,
    Executing,
    Repairing,
    Verifying,
    /// ADR-0074 D2.2（Phase F3 途中確認）: 工程の後の途中確認で止まっている（`blocked` で、直前の
    /// 遷移の reason が `awaiting_human`）。
    AwaitingHuman,
    /// ADR-0079 D5（Phase R1b）: `ready` のまま、走れる自分の leaf が無く子 task（計画の kind task の
    /// unit）だけを待っている（dispatch されず lease も持たない。待っている子は
    /// `ExecutionView.awaiting_children`）。
    AwaitingChildren,
    /// ADR-0079 D8（Phase R3b）: root の計画が人の承認を待っている（`blocked` で、直前の遷移の reason が
    /// `awaiting_plan_approval`）。理由は `ExecutionView.plan_approval`。
    AwaitingPlanApproval,
}

/// ADR-0079 D8（Phase R3b）: 承認を待っている root の計画（Execution 節と GUI の 3 つのボタンの材料）。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct PlanApprovalView {
    pub plan_id: String,
    /// `PlanApprovalRequested.reasons`（`decisions:<key>,…` / `review_human:<stage>` / `near_limit:<設定名>:<値>/<上限>`）。
    pub reasons: Vec<String>,
    /// 理由の人が読む 1 行。
    pub summary: String,
}

/// ADR-0079 D5（Phase R1b）: 親が待っている子 task 1 件（`ExecutionPhase::AwaitingChildren` の理由）。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct AwaitedChildView {
    /// 親の計画の unit の key。
    pub unit_key: String,
    /// 子 task（まだ作られていない unit〈`max_parallel_child_tasks` の空き待ち〉は `None`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<TaskId>,
    pub title: String,
    /// 子の状態（作られていなければ `None`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
}

/// ADR-0074 D2.3/D2.4（Phase F3 途中確認）: 途中確認で止まっている Task の途中報告（Execution 節と
/// GUI の 3 つのボタンの材料）。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct PhaseCheckpointView {
    pub report: task_core::PhaseReport,
    /// 途中報告の Markdown（`GET /tasks/{id}/artifacts/{idx}`）。書けなかったなら無い。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_idx: Option<usize>,
}

/// D19/D20: タスク詳細の Execution 節そのもの。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ExecutionView {
    /// D13: Complexity Gate の判定（gate が判定していない Task には無い）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<task_core::ExecutionGateDecision>,
    /// ADR-0124: planner を省く直行経路か、既存の経路を維持するかの判定（評価していない Task
    /// には無い）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<task_core::RouteDecision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<ExecutionPhase>,
    /// 計画が無い Task（D20:「直接実行」の 1 行）は `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<ExecutionPlanOverview>,
    pub metrics: task_core::ExecutionMetrics,
    /// ADR-0074 D2.4（Phase F3 途中確認）: `awaiting_human` のときだけ。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_checkpoint: Option<PhaseCheckpointView>,
    /// ADR-0079 D5（Phase R1b）: `awaiting_children` のときだけ。待っている子（unit の `seq` 順）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub awaiting_children: Vec<AwaitedChildView>,
    /// ADR-0079 D8（Phase R3b）: `awaiting_plan_approval` のときだけ。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_approval: Option<PlanApprovalView>,
}

/// D20: 計画の概要（現在アクティブでない Task でも、生涯で作った WU をまとめて見せる。
/// `versions` が replan の履歴）。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ExecutionPlanOverview {
    pub id: String,
    pub version: u32,
    pub origin: task_core::PlanOrigin,
    pub rationale: String,
    pub work_units: Vec<ExecutionWorkUnitView>,
    /// D17(f): 版の履歴（`version` 昇順。superseded を含む）。
    pub versions: Vec<ExecutionPlanVersionSummary>,
    /// ADR-0074 D1.1（Phase F2b）: v2 の工程（配列の順が実行順。v1 は空）。GUI は WU の表を工程ごとの
    /// 見出しでまとめる。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub phases: Vec<task_core::PhaseSpec>,
    /// ADR-0074 D1.2（Phase F2b）: v2 の計画を並列 1 に倒した理由（`WorkUnitsSerialized`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serialized_reason: Option<String>,
}

/// D20: WU の表の 1 行。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ExecutionWorkUnitView {
    pub id: String,
    /// Explicit unit hint, or the inherited task hint.
    pub expected_write_paths: Option<Vec<String>>,
    pub key: String,
    pub seq: u32,
    pub kind: task_core::WorkUnitKind,
    pub title: String,
    pub status: task_core::WorkUnitStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<task_core::WorkUnitBlockedReason>,
    pub depends_on: Vec<String>,
    /// D21: WU は Task の担当を継ぐ（`Task.assignee` と同じ値）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    /// 直近の run の routing（`RoutingDecided`）から。まだ 1 度も走っていなければ `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<Tier>,
    pub runs: u32,
    pub continuations: u32,
    pub retries: u32,
    /// 最後の checkpoint の全文（GUI は折り畳んで出す。D8）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_checkpoint: Option<task_core::Checkpoint>,
    /// 直近の `WorkUnitTransitioned.reason`（失敗・レビューの理由。無ければ `None`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reason: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// ADR-0074 D1（Phase F2b）: v2 の工程の key（v1 は無し）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// ADR-0074 D1.2: WU のブランチ（`celeris-wu/<task_id>/<key>`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// ADR-0079 D4 (4)（Phase R1b）: kind task の unit の子 task（作られていれば）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_task_id: Option<String>,
    /// ADR-0074 D1.2: `WorkUnitCommitted` の commit。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_commit: Option<String>,
    /// ADR-0074 D1.4: 統合 WU の統合後の Task ブランチの HEAD。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrated_commit: Option<String>,
    /// ADR-0074 D1.5: 今この WU を実行している run（同時に走っている run を GUI に出す）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running_run_id: Option<String>,
}

/// D17(f): `execution_plans` の 1 版（監査用）。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ExecutionPlanVersionSummary {
    pub id: String,
    pub version: u32,
    pub origin: task_core::PlanOrigin,
    pub status: task_core::PlanStatus,
    /// `Event::ExecutionPlanned.reason`（replan を起こした理由。新規採用なら `None`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_at: Option<String>,
}

/// ADR-0070 D1（Phase 116）: `TaskDetail.failure` / 受信箱 `AttentionItem::Failed` が共有する形。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct FailureSummary {
    pub class: derive::FailureClass,
    /// 人が読む理由 1 行（`derive::classify_task_failure` が作る）。
    pub reason: String,
    /// 配送済み（`deliveries` に `release` が付いた記録がある）なら sha12。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered_release: Option<String>,
}

/// ADR-0019 D2: クラスタ側の worktree（celeris はここだけを触り、commit はしない）。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct WorktreeView {
    /// 元のリポジトリ（`WorkspaceSpec::Remote.path`）。
    pub project: String,
    /// worktree のパス（クラスタ上）。
    pub dir: String,
    /// worktree のブランチ（`celeris/<task_id>`）。celeris は commit しないので、変更は作業ツリーに残る。
    pub branch: String,
}

/// ADR-0016 D2: 1 回の `delegate`（`Event::Delegated`）の要約。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct DelegatedView {
    pub run_id: String,
    /// イベントの ts（`EventRow.ts`）。
    pub ts: String,
    /// 子の現在の状態。既に存在しない ID は落とす。
    pub tasks: Vec<TaskRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Timers {
    pub now: String,
    pub lease_expires_at: Option<String>,
    pub backoff_until: Option<String>,
    pub consecutive_requeues: u32,
    pub max_requeues: u32,
    pub consecutive_reviewer_requeues: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct CriterionView {
    pub idx: usize,
    pub text: String,
    pub check: Check,
    pub latest_verdict: Option<VerdictView>,
    pub approval: Option<ApprovalLink>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct VerdictView {
    pub run_id: String,
    pub criterion_idx: usize,
    pub pass: bool,
    pub reason: String,
    pub ts: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct RunSummary {
    pub run_id: String,
    /// ワーカー run か Reviewer run か（ADR-0014 D1。イベントに `role` が無ければ `worker`）。
    pub role: RunRole,
    pub adapter: String,
    pub model: String,
    pub provider: Option<String>,
    /// プールのアカウント（ADR-0024 D4 / ADR-0025）。`WorkerStarted.account` がある run だけ。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub outcome: Option<RunOutcomeKind>,
    pub outcome_text: Option<String>,
    /// ADR-0072 D19/D20（Phase E1）: この run の構造化した終わり方（`WorkerFinished.end`）。
    /// 導入前の run・分類できなかった run は `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<task_core::RunEnd>,
    /// ADR-0072 D20（Phase E5）: この run が実行した WorkUnit の `key`（計画の無い Task、または
    /// `work_units`/`runs` の索引に無い導入前の run は `None`）。`runs()` 自体は events だけの
    /// 純粋関数なので、[`run_work_unit_keys`] で store から引いた後段が埋める。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_unit: Option<String>,
    pub usage: Option<Usage>,
    pub progress: u32,
    pub artifacts: u32,
    pub verdicts: u32,
    pub reviewer_deferrals: u32,
    /// `runs/<run_id>/` のファイルの有無。task-ops は `None` を入れ、task-api が埋める。
    pub files: Option<RunFiles>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct RunFiles {
    pub stdout: bool,
    pub stderr: bool,
    pub result: bool,
    /// ADR-0023 D2: `runs/<run_id>/request.json`（ワーカーに渡した指示）。導入前の run には無いので既定は false。
    #[serde(default)]
    pub request: bool,
    /// ADR-0023 M1: `runs/<run_id>/prompt.txt`（claude-code / codex が実際に渡した文面）。fake には無い。
    #[serde(default)]
    pub prompt: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcomeKind {
    Done,
    Question,
    Error,
    Requeue,
    LeaseExpired,
    /// ADR-0044 D2/D8（Phase 53）: 人のコメントで止めた run（`interrupted: comment`）。
    /// **失敗ではない**ので `bad_news` にも `error_cooldown` にも数えない。
    Interrupted,
    /// ADR-0072 D9/D11（Phase E1）: 予算切れ・yield の続き（`Trigger::Continue`）。**失敗ではない**
    /// （checkpoint から新しい run が続く。`Interrupted` と同じく集計には数えない）。
    Continued,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ApprovalLink {
    pub approval: TaskRef,
    pub criterion_idx: Option<usize>,
    pub attempt: Option<u32>,
    pub decided: Option<ApprovalDecisionView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ApprovalDecisionView {
    pub by: String,
    pub approved: bool,
    pub note: Option<String>,
    pub ts: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Approve,
    Reject,
    Answer,
    Cancel,
    /// Phase 31（実機の事故、2026-09-18）: `failed`/`cancelled` を複製してやり直す（`POST /tasks/{id}/retry`）。
    Retry,
    /// ADR-0044 D1（Phase 53）: 人が編集できる（`PATCH /tasks/{id}`。終端でないタスクだけ）。
    Edit,
    /// ADR-0044 D2（Phase 53）: 終端のタスクを同じ worktree のまま再開する（`POST /tasks/{id}/reopen`。
    /// `done` / `failed` だけ。`cancelled` は worktree を消してあるので `Retry` を使う）。
    Reopen,
    /// ADR-0070 D2（Phase 116）: 既存成果の再判定（`POST /tasks/{id}/rereview`。ADR-0051 /
    /// ADR-0054 Phase 113 D3）。`done`、または直前の遷移が `review_fail` だった `failed` だけ
    /// （`task_ops::comment::can_rereview` と同じ規則。events を要るので `actions(task)` 単体では
    /// 判定できず、`task_detail` / 受信箱の `build_attention` が events を渡して個別に足す）。
    Rereview,
    /// ADR-0074 D2.4（Phase F3 途中確認）: 工程の後の途中確認に応える（`POST /tasks/{id}/execution/
    /// phase-gate` の continue / replan / withdraw）。`blocked(awaiting_human)` のときだけで、その間は
    /// `Answer` を出さない（events が要るので `actions_with_events` が足す）。
    PhaseGate,
    /// ADR-0079 D8（Phase R3b）: root の計画の承認に応える（`POST /tasks/{id}/execution/plan-gate` の
    /// approve / replan / withdraw）。`blocked(awaiting_plan_approval)` のときだけで、その間は `Answer` を出さない。
    PlanGate,
}

pub fn task_ref(task: &Task) -> TaskRef {
    TaskRef {
        id: task.id,
        title: task.title.clone(),
        kind: task.kind,
        status: task.status,
        actions: actions(task),
    }
}

/// `docs/api/v1/gui-api.md` §5.4: 今この状態で許される操作。
pub fn actions(task: &Task) -> Vec<Action> {
    let mut out = Vec::new();
    if task.status == Status::Draft
        || (task.kind == TaskKind::Approval && task.status == Status::Ready)
    {
        out.push(Action::Approve);
    }
    if task.kind == TaskKind::Approval && task.status == Status::Ready {
        out.push(Action::Reject);
    }
    if task.status == Status::Blocked {
        out.push(Action::Answer);
    }
    if !task.status.is_terminal() || task.status == Status::Failed {
        out.push(Action::Cancel);
    }
    if matches!(task.status, Status::Failed | Status::Cancelled) {
        out.push(Action::Retry);
    }
    // ADR-0044 D1/D2（Phase 53）: 編集は終端でないタスク、再開は `done`/`failed` だけ。
    if !task.status.is_terminal() {
        out.push(Action::Edit);
    }
    if matches!(task.status, Status::Done | Status::Failed) {
        out.push(Action::Reopen);
    }
    out
}

/// ADR-0070 D2（Phase 116）: [`actions`] に `Action::Rereview` を足したもの（events が要るので
/// 別関数にした。`task_detail` と受信箱の `build_attention` が、それぞれ既に読んでいる events を
/// 渡して使う）。
pub fn actions_with_events(task: &Task, events: &[(u64, Event)]) -> Vec<Action> {
    let mut out = actions(task);
    if crate::comment::can_rereview(task, events) {
        out.push(Action::Rereview);
    }
    if crate::phase_gate::is_awaiting_human(task, events) {
        out.retain(|a| *a != Action::Answer);
        out.push(Action::PhaseGate);
    }
    // ADR-0079 D8（Phase R3b）: root の計画の承認待ちは `plan-gate` の 3 つの操作だけ（`Answer` を出さない）。
    if crate::plan_gate::is_awaiting_plan_approval(task, events) {
        out.retain(|a| *a != Action::Answer);
        out.push(Action::PlanGate);
    }
    out
}

/// RFC 3339 文字列に整形する。`OffsetDateTime` の書式化が失敗することは実質無い想定だが、
/// パニックはしない（`Debug` 表現にフォールバックする）。
pub(crate) fn to_rfc3339(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap_or_else(|_| format!("{t:?}"))
}

fn std_duration_to_time_duration(d: Duration) -> time::Duration {
    time::Duration::new(d.as_secs() as i64, d.subsec_nanos() as i32)
}

/// `Status` の serde 表現（snake_case）。`counts_by_status` のキーに使う。
pub(crate) fn status_key(status: Status) -> &'static str {
    match status {
        Status::Draft => "draft",
        Status::Ready => "ready",
        Status::Running => "running",
        Status::Blocked => "blocked",
        Status::Reviewing => "reviewing",
        Status::Done => "done",
        Status::Failed => "failed",
        Status::Cancelled => "cancelled",
    }
}

/// `derive` の各関数（`(seq, Event)` の組を期待する）に渡すための写像。
pub(crate) fn seq_pairs(rows: &[EventRow]) -> Vec<(u64, Event)> {
    rows.iter().map(|r| (r.seq, r.event.clone())).collect()
}

fn lease_expires_at_str(task: &Task) -> Option<String> {
    if task.status != Status::Running {
        return None;
    }
    task.lease.as_ref().map(|l| to_rfc3339(l.expires_at))
}

/// `docs/api/v1/gui-api.md` §5.3: `ready && attempts > 0` のときの `updated_at + retry_backoff(...)`。
/// `base == 0`、または結果が過去なら `None`。
fn backoff_until_str(task: &Task, ctx: &ViewContext, now: OffsetDateTime) -> Option<String> {
    if task.status != Status::Ready || task.attempts == 0 {
        return None;
    }
    let backoff =
        derive::retry_backoff(ctx.retry_backoff_base, ctx.retry_backoff_max, task.attempts);
    if backoff.is_zero() {
        return None;
    }
    let until = task.updated_at + std_duration_to_time_duration(backoff);
    if until > now {
        Some(to_rfc3339(until))
    } else {
        None
    }
}

/// `parent_id` ごとの `(children, pending_children)`（`pending` = 非終端）。一覧と受信箱で全件を 1 回だけ走査するために使う。
pub(crate) fn child_counts(all_tasks: &[Task]) -> HashMap<TaskId, (u32, u32)> {
    let mut counts: HashMap<TaskId, (u32, u32)> = HashMap::new();
    for t in all_tasks {
        if let Some(parent) = t.parent_id {
            let entry = counts.entry(parent).or_insert((0, 0));
            entry.0 += 1;
            if !t.status.is_terminal() {
                entry.1 += 1;
            }
        }
    }
    counts
}

pub(crate) fn build_task_summary(
    task: &Task,
    children: u32,
    pending_children: u32,
    ctx: &ViewContext,
    now: OffsetDateTime,
) -> TaskSummary {
    TaskSummary {
        id: task.id,
        parent_id: task.parent_id,
        kind: task.kind,
        status: task.status,
        title: task.title.clone(),
        priority: task.priority,
        tier: task.worker_hint.tier,
        adapter: task.worker_hint.adapter.clone(),
        attempts: task.attempts,
        max_retries: task.budget.max_retries,
        depends_on: task.depends_on.clone(),
        created_at: to_rfc3339(task.created_at),
        updated_at: to_rfc3339(task.updated_at),
        lease_expires_at: lease_expires_at_str(task),
        backoff_until: backoff_until_str(task, ctx, now),
        children,
        pending_children,
        role: task.role.clone(),
        genre: task.genre.clone(),
        assignee: task.assignee.clone(),
        conversation: task_core::is_conversation(task),
        support: task_core::support_kind(task).map(str::to_string),
        is_root_task: task_core::is_root_task(task),
        paused: task.paused_at.is_some(),
        actions: actions(task),
        labels: task.labels.clone(),
        category: task.category,
        priority_label: task_core::priority_label(task.priority).to_string(),
        project_id: task.project_id,
        milestone_id: task.milestone_id,
    }
}

/// `outcome` 文字列を `RunOutcomeKind` に分類する（`docs/api/v1/gui-api.md` §5.2）。`outcome_text` は
/// 接頭辞を除いた残りの文字列（`lease_expired` は完全一致で残りが無いので `None`。`error` は元の
/// 文字列全体を `outcome_text` に入れる。GUI が生の理由を表示できるようにするための判断）。
///
/// ADR-0072 D19（Phase E1）: `end`（`WorkerFinished.end`。構造化された分類）があれば、それを
/// 優先する。`end` が無い run（導入前・分類できなかった経路）は従来どおり字句判定にフォールバックする。
fn classify_outcome(
    outcome: &str,
    end: Option<&task_core::RunEnd>,
) -> (RunOutcomeKind, Option<String>) {
    if let Some(task_core::RunEnd::Yielded | task_core::RunEnd::BudgetExhausted { .. }) = end
        && let Some(text) = outcome.strip_prefix("continue: ")
    {
        // continuation した（`[execution] continuation = false` や上限到達で従来の `error(...)`/
        // `question: ...` に戻ったときは、その文字列どおりに分類する）。
        return (RunOutcomeKind::Continued, Some(text.to_string()));
    }
    if let Some(text) = outcome.strip_prefix("done: ") {
        (RunOutcomeKind::Done, Some(text.to_string()))
    } else if let Some(text) = outcome.strip_prefix("question: ") {
        (RunOutcomeKind::Question, Some(text.to_string()))
    } else if let Some(text) = outcome.strip_prefix("continue: ") {
        (RunOutcomeKind::Continued, Some(text.to_string()))
    } else if let Some(text) = outcome.strip_prefix("requeue: ") {
        (RunOutcomeKind::Requeue, Some(text.to_string()))
    } else if let Some(text) = outcome.strip_prefix("infra_requeue: ") {
        // ADR-0070 D3 / P-E0-3: インフラ都合の再試行も `Requeue` に数える（attempts を消費しない
        // 再試行という点で供給側の `requeue` と同じ性質）。
        (RunOutcomeKind::Requeue, Some(text.to_string()))
    } else if let Some(text) = outcome.strip_prefix("interrupted: ") {
        // ADR-0044 D2/D8: 人のコメントによる割り込み（失敗ではない）。
        (RunOutcomeKind::Interrupted, Some(text.to_string()))
    } else if outcome == "lease_expired" {
        (RunOutcomeKind::LeaseExpired, None)
    } else {
        (RunOutcomeKind::Error, Some(outcome.to_string()))
    }
}

/// `docs/api/v1/gui-api.md` §5.2: そのタスクのイベント（`event_rows_for`）から run の要約を組み立てる。
pub fn runs(rows: &[EventRow]) -> Vec<RunSummary> {
    let mut order: Vec<String> = Vec::new();
    let mut by_run: HashMap<String, RunSummary> = HashMap::new();

    for row in rows {
        match &row.event {
            Event::WorkerStarted {
                run_id,
                adapter,
                model,
                provider,
                account,
                role,
                ..
            } => {
                if !by_run.contains_key(run_id) {
                    order.push(run_id.clone());
                }
                by_run.entry(run_id.clone()).or_insert_with(|| RunSummary {
                    run_id: run_id.clone(),
                    role: role.unwrap_or(RunRole::Worker),
                    adapter: adapter.clone(),
                    model: model.clone(),
                    provider: provider.clone(),
                    account: account.clone(),
                    started_at: row.ts.clone(),
                    finished_at: None,
                    outcome: None,
                    outcome_text: None,
                    end: None,
                    work_unit: None,
                    usage: None,
                    progress: 0,
                    artifacts: 0,
                    verdicts: 0,
                    reviewer_deferrals: 0,
                    files: None,
                });
            }
            Event::WorkerProgress { run_id, msg, .. } => {
                if let Some(r) = by_run.get_mut(run_id) {
                    r.progress += 1;
                    if msg.starts_with(derive::REVIEWER_REQUEUED_PREFIX) {
                        r.reviewer_deferrals += 1;
                    }
                }
            }
            Event::ArtifactProduced { run_id, .. } => {
                if let Some(r) = by_run.get_mut(run_id) {
                    r.artifacts += 1;
                }
            }
            Event::ReviewVerdict { run_id, .. } => {
                if let Some(r) = by_run.get_mut(run_id) {
                    r.verdicts += 1;
                }
            }
            Event::WorkerFinished {
                run_id,
                outcome,
                usage,
                end,
                ..
            } => {
                if let Some(r) = by_run.get_mut(run_id) {
                    r.finished_at = Some(row.ts.clone());
                    r.usage = *usage;
                    r.end = *end;
                    let (kind, text) = classify_outcome(outcome, end.as_ref());
                    r.outcome = Some(kind);
                    r.outcome_text = text;
                }
            }
            _ => {}
        }
    }

    let mut out: Vec<RunSummary> = order
        .into_iter()
        .filter_map(|id| by_run.remove(&id))
        .collect();
    out.sort_by(|a, b| a.started_at.cmp(&b.started_at));
    out
}

/// ADR-0072 D20（Phase E5）: `run_id` → WorkUnit の `key`。`work_units`/`runs` の派生索引から
/// 引く（計画の無い Task なら空の map）。`runs()` の後段で `RunSummary.work_unit` を埋めるのに使う。
pub fn run_work_unit_keys(
    store: &dyn TaskStore,
    task_id: TaskId,
) -> Result<HashMap<String, String>, OpsError> {
    let units = store.work_units_for(task_id)?;
    if units.is_empty() {
        return Ok(HashMap::new());
    }
    let key_by_id: HashMap<&str, &str> = units
        .iter()
        .map(|u| (u.id.as_str(), u.key.as_str()))
        .collect();
    let runs = store.runs_for_task(task_id)?;
    Ok(runs
        .into_iter()
        .filter_map(|r| {
            let wu_id = r.work_unit_id?;
            let key = key_by_id.get(wu_id.as_str())?;
            Some((r.run_id, (*key).to_string()))
        })
        .collect())
}

/// `docs/api/v1/gui-api.md` §5.3。
pub fn timers(task: &Task, rows: &[EventRow], ctx: &ViewContext, now: OffsetDateTime) -> Timers {
    let events = seq_pairs(rows);
    Timers {
        now: to_rfc3339(now),
        lease_expires_at: lease_expires_at_str(task),
        backoff_until: backoff_until_str(task, ctx, now),
        consecutive_requeues: derive::consecutive_requeues(&events),
        max_requeues: ctx.max_requeues,
        consecutive_reviewer_requeues: derive::consecutive_reviewer_requeues(&events),
    }
}

/// `docs/api/v1/gui-api.md` §3.3。
pub fn task_summary(
    store: &dyn TaskStore,
    task: &Task,
    ctx: &ViewContext,
    now: OffsetDateTime,
) -> Result<TaskSummary, OpsError> {
    let (children, pending_children) = child_counts(&store.list(None)?)
        .get(&task.id)
        .copied()
        .unwrap_or((0, 0));
    Ok(build_task_summary(
        task,
        children,
        pending_children,
        ctx,
        now,
    ))
}

/// `docs/api/v1/gui-api.md` §3.3: `list_page` の結果を `TaskSummary` に写し、`counts_by_status` を付ける。
pub fn task_list(
    store: &dyn TaskStore,
    filter: &ListFilter,
    order: ListOrder,
    cursor: Option<&str>,
    limit: usize,
    ctx: &ViewContext,
    now: OffsetDateTime,
) -> Result<TaskList, OpsError> {
    let page = store.list_page(filter, order, cursor, limit)?;

    // `children` / `pending_children` は `parent_id` で集計する。数千件を想定し、`list(None)` を
    // 1 回読んで全ページ分の項目に共通のカウントマップを使う（項目ごとに全件走査しない）。
    let child_counts = child_counts(&store.list(None)?);

    let items = page
        .items
        .iter()
        .map(|t| {
            let (children, pending_children) = child_counts.get(&t.id).copied().unwrap_or((0, 0));
            build_task_summary(t, children, pending_children, ctx, now)
        })
        .collect();

    let counts_by_status = store
        .count_by_status()?
        .into_iter()
        .map(|(s, n)| (status_key(s).to_string(), n))
        .collect();

    Ok(TaskList {
        items,
        next_cursor: page.next_cursor,
        total: page.total,
        counts_by_status,
    })
}

/// `docs/api/v1/gui-api.md` §3.5。
pub fn task_detail(
    store: &dyn TaskStore,
    id: TaskId,
    ctx: &ViewContext,
    now: OffsetDateTime,
) -> Result<TaskDetail, OpsError> {
    let task = store.get(id)?.ok_or(OpsError::NotFound(id))?;
    let rows = store.event_rows_for(id, None, ALL_EVENTS)?;
    let events = seq_pairs(&rows);

    let all_tasks = store.list(None)?;
    let by_id: HashMap<TaskId, Task> = all_tasks.iter().map(|t| (t.id, t.clone())).collect();

    let mut children_refs: Vec<TaskRef> = Vec::new();
    let mut approval_children: Vec<Task> = Vec::new();
    for t in &all_tasks {
        if t.parent_id == Some(id) {
            children_refs.push(task_ref(t));
            if t.kind == TaskKind::Approval {
                approval_children.push(t.clone());
            }
        }
    }
    children_refs.sort_by_key(|r| r.id);
    approval_children.sort_by_key(|t| t.id);

    // Human check の Approval 子。`parse_human_approval_title` で `(criterion_idx, attempt)` に対応付ける。
    let mut approvals: Vec<ApprovalLink> = Vec::with_capacity(approval_children.len());
    for child in &approval_children {
        let child_rows = store.event_rows_for(child.id, None, ALL_EVENTS)?;
        let decided = child_rows.iter().rev().find_map(|r| match &r.event {
            Event::ApprovalDecided { by, approved, note } => Some(ApprovalDecisionView {
                by: by.clone(),
                approved: *approved,
                note: note.clone(),
                ts: r.ts.clone(),
            }),
            _ => None,
        });
        let parsed = parse_human_approval_title(&child.title);
        approvals.push(ApprovalLink {
            approval: task_ref(child),
            criterion_idx: parsed.map(|(i, _)| i),
            attempt: parsed.map(|(_, a)| a),
            decided,
        });
    }

    let last_run = derive::last_run_id(&events);
    let criteria: Vec<CriterionView> = task
        .acceptance
        .iter()
        .enumerate()
        .map(|(idx, criterion)| {
            let latest_verdict = last_run.as_deref().and_then(|run_id| {
                rows.iter().rev().find_map(|r| match &r.event {
                    Event::ReviewVerdict {
                        run_id: rid,
                        criterion_idx,
                        pass,
                        reason,
                    } if rid == run_id && *criterion_idx == idx => Some(VerdictView {
                        run_id: rid.clone(),
                        criterion_idx: idx,
                        pass: *pass,
                        reason: reason.clone(),
                        ts: r.ts.clone(),
                    }),
                    _ => None,
                })
            });
            // 同じ criterion_idx の Approval 子が複数（再レビューで複数回）あれば、最新の attempt を使う。
            let approval = approvals
                .iter()
                .filter(|a| a.criterion_idx == Some(idx))
                .max_by_key(|a| a.attempt.unwrap_or(0))
                .cloned();
            CriterionView {
                idx,
                text: criterion.text.clone(),
                check: criterion.check.clone(),
                latest_verdict,
                approval,
            }
        })
        .collect();

    let dependencies: Vec<TaskRef> = task
        .depends_on
        .iter()
        .filter_map(|d| by_id.get(d).map(task_ref))
        .collect();
    let mut dependents: Vec<TaskRef> = all_tasks
        .iter()
        .filter(|t| t.depends_on.contains(&id))
        .map(task_ref)
        .collect();
    dependents.sort_by_key(|r| r.id);

    let mut run_summaries = runs(&rows);
    let wu_keys_by_run = run_work_unit_keys(store, id)?;
    for r in &mut run_summaries {
        r.work_unit = wu_keys_by_run.get(&r.run_id).cloned();
    }
    let timers_view = timers(&task, &rows, ctx, now);
    let prior_review = derive::prior_review_from_events(&events);
    let answers = derive::answers_from_events(&events);
    let latest_question_raw = derive::latest_question(&events);
    let latest_question = if latest_question_raw.is_empty() {
        None
    } else {
        Some(latest_question_raw)
    };

    // ADR-0041 D1: worktree を切ったタスクでは、run のログ・成果物は作業ツリーの外
    // （`<workspace_root>/<task_id>/`）にある。判定は `workspace::local_dir`（目印ファイルを見るだけ）。
    let workspace_dir = Some(
        crate::workspace::local_dir(&task, &ctx.workspace_root)
            .to_string_lossy()
            .into_owned(),
    );
    let cluster = match &task.workspace {
        WorkspaceSpec::Local { .. } => None,
        WorkspaceSpec::Remote { cluster, .. } => Some(cluster.clone()),
    };
    // ADR-0019 D2: `sync = "worktree"` のクラスタなら、worktree のパスとブランチを出す（人が diff / commit する場所）。
    let worktree = match &task.workspace {
        WorkspaceSpec::Remote { cluster, path, .. } => ctx
            .clusters
            .get(cluster)
            .filter(|c| c.sync == "worktree")
            .map(|c| {
                let root = c
                    .worktree_root
                    .clone()
                    .unwrap_or_else(|| path.join(".celeris-worktrees"));
                WorktreeView {
                    project: path.to_string_lossy().into_owned(),
                    dir: root
                        .join(task.id.to_string())
                        .to_string_lossy()
                        .into_owned(),
                    branch: format!("{WORKTREE_BRANCH_PREFIX}{}", task.id),
                }
            }),
        // ADR-0041 D1: ローカルも worktree を切る。目印（`worktree.json`）があればそれを出す。
        WorkspaceSpec::Local { path, .. } => {
            crate::workspace::read_marker(&ctx.workspace_root.join(task.id.to_string())).map(|m| {
                WorktreeView {
                    project: if m.repo.is_empty() {
                        path.to_string_lossy().into_owned()
                    } else {
                        m.repo
                    },
                    dir: m.dir,
                    branch: m.branch,
                }
            })
        }
    };

    let task_actions = actions_with_events(&task, &events);
    let failure = task_failure(&task, &events, store)?;
    let execution = build_execution_view(store, &task, &events, now)?;
    let worker_run_hint = if task.status.is_terminal() {
        None
    } else {
        Some(format!(
            "celerisctl worker run --config <config.toml> --task {id}"
        ))
    };

    let delegated: Vec<DelegatedView> = rows
        .iter()
        .filter_map(|r| match &r.event {
            Event::Delegated { run_id, task_ids } => Some(DelegatedView {
                run_id: run_id.clone(),
                ts: r.ts.clone(),
                tasks: task_ids
                    .iter()
                    .filter_map(|tid| by_id.get(tid).map(task_ref))
                    .collect(),
            }),
            _ => None,
        })
        .collect();
    let role = task.role.clone();
    let genre = task.genre.clone();
    let priority_label = task_core::priority_label(task.priority).to_string();
    let is_root_task = task_core::is_root_task(&task);
    let paused_by = paused_by(store, &task)?;
    let cluster_job_wait = active_cluster_job_wait(store, task.id)?;
    let integration_repair = integration_repair_view(&events);
    let expected_write_paths = store.effective_task_write_paths(id)?;
    let mut actual_run_write_sets = Vec::new();
    for run in store.runs_for_task(id)? {
        actual_run_write_sets.extend(
            store
                .run_write_sets(&run.run_id)?
                .into_iter()
                .map(ActualWriteSetView::from),
        );
    }
    let mut actual_work_unit_write_sets = Vec::new();
    for unit in store.work_units_for(id)? {
        actual_work_unit_write_sets.extend(
            store
                .work_unit_write_sets(&unit.id)?
                .into_iter()
                .map(ActualWriteSetView::from),
        );
    }
    let behind_target = crate::behind_target::behind_target_of(store, id, now)?;

    Ok(TaskDetail {
        expected_write_paths,
        actual_run_write_sets,
        actual_work_unit_write_sets,
        behind_target,
        is_root_task,
        paused_by,
        cluster_job_wait,
        integration_repair,
        task,
        priority_label,
        workspace_dir,
        cluster,
        role,
        genre,
        delegated,
        timers: timers_view,
        criteria,
        runs: run_summaries,
        prior_review,
        answers,
        latest_question,
        approvals,
        dependencies,
        dependents,
        children: children_refs,
        actions: task_actions,
        worker_run_hint,
        worktree,
        failure,
        execution,
    })
}

/// ADR-0079 D13（Phase R5a）: `task` の dispatch を subtree の一時停止で止めている task（自分か、`paused_at` を持つ
/// 一番近い祖先。`parent_id`、無ければ `tree.parent_unit` を辿る。`TaskStore::ready_tasks` と同じ鎖）。
pub fn paused_by(store: &dyn TaskStore, task: &Task) -> Result<Option<TaskId>, OpsError> {
    const MAX_ANCESTRY: usize = 32;
    let parent_of = |t: &Task| {
        t.parent_id.or_else(|| {
            t.tree
                .as_ref()
                .and_then(|tree| tree.parent_unit.as_ref())
                .map(|u| u.task_id)
        })
    };
    if task.paused_at.is_some() {
        return Ok(Some(task.id));
    }
    let mut seen = std::collections::HashSet::new();
    let mut next = parent_of(task);
    while let Some(id) = next {
        if !seen.insert(id) || seen.len() > MAX_ANCESTRY {
            break;
        }
        let Some(ancestor) = store.get(id)? else {
            break;
        };
        if ancestor.paused_at.is_some() {
            return Ok(Some(ancestor.id));
        }
        next = parent_of(&ancestor);
    }
    Ok(None)
}

/// ADR-0070 D1（Phase 116）: `task.status == Failed` のときだけ `Some`。分類・理由 1 行・
/// 配送済みなら release の sha12 を持つ。`deliveries` の読み取りは `TaskDetail` の他のフィールドと
/// 同じくストアから 1 回読むだけ（LLM は使わない）。
fn task_failure(
    task: &Task,
    events: &[(u64, Event)],
    store: &dyn TaskStore,
) -> Result<Option<FailureSummary>, OpsError> {
    if task.status != Status::Failed {
        return Ok(None);
    }
    let (class, reason) = derive::classify_task_failure(events);
    let delivered_release = store.delivery_get(task.id)?.and_then(|d| d.release.clone());
    Ok(Some(FailureSummary {
        class,
        reason,
        delivered_release,
    }))
}

/// ADR-0079 D11（Phase R4a）: Execution 節の段階（[`ExecutionPhase`]）だけ（木の節点の表示用。
/// `GET /tasks/{id}/task-tree`）。Execution 節を持たない task は `None`。
pub fn execution_phase_of(
    store: &dyn TaskStore,
    task: &Task,
    events: &[(u64, Event)],
) -> Result<Option<ExecutionPhase>, OpsError> {
    Ok(build_execution_view(store, task, events, OffsetDateTime::now_utc())?.and_then(|v| v.phase))
}

/// ADR-0072 D19/D20（Phase E5）: Execution 節の組み立て。events に E-phase 由来の活動が 1 件も
/// 無ければ `None`（D23 の後方互換。既存の古いタスクの詳細を壊さない）。
fn build_execution_view(
    store: &dyn TaskStore,
    task: &Task,
    events: &[(u64, Event)],
    now: OffsetDateTime,
) -> Result<Option<ExecutionView>, OpsError> {
    let has_activity = events.iter().any(|(_, e)| {
        matches!(
            e,
            Event::ExecutionGated { .. }
                // ADR-0072「Phase F6 実装時の決定」: 人が後から実行の形を決めた（gate の判定は
                // 次の dispatch まで空になるが、節は出す）。
                | Event::ExecutionHintSet { .. }
                | Event::ExecutionPlanned { .. }
                | Event::CheckpointSaved { .. }
                | Event::WorkUnitTransitioned { .. }
                // ADR-0074 D2.3（Phase F3 途中確認）
                | Event::PhaseReported { .. }
                // ADR-0079 D8（Phase R3b）: root の計画の承認待ち
                | Event::PlanApprovalRequested { .. }
                // ADR-0124: 直行経路の判定
                | Event::ExecutionRouted { .. }
        )
    });
    if !has_activity {
        return Ok(None);
    }

    let event_list: Vec<Event> = events.iter().map(|(_, e)| e.clone()).collect();
    // ADR-0130 D4: behind は store の最後の snapshot（読取時に Git を測り直さない）。
    let metrics = task_core::summarize_execution_metrics(task, &event_list).with_behind_target(
        &crate::behind_target::behind_target_of(store, task.id, now)?,
    );
    let gate = task.routing.as_ref().and_then(|r| r.execution.clone());
    let route = task.routing.as_ref().and_then(|r| r.route.clone());

    let all_units = store.work_units_for(task.id)?;
    let active_units: Vec<&task_core::WorkUnitRow> =
        all_units.iter().filter(|u| u.status.is_active()).collect();

    let phase = execution_phase(task, &active_units);

    let plan = if active_units.is_empty() {
        None
    } else {
        let audits = task_core::routing_audit(task, &event_list);
        let audit_by_run: HashMap<&str, &task_core::RoutingAudit> =
            audits.iter().map(|a| (a.run_id.as_str(), a)).collect();
        let mut work_units = Vec::with_capacity(active_units.len());
        for u in &active_units {
            let last_audit = u
                .last_run_id
                .as_deref()
                .and_then(|rid| audit_by_run.get(rid).copied());
            let last_checkpoint = match &u.last_checkpoint_run_id {
                Some(run_id) => store
                    .run_index_get(run_id)?
                    .and_then(|r| r.checkpoint.clone()),
                None => None,
            };
            let last_reason = events.iter().rev().find_map(|(_, e)| match e {
                Event::WorkUnitTransitioned {
                    work_unit_id,
                    reason,
                    ..
                } if *work_unit_id == u.id => Some(reason.clone()),
                _ => None,
            });
            work_units.push(ExecutionWorkUnitView {
                id: u.id.clone(),
                expected_write_paths: store.work_unit_expected_write_paths(&u.id)?,
                key: u.key.clone(),
                seq: u.seq,
                kind: u.kind,
                title: u.spec.title.clone(),
                status: u.status,
                blocked_reason: u.blocked_reason,
                depends_on: u.depends_on.clone(),
                assignee: task.assignee.clone(),
                harness: u.spec.harness.clone().or_else(|| task.genre.clone()),
                model: last_audit.and_then(|a| a.model.clone()),
                lane: last_audit.and_then(|a| a.lane),
                runs: u.runs,
                continuations: u.continuations,
                retries: u.retries,
                last_checkpoint,
                last_reason,
                created_at: u.created_at.clone(),
                updated_at: u.updated_at.clone(),
                phase: u.phase.clone(),
                branch: u.branch.clone(),
                child_task_id: u.child_task_id.clone(),
                head_commit: u.head_commit.clone(),
                integrated_commit: u.integrated_commit.clone(),
                running_run_id: if u.status == task_core::WorkUnitStatus::Running {
                    u.lease_run_id.clone()
                } else {
                    None
                },
            });
        }
        work_units.sort_by_key(|w| w.seq);

        let versions = store
            .execution_plan_list(task.id)?
            .into_iter()
            .map(|row| {
                let reason = event_list.iter().find_map(|e| match e {
                    Event::ExecutionPlanned {
                        plan_id, reason, ..
                    } if *plan_id == row.id => reason.clone(),
                    _ => None,
                });
                ExecutionPlanVersionSummary {
                    id: row.id,
                    version: row.version,
                    origin: row.origin,
                    status: row.status,
                    reason,
                    created_at: row.created_at,
                    superseded_at: row.superseded_at,
                }
            })
            .collect();

        let active_plan_row = store.execution_plan_active(task.id)?;
        let serialized_reason_of = |plan_id: &str| {
            event_list.iter().rev().find_map(|e| match e {
                Event::WorkUnitsSerialized { plan_id: p, reason } if p == plan_id => {
                    Some(reason.clone())
                }
                _ => None,
            })
        };
        match active_plan_row {
            Some(row) => {
                let serialized_reason = serialized_reason_of(&row.id);
                // ADR-0079（Phase R1b）: /3 の段階は /2 の工程の形で見せる（`internal_view`）。
                let phases = task_core::internal_view(&row.spec).phases.clone();
                Some(ExecutionPlanOverview {
                    id: row.id,
                    version: row.version,
                    origin: row.origin,
                    rationale: row.spec.rationale,
                    work_units,
                    versions,
                    phases,
                    serialized_reason,
                })
            }
            None => {
                let latest = versions.last().cloned();
                latest.map(|latest| ExecutionPlanOverview {
                    serialized_reason: serialized_reason_of(&latest.id),
                    id: latest.id,
                    version: latest.version,
                    origin: latest.origin,
                    rationale: String::new(),
                    work_units,
                    versions,
                    phases: Vec::new(),
                })
            }
        }
    };

    let phase_checkpoint =
        crate::phase_gate::latest_phase_checkpoint(task, events).map(|info| PhaseCheckpointView {
            report: info.report,
            report_idx: info.report_idx,
        });
    let phase = if phase_checkpoint.is_some() {
        Some(ExecutionPhase::AwaitingHuman)
    } else {
        phase
    };
    // ADR-0079 D8（Phase R3b）: root の計画の承認待ち。
    let plan_approval =
        crate::plan_gate::latest_plan_approval(task, events).map(|info| PlanApprovalView {
            summary: crate::plan_gate::describe_reasons(&info.reasons),
            plan_id: info.plan_id,
            reasons: info.reasons,
        });
    let phase = if plan_approval.is_some() {
        Some(ExecutionPhase::AwaitingPlanApproval)
    } else {
        phase
    };
    // ADR-0079 D5（Phase R1b）: 子 task だけを待っている親の理由（待っている子の題名と状態）。
    let mut awaiting_children = Vec::new();
    if phase == Some(ExecutionPhase::AwaitingChildren) {
        for u in active_units.iter().filter(|u| {
            u.kind == task_core::WorkUnitKind::Task
                && !u.status.is_terminal()
                && u.status != task_core::WorkUnitStatus::Pending
        }) {
            let child = match u
                .child_task_id
                .as_deref()
                .and_then(|s| s.parse::<TaskId>().ok())
            {
                Some(id) => store.get(id)?,
                None => None,
            };
            awaiting_children.push(AwaitedChildView {
                unit_key: u.key.clone(),
                task_id: child.as_ref().map(|c| c.id),
                title: child
                    .as_ref()
                    .map(|c| c.title.clone())
                    .unwrap_or_else(|| u.spec.title.clone()),
                status: child.as_ref().map(|c| c.status),
            });
        }
    }
    Ok(Some(ExecutionView {
        gate,
        route,
        phase,
        plan,
        metrics,
        phase_checkpoint,
        awaiting_children,
        plan_approval,
    }))
}

/// D20: 今どの段階か（[`ExecutionPhase`] のドキュメント参照）。
fn execution_phase(
    task: &Task,
    active_units: &[&task_core::WorkUnitRow],
) -> Option<ExecutionPhase> {
    match task.status {
        Status::Reviewing => Some(ExecutionPhase::Verifying),
        // ADR-0079 D5（Phase R1b）: 走れる自分の leaf が無く、子 task（kind task の unit）だけを待つ。
        Status::Ready if awaits_only_children(active_units) => {
            Some(ExecutionPhase::AwaitingChildren)
        }
        Status::Running => {
            if active_units.is_empty() {
                None
            } else if let Some(u) = active_units
                .iter()
                .find(|u| u.status == task_core::WorkUnitStatus::Running)
            {
                if u.kind == task_core::WorkUnitKind::Repair {
                    Some(ExecutionPhase::Repairing)
                } else {
                    Some(ExecutionPhase::Executing)
                }
            } else {
                Some(ExecutionPhase::Planning)
            }
        }
        _ => None,
    }
}

/// ADR-0079 D5（Phase R1b）: 非終端の kind task の unit（子を待つ・子の生成を待つ）があり、leaf（統合 WU を
/// 除く）に走れる・走っているものが無い。
fn awaits_only_children(active_units: &[&task_core::WorkUnitRow]) -> bool {
    let waiting_on_child = active_units.iter().any(|u| {
        u.kind == task_core::WorkUnitKind::Task
            && matches!(
                u.status,
                task_core::WorkUnitStatus::Ready | task_core::WorkUnitStatus::Running
            )
    });
    let leaf_runnable = active_units.iter().any(|u| {
        !matches!(
            u.kind,
            task_core::WorkUnitKind::Task | task_core::WorkUnitKind::Integrate
        ) && matches!(
            u.status,
            task_core::WorkUnitStatus::Ready
                | task_core::WorkUnitStatus::NeedsContinuation
                | task_core::WorkUnitStatus::Running
        )
    });
    waiting_on_child && !leaf_runnable
}

/// `Approval needed: <title> — criterion <idx> (attempt <n>)`（`derive::human_approval_title` の書式）を解析して
/// `(criterion_idx, attempt)` を返す。書式に合わなければ `None`。
///
/// `<title>` 自体に ` — criterion ` を含む可能性があるため、マーカーは**末尾から**（`rfind`）探す
/// （実際の構造上のマーカーは常に最後に現れるものになる）。
pub fn parse_human_approval_title(title: &str) -> Option<(usize, u32)> {
    const PREFIX: &str = "Approval needed: ";
    const MARKER: &str = " — criterion ";

    if !title.starts_with(PREFIX) {
        return None;
    }
    let marker_pos = title.rfind(MARKER)?;
    let rest = &title[marker_pos + MARKER.len()..];
    let (idx_str, remainder) = rest.split_once(" (attempt ")?;
    let attempt_str = remainder.strip_suffix(')')?;
    let idx = idx_str.parse::<usize>().ok()?;
    let attempt = attempt_str.parse::<u32>().ok()?;
    Some((idx, attempt))
}

#[cfg(test)]
mod tests;
