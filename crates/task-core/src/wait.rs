//! ADR 2026-10-11-task-wait-reasons: task の「なぜ今走らないか」（待ちの理由）。
//!
//! DB だけで決まる理由（D1〜D13）はここの純粋関数（`halt_waits`・`db_waits`・`plan_waits`・`unit_waits`）で決め、
//! `ready_tasks_impl`・`newly_ready`・dispatcher の `resolve_child_dependencies` が同じ関数を呼ぶ（画面用に別の
//! 推測を作らない）。dispatcher のメモリにしか無い理由（M1〜M8）は dispatcher が tick ごとに決め、
//! `DaemonSnapshot.task_waits` に載せる。I/O・LLM 呼び出しはしない。

use std::collections::{BTreeMap, BTreeSet, HashMap};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::execution_plan::{
    CHILD_DEP_PREFIX, WorkUnitBlockedReason, WorkUnitKind, WorkUnitRow, WorkUnitStatus,
    newly_ready_with, phase_ranks, runnable_work_units,
};
use crate::message::is_conversation;
use crate::model::{Status, Task, TaskId, TaskKind};
use crate::org::{MilestoneId, ProjectId};

/// 案件・途中目標が dispatch を止めている状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HaltState {
    Paused,
    Cancelled,
    Archived,
}

/// 止まっている案件／途中目標の id（文字列）→ 状態。
pub type HaltedSet = HashMap<String, HaltState>;

/// 人の gate の種類（D12）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HumanGateKind {
    PlanApproval,
    StageReview,
    PhaseCheckpoint,
}

/// 枠が満杯の範囲（M1/M9）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CapacityScope {
    /// 全体の `max_concurrency`（窓・容量で今 tick 照合されなかった M9 を含む）。
    Global,
    /// CoS の対話の枠（`max_cos_runs`）。
    Cos,
    /// account の上限（またはクラスタの並列度）。
    Account,
    /// 暗黙アカウント（LLM source）の上限・選べる source が全部満杯。
    Source,
}

/// バックオフの原因（M5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackoffCause {
    Retry,
    Infra,
}

/// M10: root 間の依存先が既定 branch に入っていない理由（ADR 2026-10-17-root-dependency-waits-for-integration D3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationWaitCause {
    /// 取り込み待ち（delivery の進行中を含む）。
    Unmerged,
    /// delivery が取り込みを見送った（`detail` に理由）。
    DeliverySkipped,
    /// 人が保留にした。
    Held,
    /// 人が取り込まないと決めた。
    Discarded,
}

/// 待っている WorkUnit 1 件（D8/D9）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnitWait {
    pub key: String,
    pub stage: Option<String>,
    /// depends_on のうち done でない key（宣言順）。`child:<key>` 依存もそのまま出す。
    pub waiting_on: Vec<String>,
    /// 前の段が終わっていないなら、その段の key（D9）。
    pub waiting_stage: Option<String>,
}

