//! Materialized work units, scheduling, and dependency transitions.

use super::*;

/// ADR-0074 D3.7（Phase F4b (f)）: `newly_ready` の一般化。`external_done` は満たされた外部の依存
/// （`child:<key>` のうち子 Task が `done` のもの）。`child:` の依存は `external_done` にあれば満たす。
pub fn newly_ready_with(units: &[WorkUnitRow], external_done: &BTreeSet<String>) -> Vec<String> {
    let done: BTreeSet<&str> = units
        .iter()
        .filter(|u| u.status == WorkUnitStatus::Done)
        .map(|u| u.key.as_str())
        .chain(external_done.iter().map(String::as_str))
        .collect();
    let ranks = phase_ranks(units);
    units
        .iter()
        .filter(|u| u.status == WorkUnitStatus::Pending)
        .filter(|u| u.kind != WorkUnitKind::Integrate)
        .filter(|u| u.depends_on.iter().all(|d| done.contains(d.as_str())))
        .filter(|u| earlier_phases_done(units, &ranks, u))
        .map(|u| u.id.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// D6: WorkUnit / Run の状態
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkUnitStatus {
    Pending,
    Ready,
    NeedsContinuation,
    Running,
    Done,
    Failed,
    Blocked,
    Superseded,
    Cancelled,
}

impl WorkUnitStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkUnitStatus::Pending => "pending",
            WorkUnitStatus::Ready => "ready",
            WorkUnitStatus::NeedsContinuation => "needs_continuation",
            WorkUnitStatus::Running => "running",
            WorkUnitStatus::Done => "done",
            WorkUnitStatus::Failed => "failed",
            WorkUnitStatus::Blocked => "blocked",
            WorkUnitStatus::Superseded => "superseded",
            WorkUnitStatus::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(WorkUnitStatus::Pending),
            "ready" => Some(WorkUnitStatus::Ready),
            "needs_continuation" => Some(WorkUnitStatus::NeedsContinuation),
            "running" => Some(WorkUnitStatus::Running),
            "done" => Some(WorkUnitStatus::Done),
            "failed" => Some(WorkUnitStatus::Failed),
            "blocked" => Some(WorkUnitStatus::Blocked),
            "superseded" => Some(WorkUnitStatus::Superseded),
            "cancelled" => Some(WorkUnitStatus::Cancelled),
            _ => None,
        }
    }

    /// この状態が「まだ計画の実行に関わる」か（superseded/cancelled は外れる）。
    pub fn is_active(self) -> bool {
        !matches!(self, WorkUnitStatus::Superseded | WorkUnitStatus::Cancelled)
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            WorkUnitStatus::Done | WorkUnitStatus::Superseded | WorkUnitStatus::Cancelled
        )
    }
}

/// D6: `work_units.blocked_reason`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkUnitBlockedReason {
    Question,
    DependencyFailed,
    Limit,
    /// ADR-0072 D17 3.（Phase E4b 項目2）: worker の checkpoint/result.json が `plan_issue`
    /// （計画そのものが誤っているという 1 文の申告）を書いた。replan の余地があれば
    /// `Trigger::Continue{why: Replan}` で即座に Task を Ready へ戻す（Blocked のままにはしない）ので、
    /// この行が実際に `Task.status == Blocked` と一緒に残るのは replan の上限を使い切ったときだけ。
    PlanIssue,
    /// ADR-0079 D3 / D4 (3)（Phase R2a）: 人への決定の要求（`kind: limit` / `leaf_too_large`）を待つ。
    /// 木の上限を超える unit、子 task にできない深さの compound な leaf。同じ段階の他の unit・兄弟は
    /// 止めない（工程の失敗にも質問にも数えない）。回答で再開するのは R3a。
    Decision,
    /// ADR-0079 D9（Phase R2b）: kind task の unit の子が基盤の分類で失敗し、自動の作り直し
    /// （`MAX_CHILD_INFRA_RETRIES`）でも失敗した。障害通知を出し、人の再試行を待つ（質問でも決定でもない）。
    /// `Decision` と同じく工程の失敗に数えず、同じ段階の他の unit・兄弟は止めない（段階は完了しない）。
    Infra,
    /// ADR-0090 D2: この unit の run が `result.json` の `wait` でクラスタ job の終了を待っている。daemon の poll が
    /// すべての job の終了を見たら `needs_continuation` に戻す。`Decision` と同じく工程の失敗にも質問にも数えず、
    /// 同じ段階の他の unit・兄弟は止めない（段階は完了しない）。
    ClusterJobs,
}

impl WorkUnitBlockedReason {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkUnitBlockedReason::Question => "question",
            WorkUnitBlockedReason::DependencyFailed => "dependency_failed",
            WorkUnitBlockedReason::Limit => "limit",
            WorkUnitBlockedReason::PlanIssue => "plan_issue",
            WorkUnitBlockedReason::Decision => "decision",
            WorkUnitBlockedReason::Infra => "infra",
            WorkUnitBlockedReason::ClusterJobs => "cluster_jobs",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "question" => Some(WorkUnitBlockedReason::Question),
            "dependency_failed" => Some(WorkUnitBlockedReason::DependencyFailed),
            "limit" => Some(WorkUnitBlockedReason::Limit),
            "plan_issue" => Some(WorkUnitBlockedReason::PlanIssue),
            "decision" => Some(WorkUnitBlockedReason::Decision),
            "infra" => Some(WorkUnitBlockedReason::Infra),
            "cluster_jobs" => Some(WorkUnitBlockedReason::ClusterJobs),
            _ => None,
        }
    }
}