/// task の待ちの理由 1 件。並びは宣言順（DB の理由 → メモリの理由）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TaskWait {
    /// D1/D2。`by` が自分なら自分の一時停止。
    Paused { by: TaskId },
    /// D2: 祖先が止まった案件に属す。
    AncestorProjectHalted {
        task_id: TaskId,
        project_id: ProjectId,
    },
    /// D3。
    ProjectHalted {
        project_id: ProjectId,
        state: HaltState,
    },
    /// D4。
    MilestoneHalted {
        milestone_id: MilestoneId,
        state: HaltState,
    },
    /// D5: 未完の先行 task（id 順）。
    DependsOn { task_ids: Vec<TaskId> },
    /// D6: 親が未完の Approval。
    ApprovalParent { task_id: TaskId },
    /// D7: 未完の子と、その代理 unit の key。
    Children {
        task_ids: Vec<TaskId>,
        unit_keys: Vec<String>,
    },
    /// D8/D9: runnable な unit が無く、待っている unit がある。
    WorkUnits { units: Vec<UnitWait> },
    /// D10: 段に failed / blocked がある。
    StageHalted {
        stage: String,
        unit_keys: Vec<String>,
    },
    /// ADR 2026-10-23-stage-order-and-plan-stall D4: 計画の都合で何も起こせない（走っている unit も、起こせる・上げられる
    /// unit も、走れる統合も、名指しの待ち〈失敗・blocked・子・決定〉も無いのに非終端の unit が残る）。`stage` は scheduler の
    /// 現在の段、`unit_keys` は現在の段の外で ready の unit と現在の段の pending の unit（seq 順）。
    PlanStalled {
        stage: Option<String>,
        unit_keys: Vec<String>,
        detail: String,
    },
    /// D11。
    HumanDecision { decision_keys: Vec<String> },
    /// D12。
    HumanGate {
        gate: HumanGateKind,
        stage: Option<String>,
    },
    /// D13（infra / cluster_jobs）。
    UnitsBlocked {
        reason: WorkUnitBlockedReason,
        unit_keys: Vec<String>,
    },
    /// M1/M9。
    Capacity {
        scope: CapacityScope,
        id: Option<String>,
    },
    /// M2。
    Unroutable { detail: String },
    /// M3。
    Cluster { cluster_id: String, detail: String },
    /// M4。
    WriteSet { blocking_task_ids: Vec<TaskId> },
    /// M5。`until` は RFC 3339。
    Backoff { until: String, cause: BackoffCause },
    /// M6。
    DiskCritical { path: String },
    /// M7。
    DelegatedChildren { task_ids: Vec<TaskId> },
    /// M8。
    ReviewAwaitingHuman,
    /// M10: root 間の依存先は done だが、その成果が既定 branch に入っていない。
    DependsOnIntegration {
        task_id: TaskId,
        cause: IntegrationWaitCause,
        detail: String,
    },
}

/// 祖先を辿る上限（循環と深すぎる鎖で dispatch 全体を止めない）。
pub const MAX_ANCESTRY: usize = 32;

/// 木の親（`parent_id`、無ければ `tree.parent_unit.task_id`）。
pub fn parent_of(task: &Task) -> Option<TaskId> {
    task.parent_id.or_else(|| {
        task.tree
            .as_ref()
            .and_then(|tree| tree.parent_unit.as_ref())
            .map(|u| u.task_id)
    })
}

/// D1〜D6 の判定に使う DB の事実。
pub struct WaitFacts<'a> {
    pub task: &'a Task,
    /// 親から root へ（`MAX_ANCESTRY` まで。循環で打ち切り）。
    pub ancestors: &'a [Task],
    /// `depends_on` の各 task の状態（無い行は `None`）。
    pub deps: &'a [(TaskId, Option<Status>)],
    pub parent: Option<&'a Task>,
    pub halted_projects: &'a HaltedSet,
    pub halted_milestones: &'a HaltedSet,
}

/// D1〜D4: 一時停止と案件・途中目標の停止。対話（`is_conversation`）は止めない（ADR-0044 D6）。
/// 自分の案件・途中目標の停止を先に、祖先の鎖は最初に当たった 1 件だけを返す。
pub fn halt_waits(
    task: &Task,
    ancestors: &[Task],
    halted_projects: &HaltedSet,
    halted_milestones: &HaltedSet,
) -> Vec<TaskWait> {
    let mut out = Vec::new();
    if is_conversation(task) {
        return out;
    }
    if let Some(p) = task.project_id
        && let Some(state) = halted_projects.get(&p.to_string())
    {
        out.push(TaskWait::ProjectHalted {
            project_id: p,
            state: *state,
        });
    }
    if let Some(m) = task.milestone_id
        && let Some(state) = halted_milestones.get(&m.to_string())
    {
        out.push(TaskWait::MilestoneHalted {
            milestone_id: m,
            state: *state,
        });
    }
    // ADR-0079 D13（Phase R5a）: subtree の一時停止。
    if task.paused_at.is_some() {
        out.push(TaskWait::Paused { by: task.id });
        return out;
    }
    for ancestor in ancestors {
        if ancestor.paused_at.is_some() {
            out.push(TaskWait::Paused { by: ancestor.id });
            break;
        }
        if let Some(p) = ancestor.project_id
            && halted_projects.contains_key(&p.to_string())
        {
            out.push(TaskWait::AncestorProjectHalted {
                task_id: ancestor.id,
                project_id: p,
            });
            break;
        }
    }
    out
}

/// D1〜D6: DB だけで決まる task 単位の待ち。空なら `ready_tasks` に入る。
pub fn db_waits(f: &WaitFacts<'_>) -> Vec<TaskWait> {
    let mut out = halt_waits(f.task, f.ancestors, f.halted_projects, f.halted_milestones);
    // P-78（ADR-0033 D4）: 対話の depends_on は前の対話が終端なら進めてよい。
    let is_conv = is_conversation(f.task);
    let mut unfinished: Vec<TaskId> = f
        .deps
        .iter()
        .filter(|(_, status)| match status {
            Some(Status::Done) => false,
            Some(s) if is_conv && s.is_terminal() => false,
            _ => true,
        })
        .map(|(id, _)| *id)
        .collect();
    unfinished.sort();
    unfinished.dedup();
    if !unfinished.is_empty() {
        out.push(TaskWait::DependsOn {
            task_ids: unfinished,
        });
    }
    if let Some(parent) = f.parent
        && parent.kind == TaskKind::Approval
        && parent.status != Status::Done
    {
        out.push(TaskWait::ApprovalParent { task_id: parent.id });
    }
    out
}

/// 子 task 1 件の事実（`child-<key>` のラベルで計画の `child:<key>` 依存と結ぶ）。
#[derive(Debug, Clone)]
pub struct ChildFacts {
    pub task_id: TaskId,
    pub status: Status,
    pub labels: Vec<String>,
}

/// ADR-0074 D3.7: `child:<key>` 依存の解決と子待ち（D7）。dispatcher の `resolve_child_dependencies` と
/// `plan_waits` が同じ値を使う。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChildResolution {
    /// 子が done の `child:<key>`。
    pub done: BTreeSet<String>,
    /// 子が failed / cancelled の `child:<key>`。
    pub failed: BTreeSet<String>,
    /// 解けていない `child:<key>` 依存を持つ pending unit の key（seq 順）。
    pub waiting_units: Vec<String>,
}

/// `child:<key>` 依存を子の状態で解く。
pub fn resolve_children(units: &[WorkUnitRow], children: &[ChildFacts]) -> ChildResolution {
    let mut res = ChildResolution::default();
    for child in children {
        for label in &child.labels {
            let Some(key) = label.strip_prefix("child-") else {
                continue;
            };
            let dep = format!("{CHILD_DEP_PREFIX}{key}");
            match child.status {
                Status::Done => {
                    res.done.insert(dep);
                }
                Status::Failed | Status::Cancelled => {
                    res.failed.insert(dep);
                }
                _ => {}
            }
        }
    }
    let mut waiting: Vec<&WorkUnitRow> = units
        .iter()
        .filter(|u| {
            u.status == WorkUnitStatus::Pending
                && u.depends_on.iter().any(|d| {
                    d.starts_with(CHILD_DEP_PREFIX)
                        && !res.done.contains(d)
                        && !res.failed.contains(d)
                })
        })
        .collect();
    waiting.sort_by_key(|u| u.seq);
    res.waiting_units = waiting.into_iter().map(|u| u.key.clone()).collect();
    res
}

/// D8/D9: pending の各 unit の待ち（`newly_ready` と同じ規則。待ちが空なら `newly_ready` に入る）。
/// `external_done` は unit の外で done になった依存（`child:<key>`）。待ちの無い unit も返す（seq 順）。
pub fn unit_waits(units: &[WorkUnitRow], external_done: &BTreeSet<String>) -> Vec<UnitWait> {
    let mut pending: Vec<&WorkUnitRow> = units
        .iter()
        .filter(|u| u.status == WorkUnitStatus::Pending)
        .collect();
    pending.sort_by_key(|u| u.seq);
    let judge = UnitWaitJudge::new(units, external_done);
    pending.into_iter().map(|u| judge.wait_of(u)).collect()
}