/// D5: `execution_plans.origin`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanOrigin {
    Planner,
    Human,
    Repair,
    Fixture,
}

impl PlanOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            PlanOrigin::Planner => "planner",
            PlanOrigin::Human => "human",
            PlanOrigin::Repair => "repair",
            PlanOrigin::Fixture => "fixture",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "planner" => Some(PlanOrigin::Planner),
            "human" => Some(PlanOrigin::Human),
            "repair" => Some(PlanOrigin::Repair),
            "fixture" => Some(PlanOrigin::Fixture),
            _ => None,
        }
    }
}

/// D5: `execution_plans.status`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Active,
    Superseded,
    Completed,
    Abandoned,
}

impl PlanStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            PlanStatus::Active => "active",
            PlanStatus::Superseded => "superseded",
            PlanStatus::Completed => "completed",
            PlanStatus::Abandoned => "abandoned",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "active" => Some(PlanStatus::Active),
            "superseded" => Some(PlanStatus::Superseded),
            "completed" => Some(PlanStatus::Completed),
            "abandoned" => Some(PlanStatus::Abandoned),
            _ => None,
        }
    }
}

/// D5: `runs.role`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunIndexRole {
    Worker,
    Reviewer,
    Planner,
    WrapUp,
}

impl RunIndexRole {
    pub fn as_str(self) -> &'static str {
        match self {
            RunIndexRole::Worker => "worker",
            RunIndexRole::Reviewer => "reviewer",
            RunIndexRole::Planner => "planner",
            RunIndexRole::WrapUp => "wrap_up",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "worker" => Some(RunIndexRole::Worker),
            "reviewer" => Some(RunIndexRole::Reviewer),
            "planner" => Some(RunIndexRole::Planner),
            "wrap_up" => Some(RunIndexRole::WrapUp),
            _ => None,
        }
    }
}

/// D5: `runs.status`（Run の終わり方。`RunEnd` とほぼ対応するが `running` を持つ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunIndexStatus {
    Running,
    Completed,
    Yielded,
    BudgetExhausted,
    Question,
    Failed,
    HarnessError,
    Cancelled,
    /// ADR-0090 D4: クラスタ job の wait で閉じた run（`running` ではないので R6-1 の照合は閉じ直さない）。
    Waiting,
}

impl RunIndexStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RunIndexStatus::Running => "running",
            RunIndexStatus::Completed => "completed",
            RunIndexStatus::Yielded => "yielded",
            RunIndexStatus::BudgetExhausted => "budget_exhausted",
            RunIndexStatus::Question => "question",
            RunIndexStatus::Failed => "failed",
            RunIndexStatus::HarnessError => "harness_error",
            RunIndexStatus::Cancelled => "cancelled",
            RunIndexStatus::Waiting => "waiting",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "running" => Some(RunIndexStatus::Running),
            "completed" => Some(RunIndexStatus::Completed),
            "yielded" => Some(RunIndexStatus::Yielded),
            "budget_exhausted" => Some(RunIndexStatus::BudgetExhausted),
            "question" => Some(RunIndexStatus::Question),
            "failed" => Some(RunIndexStatus::Failed),
            "harness_error" => Some(RunIndexStatus::HarnessError),
            "cancelled" => Some(RunIndexStatus::Cancelled),
            "waiting" => Some(RunIndexStatus::Waiting),
            _ => None,
        }
    }

    /// `RunEnd`（D7）から `runs.status` へ。
    pub fn from_run_end(end: crate::execution::RunEnd) -> Self {
        use crate::execution::RunEnd;
        match end {
            RunEnd::Completed => RunIndexStatus::Completed,
            RunEnd::Yielded => RunIndexStatus::Yielded,
            RunEnd::BudgetExhausted { .. } => RunIndexStatus::BudgetExhausted,
            RunEnd::Question => RunIndexStatus::Question,
            RunEnd::Failed { .. } => RunIndexStatus::Failed,
            RunEnd::HarnessError { .. } => RunIndexStatus::HarnessError,
            RunEnd::Cancelled => RunIndexStatus::Cancelled,
            RunEnd::Waiting => RunIndexStatus::Waiting,
        }
    }
}

// ---------------------------------------------------------------------------
// 派生の索引の行（D5）。永続化そのものは `task_core::store` が行う。
// ---------------------------------------------------------------------------

/// `execution_plans` の 1 行。
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutionPlanRow {
    pub id: String,
    pub task_id: String,
    pub version: u32,
    pub origin: PlanOrigin,
    pub planner_run_id: Option<String>,
    pub status: PlanStatus,
    pub spec: ExecutionPlanSpec,
    pub created_at: String,
    pub superseded_at: Option<String>,
}