/// `unit_waits` と `newly_ready` が共有する判定（done の key 集合と段の順位を 1 回だけ作る）。
pub(crate) struct UnitWaitJudge<'a> {
    units: &'a [WorkUnitRow],
    done: BTreeSet<&'a str>,
    ranks: BTreeMap<String, u32>,
}

impl<'a> UnitWaitJudge<'a> {
    pub(crate) fn new(units: &'a [WorkUnitRow], external_done: &'a BTreeSet<String>) -> Self {
        let done = units
            .iter()
            .filter(|u| u.status == WorkUnitStatus::Done)
            .map(|u| u.key.as_str())
            .chain(external_done.iter().map(String::as_str))
            .collect();
        Self {
            units,
            done,
            ranks: phase_ranks(units),
        }
    }

    pub(crate) fn wait_of(&self, u: &WorkUnitRow) -> UnitWait {
        UnitWait {
            key: u.key.clone(),
            stage: u.phase.clone(),
            waiting_on: u
                .depends_on
                .iter()
                .filter(|d| !self.done.contains(d.as_str()))
                .cloned()
                .collect(),
            waiting_stage: earliest_unfinished_phase(self.units, &self.ranks, u),
        }
    }
}

/// `u` より前の段のうち、done でない有効な行を持つ最初の段（ADR-0074 D1.3 の工程の障壁）。
fn earliest_unfinished_phase(
    units: &[WorkUnitRow],
    ranks: &BTreeMap<String, u32>,
    u: &WorkUnitRow,
) -> Option<String> {
    let rank = u.phase.as_ref().and_then(|p| ranks.get(p))?;
    units
        .iter()
        .filter(|o| o.status.is_active() && o.status != WorkUnitStatus::Done)
        .filter_map(|o| {
            let p = o.phase.as_ref()?;
            let r = ranks.get(p)?;
            (r < rank).then_some((*r, p.clone()))
        })
        .min()
        .map(|(_, p)| p)
}

impl UnitWait {
    /// 待ちが無い（`newly_ready` に入る）。
    pub fn is_clear(&self) -> bool {
        self.waiting_on.is_empty() && self.waiting_stage.is_none()
    }
}

/// ADR 2026-10-23-stage-order-and-plan-stall D4: [`plan_stall`] の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanStall {
    pub stage: Option<String>,
    pub unit_keys: Vec<String>,
    pub detail: String,
}