/// `work_units` の 1 行。
#[derive(Debug, Clone, PartialEq)]
pub struct WorkUnitRow {
    pub id: String,
    pub task_id: String,
    pub plan_id: String,
    pub key: String,
    pub seq: u32,
    pub kind: WorkUnitKind,
    pub status: WorkUnitStatus,
    pub blocked_reason: Option<WorkUnitBlockedReason>,
    pub depends_on: Vec<String>,
    pub runs: u32,
    pub continuations: u32,
    pub retries: u32,
    pub last_run_id: Option<String>,
    pub last_checkpoint_run_id: Option<String>,
    pub spec: WorkUnitSpec,
    pub created_at: String,
    pub updated_at: String,
    /// ADR-0074 D1（Phase F2 / migration 0027）: v2 の工程の key（v1・atomic は `None`）。
    /// 普通の WU は `spec.phase` の写し、統合 WU（`integrate-<phase>`）は自分の工程。
    pub phase: Option<String>,
    /// ADR-0074 D1.5: この WU を今実行している run（`acquire_work_unit_lease`）。揮発（replay で比べない）。
    pub lease_run_id: Option<String>,
    /// ADR-0074 D1.5: 上の lease の期限（RFC 3339）。揮発。
    pub lease_expires_at: Option<String>,
    /// ADR-0074 D1.2: `celeris-wu/<task_id>/<key>`（WU の worktree を切ったときだけ）。
    pub branch: Option<String>,
    /// ADR-0074 D1.2: WU の worktree を切った基点 sha。
    pub base_commit: Option<String>,
    /// ADR-0074 D1.2: `WorkUnitCommitted` の commit（run が done になったときの決定的な commit）。
    pub head_commit: Option<String>,
    /// ADR-0074 D1.4: `PhaseIntegrated` の Task ブランチの HEAD（統合 WU の行だけ）。
    pub integrated_commit: Option<String>,
    /// ADR-0079 D4 (4) / D15（migration 0031）: kind task の unit の子 task（`ChildTaskCreated` /
    /// `ChildAdopted` の写し。leaf・統合 WU は `None`）。
    pub child_task_id: Option<String>,
    /// ADR-0079 D7（migration 0031）: この unit が回答を待つ決定の key（/3 の
    /// [`effective_needs_decisions`]。/1・/2 は空）。
    pub needs_decisions: Vec<String>,
}

impl WorkUnitRow {
    pub fn new(
        id: String,
        task_id: String,
        plan_id: String,
        seq: u32,
        spec: WorkUnitSpec,
        status: WorkUnitStatus,
        created_at: String,
    ) -> Self {
        WorkUnitRow {
            id,
            task_id,
            plan_id,
            key: spec.key.clone(),
            seq,
            kind: spec.kind,
            status,
            blocked_reason: None,
            depends_on: spec.depends_on.clone(),
            runs: 0,
            continuations: 0,
            retries: 0,
            last_run_id: None,
            last_checkpoint_run_id: None,
            phase: spec.phase.clone(),
            spec,
            created_at: created_at.clone(),
            updated_at: created_at,
            lease_run_id: None,
            lease_expires_at: None,
            branch: None,
            base_commit: None,
            head_commit: None,
            integrated_commit: None,
            child_task_id: None,
            needs_decisions: Vec::new(),
        }
    }

    /// ADR-0074 D1.5: WU の lease を外す（`running` を離れる遷移で呼ぶ）。
    pub fn clear_lease(&mut self) {
        self.lease_run_id = None;
        self.lease_expires_at = None;
    }
}

/// `runs` の 1 行。
#[derive(Debug, Clone, PartialEq)]
pub struct RunRow {
    pub run_id: String,
    pub task_id: String,
    pub work_unit_id: Option<String>,
    pub role: RunIndexRole,
    pub seq: u32,
    pub status: RunIndexStatus,
    pub adapter: Option<String>,
    pub model: Option<String>,
    pub account: Option<String>,
    pub session_id: Option<String>,
    pub checkpoint: Option<crate::execution::Checkpoint>,
    pub usage: Option<crate::model::Usage>,
    pub metrics: Option<crate::model::RunMetrics>,
    pub started_at: String,
    pub finished_at: Option<String>,
}

// ---------------------------------------------------------------------------
// D15: scheduler（決定的。ready queue・依存の伝播）
// ---------------------------------------------------------------------------

/// D15: 次に何をすべきか。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextStep {
    /// この WorkUnit（`work_units.id`）の Run を起こす。
    RunWorkUnit(String),
    /// Planner run を起こす（`replan` なら replan モード。E2 では発行しない）。
    RunPlanner {
        replan: bool,
    },
    AllDone,
    Stuck(String),
}

/// D15: `next_work_unit`。`needs_continuation` を優先し、次に `ready` を `seq` 順で選ぶ。
/// `units` は `superseded`/`cancelled` を含めてよい（無視する）。
pub fn next_work_unit(units: &[WorkUnitRow]) -> NextStep {
    let active: Vec<&WorkUnitRow> = units.iter().filter(|u| u.status.is_active()).collect();
    if active.is_empty() {
        return NextStep::Stuck("計画に有効な WorkUnit がありません".to_string());
    }
    if let Some(u) = active
        .iter()
        .filter(|u| u.status == WorkUnitStatus::NeedsContinuation)
        .min_by_key(|u| u.seq)
    {
        return NextStep::RunWorkUnit(u.id.clone());
    }
    if let Some(u) = active
        .iter()
        .filter(|u| u.status == WorkUnitStatus::Ready)
        .min_by_key(|u| u.seq)
    {
        return NextStep::RunWorkUnit(u.id.clone());
    }
    if active.iter().all(|u| u.status == WorkUnitStatus::Done) {
        return NextStep::AllDone;
    }
    if active
        .iter()
        .any(|u| matches!(u.status, WorkUnitStatus::Running))
    {
        // 直列実行（D6）なので、走っている WU があれば「次」は無い（呼ばれない想定）。
        return NextStep::Stuck("既に実行中の WorkUnit があります".to_string());
    }
    NextStep::Stuck(
        "実行できる WorkUnit がありません（blocked/failed のみ残っています）".to_string(),
    )
}

/// ADR-0074「F5-fix8 実装時の明確化」: 計画に残っている仕事が無い（有効な WorkUnit がすべて `done`、または
/// 有効な WorkUnit が 1 つも無い）か。統合 WU・repair WU も含めて見る（統合が済んでいない工程は `false`）。
pub fn plan_work_finished(units: &[WorkUnitRow]) -> bool {
    units
        .iter()
        .filter(|u| u.status.is_active())
        .all(|u| u.status == WorkUnitStatus::Done)
}

/// ADR-0074「F5-fix8 実装時の明確化」: `active` な計画（`plan_id`）が採用されてから、最終レビューの判定
/// （`review_pass` / `review_fail` / `review_repair` で `reviewing` を出た遷移）がまだ 1 度も無いか。
///
/// 仕事の残っていない計画について dispatcher が「最終レビューへ進める（`Trigger::PlanComplete`）」か
/// 「replan を試す（D17 4.）」かを分ける。採用の後に判定が無い = この版はまだ審査されていない（replan で
/// 何も足さなかった版を含む）ので、審査に出す。判定の後（不合格で `ready` に戻った）なら従来どおり replan。
/// 採用の event（`ExecutionPlanned{plan_id}`）が見つからなければ `false`（従来どおり）。純粋関数（LLM なし）。
pub fn plan_awaits_final_review(events: &[(u64, crate::model::Event)], plan_id: &str) -> bool {
    use crate::model::{Event, Status};
    for (_, event) in events.iter().rev() {
        match event {
            Event::ExecutionPlanned { plan_id: id, .. } if id == plan_id => return true,
            Event::Transitioned {
                from: Status::Reviewing,
                reason,
                ..
            } if matches!(
                reason.as_str(),
                "review_pass" | "review_fail" | "review_repair"
            ) =>
            {
                return false;
            }
            _ => {}
        }
    }
    false
}

/// ADR-0074 D1.3（Phase F2）: `next_work_unit` の一般化。工程の中で並列に何本まで起こせるかを
/// 決める（純粋関数。実際に走らせる・lease を取るのは呼び出し側の責務）。
///
/// - 対象は**現在の工程**（有効〈`!is_terminal()`〉な WorkUnit が残っている工程のうち、
///   `seq` が最も小さいもの）だけ。v1（`spec.phase` が常に `None`）は工程が実質 1 つなので
///   全部が対象になる（`limit = 1` と組み合わせると `next_work_unit` と同じ 1 件を返す）。
/// - `needs_continuation` を先に、次に `ready` を `seq` 順で選ぶ（`next_work_unit` と同じ順序）。
/// - `limit` から `in_flight`（呼び出し側がこの Task について現在走らせている run の数）を引いた
///   件数まで。
/// - 現在の工程に `failed`/`blocked` の WorkUnit があれば、**新しい**（`ready` の）WorkUnit は
///   起こさない。ただし既に走ったことのある `needs_continuation` の WorkUnit は続ける
///   （D1.6「走っている WU とその continuation だけは続ける」）。
pub fn runnable_work_units(units: &[WorkUnitRow], in_flight: usize, limit: usize) -> Vec<String> {
    let slots = limit.saturating_sub(in_flight);
    if slots == 0 {
        return Vec::new();
    }
    let in_play: Vec<&WorkUnitRow> = units.iter().filter(|u| !u.status.is_terminal()).collect();
    let Some(current) = in_play.iter().min_by_key(|u| u.seq) else {
        return Vec::new();
    };
    let current_phase = current.spec.phase.as_deref();
    // ADR-0074 D1.4（Phase F2b）: 統合 WU は LLM run を起こさない（scheduler が直接走らせる）。
    let in_phase: Vec<&WorkUnitRow> = in_play
        .into_iter()
        .filter(|u| u.spec.phase.as_deref() == current_phase)
        .filter(|u| u.kind != WorkUnitKind::Integrate)
        // ADR-0079（Phase R1a）: kind task の unit は子 task の代理で、LLM run を起こさない（R1b）。
        .filter(|u| u.kind != WorkUnitKind::Task)
        .collect();

    // ADR-0079 D5（Phase R2a）: 人への決定を待つ unit（`blocked(decision)`）は段階の完了を止めるが、同じ段階の
    // 他の unit は止めない（/1・/2 にこの理由は無いので従来どおり）。ADR-0090 D2: クラスタ job を待つ unit
    // （`blocked(cluster_jobs)`）も同じ段階の兄弟を止めない。
    let has_failed_or_blocked = in_phase.iter().any(|u| {
        u.status == WorkUnitStatus::Failed
            || (u.status == WorkUnitStatus::Blocked
                && !matches!(
                    u.blocked_reason,
                    Some(
                        WorkUnitBlockedReason::Decision
                            | WorkUnitBlockedReason::Infra
                            | WorkUnitBlockedReason::ClusterJobs
                    )
                ))
    });

    let mut candidates: Vec<&WorkUnitRow> = in_phase
        .iter()
        .copied()
        .filter(|u| u.status == WorkUnitStatus::NeedsContinuation)
        .collect();
    candidates.sort_by_key(|u| u.seq);

    if !has_failed_or_blocked {
        let mut ready: Vec<&WorkUnitRow> = in_phase
            .iter()
            .copied()
            .filter(|u| u.status == WorkUnitStatus::Ready)
            .collect();
        ready.sort_by_key(|u| u.seq);
        candidates.extend(ready);
    }

    candidates
        .into_iter()
        .take(slots)
        .map(|u| u.id.clone())
        .collect()
}