/// ADR 2026-10-23-stage-order-and-plan-stall D4: scheduler が計画の都合で何も起こせないか（純粋関数）。次のすべてのとき
/// `Some`:
/// - 非終端の有効な unit がある。
/// - `running` の unit（kind task の子の写しを含む）・`blocked`・`failed` の unit が無い（それぞれ別の名指しの待ちか replan）。
/// - 未回答の決定（`open_decision_keys`）・未完の子への `child:` 依存が無い。
/// - 起こせる unit（[`runnable_work_units`]）・現在の段の ready の kind task の unit・上げられる unit（[`newly_ready_with`]）が無い。
/// - 現在の段の非統合の unit がすべて done で統合 WU が待っている（dispatcher が統合を走らせる）形でもない。
pub fn plan_stall(
    units: &[WorkUnitRow],
    external_done: &BTreeSet<String>,
    open_decision_keys: &[String],
) -> Option<PlanStall> {
    if !open_decision_keys.is_empty() {
        return None;
    }
    let live: Vec<&WorkUnitRow> = units
        .iter()
        .filter(|u| u.status.is_active() && !u.status.is_terminal())
        .collect();
    let current = live.iter().min_by_key(|u| u.seq)?;
    if live.iter().any(|u| {
        matches!(
            u.status,
            WorkUnitStatus::Running | WorkUnitStatus::Blocked | WorkUnitStatus::Failed
        )
    }) {
        return None;
    }
    if live.iter().any(|u| {
        u.status == WorkUnitStatus::Pending
            && u.depends_on
                .iter()
                .any(|d| d.starts_with(CHILD_DEP_PREFIX) && !external_done.contains(d))
    }) {
        return None;
    }
    let stage = current.phase.clone();
    let in_stage = |u: &&&WorkUnitRow| u.phase == stage;
    if !runnable_work_units(units, 0, usize::MAX).is_empty()
        || !newly_ready_with(units, external_done).is_empty()
        || live.iter().filter(in_stage).any(|u| {
            u.kind == WorkUnitKind::Task
                && matches!(
                    u.status,
                    WorkUnitStatus::Ready | WorkUnitStatus::NeedsContinuation
                )
        })
    {
        return None;
    }
    let stage_units_done = units
        .iter()
        .filter(|u| u.status.is_active() && u.phase == stage && u.kind != WorkUnitKind::Integrate)
        .all(|u| u.status == WorkUnitStatus::Done);
    if stage.is_some()
        && stage_units_done
        && live
            .iter()
            .filter(in_stage)
            .any(|u| u.kind == WorkUnitKind::Integrate)
    {
        return None;
    }
    let mut stuck: Vec<&&WorkUnitRow> = live
        .iter()
        .filter(|u| u.kind != WorkUnitKind::Integrate)
        .filter(|u| {
            (u.phase != stage
                && matches!(
                    u.status,
                    WorkUnitStatus::Ready | WorkUnitStatus::NeedsContinuation
                ))
                || (u.phase == stage && u.status == WorkUnitStatus::Pending)
        })
        .collect();
    stuck.sort_by_key(|u| u.seq);
    let unit_keys: Vec<String> = stuck.iter().map(|u| u.key.clone()).collect();
    let label = stage.clone().unwrap_or_else(|| "-".to_string());
    let detail = format!(
        "計画の都合で起こせる unit が無い（現在の段 {label}、止まっている unit: {}）",
        if unit_keys.is_empty() {
            "-".to_string()
        } else {
            unit_keys.join(", ")
        }
    );
    Some(PlanStall {
        stage,
        unit_keys,
        detail,
    })
}