/// D15: 依存の解決。`depends_on` が全て `done` になった `pending` の WU を `ready` にする（`id` の集合を返す。
/// 呼び出し側が状態を書き換える）。
///
/// ADR-0074 D1.3（Phase F2b）: v2（`phase` のある行）では「前の工程の統合が done」を追加の条件にする
/// （工程の境は障壁。前の工程の有効な行〈統合 WU を含む〉がすべて `done` になるまで上げない）。
/// 統合 WU（`kind = integrate`）は `ready` にしない（scheduler が工程の完了を見て直接走らせる）。
/// v1（`phase` が無い行）は従来と同じ。
pub fn newly_ready(units: &[WorkUnitRow]) -> Vec<String> {
    let done: BTreeSet<&str> = units
        .iter()
        .filter(|u| u.status == WorkUnitStatus::Done)
        .map(|u| u.key.as_str())
        .collect();
    let ranks = phase_ranks(units);
    units
        .iter()
        .filter(|u| u.status == WorkUnitStatus::Pending)
        .filter(|u| u.kind != WorkUnitKind::Integrate)
        .filter(|u| u.depends_on.iter().all(|d| done.contains(d.as_str())))
        .filter(|u| earlier_phases_done(units, &ranks, u))
        .map(|u| u.id.clone())
        .collect()
}

/// ADR-0074 D1.3（Phase F2b）: 工程の key → 順位（その工程の行の `seq` の最小値。`topo_sort` が
/// 工程順を保証し、統合 WU は工程の末尾に置くので、`seq` の最小値で工程の順が引ける）。
pub fn phase_ranks(units: &[WorkUnitRow]) -> BTreeMap<String, u32> {
    let mut ranks: BTreeMap<String, u32> = BTreeMap::new();
    for u in units.iter().filter(|u| u.status.is_active()) {
        if let Some(p) = &u.phase {
            let e = ranks.entry(p.clone()).or_insert(u.seq);
            if u.seq < *e {
                *e = u.seq;
            }
        }
    }
    ranks
}

fn earlier_phases_done(
    units: &[WorkUnitRow],
    ranks: &BTreeMap<String, u32>,
    u: &WorkUnitRow,
) -> bool {
    let Some(rank) = u.phase.as_ref().and_then(|p| ranks.get(p)) else {
        return true;
    };
    units
        .iter()
        .filter(|o| o.status.is_active())
        .filter(|o| {
            o.phase
                .as_ref()
                .and_then(|p| ranks.get(p))
                .is_some_and(|r| r < rank)
        })
        .all(|o| o.status == WorkUnitStatus::Done)
}

/// ADR-0074 D1.4（Phase F2b）: 統合 WU の key の接頭辞（`integrate-<phase>`）。
pub const INTEGRATE_KEY_PREFIX: &str = "integrate-";

/// `integrate-<phase>`。
pub fn integrate_key(phase: &str) -> String {
    format!("{INTEGRATE_KEY_PREFIX}{phase}")
}

/// ADR-0074 D1.4（Phase F2b）: v2 の工程ごとの統合 WU の spec（`kind = integrate`、依存は
/// その工程のすべての WU）。計画の spec（`ExecutionPlanned.plan`）には入れない（daemon が足す
/// system WU。`max_work_units` にも数えない）。v1 は空。
pub fn integration_work_unit_specs(spec: &ExecutionPlanSpec) -> Vec<WorkUnitSpec> {
    if !is_phased_schema(&spec.schema) {
        return Vec::new();
    }
    // ADR-0079（Phase R1a）: /3 は段階を工程として同じ統合 WU を足す（`internal_view`）。
    let spec = internal_view(spec);
    spec.phases
        .iter()
        .map(|p| WorkUnitSpec {
            key: integrate_key(&p.key),
            kind: WorkUnitKind::Integrate,
            title: format!("工程 {} の統合", p.title),
            objective: format!(
                "工程 {} の WorkUnit のブランチを Task のブランチへ決定的に merge し、検査を再実行する（daemon が行う。LLM run は起こさない）",
                p.key
            ),
            depends_on: spec
                .work_units
                .iter()
                .filter(|w| w.phase.as_deref() == Some(p.key.as_str()))
                .map(|w| w.key.clone())
                .collect(),
            done_when: Vec::new(),
            checks: Vec::new(),
            context: WorkUnitContext::default(),
            harness: None,
            features: None,
            budget: None,
            outputs: Vec::new(),
            phase: Some(p.key.clone()),
        })
        .collect()
}

/// ADR-0074 F5-fix: 検証エラーに添える「daemon が足した WU は書かなくてよい」の一文。
pub const DAEMON_ADDED_HINT: &str = "daemon が足した WU（kind = integrate の統合 WU・統合の repair WU・配送 / 最終レビューの repair WU〈計画に無い kind = repair の repair-N〉）は書かなくてよい・書かない（daemon が旧版から持ち越す・補う）";

/// ADR-0074 D1.4（Phase F2b、F5-fix で共通化）: この行が daemon の足した WU（計画の spec に無い
/// system WU）か。`kind = integrate` の統合 WU、および `active` な計画の spec に key が無い行のうち、
/// v2/v3 で `phase` を持つもの（統合の repair WU）と、ADR-0079 付記「R7-12」D1: `kind = repair` のもの
/// （配送の repair WU・計画のある task の最終レビューの repair WU。`phase` を持たない）。planner の視野に
/// 無いので、replan の done の不変条件の対象にしない。
pub fn is_daemon_added_work_unit(active: &ExecutionPlanSpec, row: &WorkUnitRow) -> bool {
    if row.kind == WorkUnitKind::Integrate {
        return true;
    }
    let candidate = row.kind == WorkUnitKind::Repair
        || (is_phased_schema(&active.schema) && row.phase.is_some());
    candidate
        && !internal_view(active)
            .work_units
            .iter()
            .any(|w| w.key == row.key)
}

/// ADR-0079 付記「R7-12」D3: 新しい計画 `spec` が、生きた（superseded / cancelled でない）daemon の足した WU
/// （[`is_daemon_added_work_unit`]、統合 WU 以外）の key を unit に使っていないか。使えば `replan` はその key を
/// 新しい unit とみなし、既存の行（done の repair WU）を ready に戻して repair を再実行してしまう。統合 WU の key は
/// 段階から決まる（[`retired_key_errors`] の対象）ので見ない。
pub fn daemon_added_key_errors(
    active: &ExecutionPlanSpec,
    spec: &ExecutionPlanSpec,
    rows: &[WorkUnitRow],
) -> Vec<PlanValidationError> {
    let daemon: BTreeSet<&str> = rows
        .iter()
        .filter(|u| u.status.is_active())
        .filter(|u| u.kind != WorkUnitKind::Integrate && is_daemon_added_work_unit(active, u))
        .map(|u| u.key.as_str())
        .collect();
    internal_view(spec)
        .work_units
        .iter()
        .filter(|w| daemon.contains(w.key.as_str()))
        .map(|w| PlanValidationError::DaemonAddedKeyReused { key: w.key.clone() })
        .collect()
}

/// ADR-0074 D5.3/D1.4（F5-fix）: replan の `validate` に渡す done の WU（`(key, spec)`）。
/// daemon が足した WU（[`is_daemon_added_work_unit`]）は除く（行は replan が触れずに持ち越す）。
/// dispatcher（planner run の検証）と `task_ops::execution::replan` の両方がこれを使う。
pub fn replan_done_work_units(
    active: &ExecutionPlanSpec,
    rows: &[WorkUnitRow],
) -> Vec<(String, WorkUnitSpec)> {
    rows.iter()
        .filter(|u| u.status == WorkUnitStatus::Done && !is_daemon_added_work_unit(active, u))
        .map(|u| (u.key.clone(), u.spec.clone()))
        .collect()
}

/// ADR-0079 付記「R7-3」D3: 新しい計画 `spec` が退役した（superseded / cancelled の）行の key を再利用していないか
/// （`work_units` は `UNIQUE(task_id, key)`。採用の前に検証の理由として planner に返す）。`rows` はその task のすべての
/// 行（退役したものを含む）。
/// - 生きた行に無く退役した行が持つ key を、新しい計画の unit が使う（D5 の「superseded の key は再利用できない」）。
/// - 新しい計画の段階の統合 WU の key（`integrate-<stage>`）が、生きた行に無く退役した統合 WU の行と重なる（前の版で消した
///   段階の key を戻した）。以前はこれを調べず、採用が sqlite の `UNIQUE constraint failed` に落ちていた。
pub fn retired_key_errors(
    spec: &ExecutionPlanSpec,
    rows: &[WorkUnitRow],
) -> Vec<PlanValidationError> {
    let live: BTreeSet<&str> = rows
        .iter()
        .filter(|u| u.status.is_active())
        .map(|u| u.key.as_str())
        .collect();
    let retired: BTreeSet<&str> = rows
        .iter()
        .filter(|u| !u.status.is_active())
        .map(|u| u.key.as_str())
        .filter(|k| !live.contains(k))
        .collect();
    let mut errors = Vec::new();
    for w in &internal_view(spec).work_units {
        if retired.contains(w.key.as_str()) {
            errors.push(PlanValidationError::RetiredKeyReused {
                key: w.key.clone(),
                stage: None,
            });
        }
    }
    for integ in integration_work_unit_specs(spec) {
        if retired.contains(integ.key.as_str()) {
            errors.push(PlanValidationError::RetiredKeyReused {
                key: integ.key.clone(),
                stage: integ.phase.clone(),
            });
        }
    }
    errors
}