/// D7〜D11・D13: 計画（WorkUnit）から決まる待ち。`open_decision_keys` は未回答の `needed_before: self` の決定。
pub fn plan_waits(
    units: &[WorkUnitRow],
    children: &[ChildFacts],
    open_decision_keys: &[String],
) -> Vec<TaskWait> {
    let mut out = Vec::new();
    let live: Vec<&WorkUnitRow> = units.iter().filter(|u| u.status.is_active()).collect();
    let resolution = resolve_children(units, children);

    // D7: 未完の kind task の unit（子の代理）と、解けていない `child:<key>` 依存を持つ pending unit。
    let child_status: HashMap<String, Status> = children
        .iter()
        .map(|c| (c.task_id.to_string(), c.status))
        .collect();
    let mut child_ids: BTreeSet<TaskId> = BTreeSet::new();
    let mut child_keys: Vec<String> = Vec::new();
    for u in live.iter().filter(|u| {
        u.kind == WorkUnitKind::Task && !u.status.is_terminal() && u.child_task_id.is_some()
    }) {
        let Some(id) = u.child_task_id.as_deref() else {
            continue;
        };
        if child_status.get(id).is_some_and(|s| s.is_terminal()) {
            continue;
        }
        if let Ok(ulid) = id.parse() {
            child_ids.insert(TaskId(ulid));
        }
        child_keys.push(u.key.clone());
    }
    let waiting_dep_keys: BTreeSet<String> = units
        .iter()
        .filter(|u| resolution.waiting_units.contains(&u.key))
        .flat_map(|u| u.depends_on.iter())
        .filter_map(|d| d.strip_prefix(CHILD_DEP_PREFIX))
        .map(|k| format!("child-{k}"))
        .collect();
    for c in children {
        if !c.status.is_terminal() && c.labels.iter().any(|l| waiting_dep_keys.contains(l)) {
            child_ids.insert(c.task_id);
        }
    }
    for k in &resolution.waiting_units {
        if !child_keys.contains(k) {
            child_keys.push(k.clone());
        }
    }
    if !child_keys.is_empty() || !child_ids.is_empty() {
        out.push(TaskWait::Children {
            task_ids: child_ids.into_iter().collect(),
            unit_keys: child_keys,
        });
    }

    // ADR 2026-10-23-stage-order-and-plan-stall D4: 計画の詰まり（D8/D9 の unit ごとの待ちも出せるときは続けて出す）。
    let stall = plan_stall(units, &resolution.done, open_decision_keys);
    if let Some(s) = &stall {
        out.push(TaskWait::PlanStalled {
            stage: s.stage.clone(),
            unit_keys: s.unit_keys.clone(),
            detail: s.detail.clone(),
        });
    }
    // D8/D9: runnable な unit が無く、走っている unit も無いときだけ。
    let moving = live.iter().any(|u| {
        matches!(
            u.status,
            WorkUnitStatus::Running | WorkUnitStatus::Ready | WorkUnitStatus::NeedsContinuation
        )
    });
    if !moving && runnable_work_units(units, 0, usize::MAX).is_empty() {
        let waits: Vec<UnitWait> = unit_waits(units, &resolution.done)
            .into_iter()
            .filter(|w| !w.is_clear())
            .collect();
        if !waits.is_empty() {
            out.push(TaskWait::WorkUnits { units: waits });
        }
    }

    // D10: 現在の段（非終端の最小 seq の段）に failed / blocked（decision・infra・cluster_jobs 以外）がある。
    if let Some(current) = live
        .iter()
        .filter(|u| !u.status.is_terminal())
        .min_by_key(|u| u.seq)
    {
        let phase = current.spec.phase.clone();
        let mut halted: Vec<&WorkUnitRow> = live
            .iter()
            .copied()
            .filter(|u| !u.status.is_terminal() && u.spec.phase == phase)
            .filter(|u| {
                u.status == WorkUnitStatus::Failed
                    || (u.status == WorkUnitStatus::Blocked
                        && !matches!(
                            u.blocked_reason,
                            Some(
                                WorkUnitBlockedReason::Decision
                                    | WorkUnitBlockedReason::Infra
                                    | WorkUnitBlockedReason::ClusterJobs
                                    | WorkUnitBlockedReason::WaitingForAuth
                            )
                        ))
            })
            .collect();
        halted.sort_by_key(|u| u.seq);
        if !halted.is_empty() {
            out.push(TaskWait::StageHalted {
                stage: phase.unwrap_or_default(),
                unit_keys: halted.into_iter().map(|u| u.key.clone()).collect(),
            });
        }
    }

    // D11: 人の決定。
    let mut decision_keys: Vec<String> = open_decision_keys.to_vec();
    for u in live.iter().filter(|u| {
        u.status == WorkUnitStatus::Blocked
            && u.blocked_reason == Some(WorkUnitBlockedReason::Decision)
    }) {
        if u.needs_decisions.is_empty() {
            decision_keys.push(u.key.clone());
        }
        decision_keys.extend(u.needs_decisions.iter().cloned());
    }
    let mut seen = BTreeSet::new();
    decision_keys.retain(|k| seen.insert(k.clone()));
    if !decision_keys.is_empty() {
        out.push(TaskWait::HumanDecision { decision_keys });
    }

    // D13: infra / cluster_jobs で止まっている unit。
    for reason in [
        WorkUnitBlockedReason::Infra,
        WorkUnitBlockedReason::ClusterJobs,
        WorkUnitBlockedReason::WaitingForAuth,
    ] {
        let mut blocked: Vec<&WorkUnitRow> = live
            .iter()
            .copied()
            .filter(|u| u.status == WorkUnitStatus::Blocked && u.blocked_reason == Some(reason))
            .collect();
        blocked.sort_by_key(|u| u.seq);
        if !blocked.is_empty() {
            out.push(TaskWait::UnitsBlocked {
                reason,
                unit_keys: blocked.into_iter().map(|u| u.key.clone()).collect(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests;