/// ADR-0079 D9（Phase R2b）: /3 の replan（差分 `execution-plan-delta/1` は /2 の形しか持たないので /3 は計画の全体を
/// 書く）で、`done` の unit を今の版（`active`）から持ち越す（純粋関数）。planner が done の unit を書かなかった・
/// 書き写し損ねた（unit の gate で上げ下げされた spec を知らない）ときも、採用した spec のまま新しい版に入る:
/// - `done_keys` の unit は `active` の spec で置き換える（無ければ足す。並びは新しい計画の段階の中の先頭）。
/// - その段階が新しい計画に無ければ、`active` の段階の順を保って挿入する。
/// - その unit が待つ決定（`needs_decisions`）が新しい計画に無ければ、`active` の決定を足す。
///
/// `active` か `new` が /3 でなければ何もしない。
pub fn carry_done_units_v3(
    active: &ExecutionPlanSpec,
    new: &mut ExecutionPlanSpec,
    done_keys: &BTreeSet<String>,
) {
    if active.schema != EXECUTION_PLAN_SCHEMA_V3 || new.schema != EXECUTION_PLAN_SCHEMA_V3 {
        return;
    }
    for done in active.units.iter().filter(|u| done_keys.contains(&u.key)) {
        // 段階（無ければ active の順を保って挿入）。
        if !new.stages.iter().any(|s| s.key == done.stage)
            && let Some(stage) = active.stages.iter().find(|s| s.key == done.stage)
        {
            let active_pos = |key: &str| active.stages.iter().position(|s| s.key == key);
            let own = active_pos(&stage.key).unwrap_or(0);
            let at = new
                .stages
                .iter()
                .position(|s| active_pos(&s.key).is_some_and(|p| p > own))
                .unwrap_or(new.stages.len());
            new.stages.insert(at, stage.clone());
        }
        match new.units.iter_mut().find(|u| u.key == done.key) {
            // ADR-0079 付記「R7-3」D1: planner が done の unit に空でない `checks` を書いていれば残す（段階の統合の
            // check を直せる）。他の欄は採用した spec に戻す。`checks` を書かなかった（空の）写しは従来どおり採用した
            // spec のまま（簡略に写した done の unit で check を黙って消さない）。
            Some(existing) => {
                let checks = std::mem::take(&mut existing.checks);
                *existing = done.clone();
                if !checks.is_empty() {
                    existing.checks = checks;
                }
            }
            None => {
                let at = new
                    .units
                    .iter()
                    .position(|u| u.stage == done.stage)
                    .unwrap_or(new.units.len());
                new.units.insert(at, done.clone());
            }
        }
        for d in &done.needs_decisions {
            if !new.decisions.iter().any(|x| &x.key == d)
                && !new
                    .units
                    .iter()
                    .any(|u| u.decisions.iter().any(|x| &x.key == d))
                && let Some(spec) = active.decisions.iter().find(|x| &x.key == d)
            {
                new.decisions.push(spec.clone());
            }
        }
    }
}

/// ADR-0074 D1.1/D1.4（Phase F2b）: 採用する計画の WU の並び（`seq` の順）。v1 はトポロジカル順
/// そのまま（従来どおり）。v2 は工程ごとに「その工程の WU（トポロジカル順）→ `integrate-<phase>`」。
pub fn materialized_order(
    spec: &ExecutionPlanSpec,
    topological_order: &[usize],
) -> Vec<WorkUnitSpec> {
    // ADR-0079（Phase R1a）: /3 は `units` を /2 の `work_units` の形に写してから並べる
    // （`topological_order` は `units` の index。写しても index は変わらない）。
    let integrations = integration_work_unit_specs(spec);
    let spec = internal_view(spec);
    let in_order: Vec<WorkUnitSpec> = topological_order
        .iter()
        .filter_map(|&i| spec.work_units.get(i).cloned())
        .collect();
    if !is_phased_schema(&spec.schema) {
        return in_order;
    }
    let mut out = Vec::with_capacity(in_order.len() + integrations.len());
    for (phase, integrate) in spec.phases.iter().zip(integrations) {
        out.extend(
            in_order
                .iter()
                .filter(|w| w.phase.as_deref() == Some(phase.key.as_str()))
                .cloned(),
        );
        out.push(integrate);
    }
    out
}

/// ADR-0074 D1.1/D1.4（Phase F2b）: 採用する計画の `work_units` の行を作る（純粋関数）。`id_of` は
/// 行の id を決める（`adopt_plan` は新しい ULID、replay は events から復元した id）。
/// v1 は「依存が無ければ ready、あれば pending」（従来どおり）。v2 は統合 WU を足し、
/// 工程の障壁つきの [`newly_ready`] で ready を決める（最初の工程の依存の無い WU だけが ready）。
pub fn materialize_work_units(
    task_id: &str,
    plan_id: &str,
    spec: &ExecutionPlanSpec,
    topological_order: &[usize],
    created_at: &str,
    id_of: &mut dyn FnMut(&WorkUnitSpec) -> String,
) -> Vec<WorkUnitRow> {
    let v2 = is_phased_schema(&spec.schema);
    let v3 = spec.schema == EXECUTION_PLAN_SCHEMA_V3;
    let mut rows: Vec<WorkUnitRow> = materialized_order(spec, topological_order)
        .into_iter()
        .enumerate()
        .map(|(seq, wu_spec)| {
            let status = if !v2 && wu_spec.depends_on.is_empty() {
                WorkUnitStatus::Ready
            } else {
                WorkUnitStatus::Pending
            };
            WorkUnitRow::new(
                id_of(&wu_spec),
                task_id.to_string(),
                plan_id.to_string(),
                seq as u32,
                wu_spec,
                status,
                created_at.to_string(),
            )
        })
        .collect();
    // ADR-0079 D7（Phase R1a）: /3 の unit が回答を待つ決定（`work_units.needs_decisions_json`）。
    if v3 {
        for r in rows.iter_mut() {
            r.needs_decisions = effective_needs_decisions(spec, &r.key);
        }
    }
    if v2 {
        let ready = newly_ready(&rows);
        for r in rows.iter_mut() {
            if ready.contains(&r.id) {
                r.status = WorkUnitStatus::Ready;
            }
        }
    }
    rows
}

/// ADR-0074 D1.4（Phase F2b）: 工程 `phase` の葉の WU（同じ工程の他の有効な WU に依存されていない、
/// 統合 WU 以外、ブランチを持つもの）を `seq` 順で。積み上げた依存先は葉に含まれる。
pub fn phase_leaves<'a>(units: &'a [WorkUnitRow], phase: &str) -> Vec<&'a WorkUnitRow> {
    let in_phase: Vec<&WorkUnitRow> = units
        .iter()
        .filter(|u| u.status.is_active())
        .filter(|u| u.kind != WorkUnitKind::Integrate)
        .filter(|u| u.phase.as_deref() == Some(phase))
        .collect();
    let mut leaves: Vec<&WorkUnitRow> = in_phase
        .iter()
        .copied()
        .filter(|u| u.branch.is_some())
        .filter(|u| {
            !in_phase
                .iter()
                .any(|o| o.id != u.id && o.depends_on.iter().any(|d| d == &u.key))
        })
        .collect();
    leaves.sort_by_key(|u| u.seq);
    leaves
}

/// ADR-0079 付記「R7-9」D3: dispatcher が統合済みの段階を開き直したときの `WorkUnitTransitioned.reason`
/// （replay はこの値の統合 WU の遷移で [`reopened_integration`] を当てる）。
pub const STAGE_REOPENED_REASON: &str = "stage_reopened";

/// ADR-0079 付記「R7-9」D1: done の統合 WU のうち、同じ段階に、その統合 WU の `depends_on` に無い生きた unit
/// （統合 WU 以外）があるもの（統合の後に段階へ入った unit。その branch / 子のブランチは task のブランチに merge
/// されていない）。戻り値は `(統合 WU の id, 足りない key〈seq 順〉)`、統合 WU の `seq` 順。純粋関数。
///
/// 統合 WU の依存は採用の時点の段階の unit すべて（[`integration_work_unit_specs`]）と統合の repair WU なので、
/// 統合の時点で段階にあった unit は必ず依存に入っている。`phase` を持たない行（最終レビューの repair WU）は
/// どの段階にも属さないので見ない。
pub fn stale_stage_integrations(units: &[WorkUnitRow]) -> Vec<(String, Vec<String>)> {
    let mut integrations: Vec<&WorkUnitRow> = units
        .iter()
        .filter(|u| u.kind == WorkUnitKind::Integrate && u.status == WorkUnitStatus::Done)
        .filter(|u| u.phase.is_some())
        .collect();
    integrations.sort_by_key(|u| u.seq);
    let mut out = Vec::new();
    for integ in integrations {
        let mut missing: Vec<&WorkUnitRow> = units
            .iter()
            .filter(|u| u.status.is_active())
            .filter(|u| u.kind != WorkUnitKind::Integrate)
            .filter(|u| u.phase.is_some() && u.phase == integ.phase)
            .filter(|u| !integ.depends_on.iter().any(|d| d == &u.key))
            .collect();
        if missing.is_empty() {
            continue;
        }
        missing.sort_by_key(|u| u.seq);
        out.push((
            integ.id.clone(),
            missing.into_iter().map(|u| u.key.clone()).collect(),
        ));
    }
    out
}

/// ADR-0079 付記「R7-9」D3/D4: [`stale_stage_integrations`] に当たった統合 WU `integ` を開き直した行（`pending`、
/// `depends_on` と `spec.depends_on` の末尾に `missing` を足す。lease・blocked_reason は外す。`integrated_commit` は
/// 前の統合の HEAD のまま残す〈次の統合が上書きする〉）。dispatcher と replay が同じ関数を使う。
pub fn reopened_integration(integ: &WorkUnitRow, missing: &[String]) -> WorkUnitRow {
    let mut row = integ.clone();
    for key in missing {
        if !row.depends_on.contains(key) {
            row.depends_on.push(key.clone());
        }
    }
    row.spec.depends_on = row.depends_on.clone();
    row.status = WorkUnitStatus::Pending;
    row.blocked_reason = None;
    row.clear_lease();
    row
}

/// D15: WU が `failed` になったとき、それに（直接・間接に）依存する未着手の WU を
/// `blocked(dependency_failed)` にする対象の `id` を返す（推移閉包）。
pub fn dependents_to_block(units: &[WorkUnitRow], failed_key: &str) -> Vec<String> {
    let mut blocked_keys: BTreeSet<String> = BTreeSet::new();
    blocked_keys.insert(failed_key.to_string());
    let mut changed = true;
    while changed {
        changed = false;
        for u in units {
            if blocked_keys.contains(&u.key) {
                continue;
            }
            if matches!(
                u.status,
                WorkUnitStatus::Pending | WorkUnitStatus::Ready | WorkUnitStatus::Blocked
            ) && u.depends_on.iter().any(|d| blocked_keys.contains(d))
            {
                blocked_keys.insert(u.key.clone());
                changed = true;
            }
        }
    }
    blocked_keys.remove(failed_key);
    units
        .iter()
        .filter(|u| blocked_keys.contains(&u.key))
        .map(|u| u.id.clone())
        .collect()
}
