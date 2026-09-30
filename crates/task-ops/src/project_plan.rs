//! 案件計画（ADR-0074 D3、`celeris.project-plan/1`）の**読み取りだけ**を残したもの。
//!
//! ADR-0079 D13（Phase R5a）: 案件は計画を持たない。書き込みの入口（`POST /projects/{id}/plan` の
//! `decompose` / `milestones`、`POST /projects/{id}/project-plan/{version}/decide`、`celerisctl projects plan`）と、
//! それが使っていた `start` / `start_milestones` / `start_replan` / `propose` / `propose_delta` / `decide`、
//! dispatcher の提案の取り込み（`validate_delta_against_store` / `record_proposal_failure`）、ADR-0077 の
//! `mark_milestone_dispatched` / `auto_reach_done_milestones` は外した（API は 410）。
//!
//! 残すのは、既存の行（本番では提案 0 件）を履歴として読むための `plan_state` と `dag_view`
//! （`GET /projects/{id}?include_frozen=true` の `project_plan`）。I/O はストアの読み取りだけで、LLM は呼ばない。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::{
    Event, ListFilter, ListOrder, Milestone, MilestoneId, MilestoneStatus, Project, ProjectId,
    ProjectStatus, ProposedMilestone, Status, Task, TaskId, TaskKind, TaskStore,
};

use crate::error::OpsError;

// ---- ADR-0074 D3.4（Phase F4b (e)）: 案件計画の版（plan タスクの events が正本）----

/// 案件計画の 1 つの版（`Event::ProjectPlanProposed` と、あれば `Event::ProjectPlanDecided`）。
#[derive(Debug, Clone, PartialEq)]
pub struct PlanVersion {
    pub version: u32,
    pub plan_task_id: TaskId,
    pub supersedes: Option<u32>,
    /// 承認されたときの計画全体。
    pub plan: task_core::ProjectPlanSpec,
    /// 承認されたときの計画全体の key → 途中目標 / Task。
    pub milestones: Vec<ProposedMilestone>,
    /// replan の差分（初回は `None`）。
    pub delta: Option<task_core::ProjectPlanDelta>,
    /// `Some(true)` 承認、`Some(false)` 却下、`None` 未決。
    pub decided: Option<bool>,
}

impl PlanVersion {
    /// この版の提案が新しく作った（`add` の、初回なら全部の）key → 途中目標 / Task。
    pub fn created(&self) -> Vec<&ProposedMilestone> {
        match &self.delta {
            None => self.milestones.iter().collect(),
            Some(delta) => self
                .milestones
                .iter()
                .filter(|m| delta.add.iter().any(|a| a.key == m.key))
                .collect(),
        }
    }
}

/// 案件の案件計画の全版（版の昇順）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectPlanState {
    pub versions: Vec<PlanVersion>,
    /// 案件計画 run（`is_milestones_plan_task`）のうち、まだ終端でなく提案も出していないもの。
    pub in_flight_plan_tasks: Vec<TaskId>,
}

impl ProjectPlanState {
    /// 現行の（最新の承認済みの）版。
    pub fn current(&self) -> Option<&PlanVersion> {
        self.versions.iter().rev().find(|v| v.decided == Some(true))
    }
    /// 未決の提案（あれば 1 つ。二重の依頼は `ensure_no_plan_in_flight` が防ぐ）。
    pub fn pending(&self) -> Option<&PlanVersion> {
        self.versions.iter().rev().find(|v| v.decided.is_none())
    }
    pub fn version(&self, version: u32) -> Option<&PlanVersion> {
        self.versions.iter().find(|v| v.version == version)
    }
    /// 次に提案する版（既存の最大 + 1。却下された版の番号も再利用しない）。
    pub fn next_version(&self) -> u32 {
        self.versions.iter().map(|v| v.version).max().unwrap_or(0) + 1
    }
}

/// 案件の plan タスク（`is_milestones_plan_task`）の events から、案件計画の全版を決定的に組み立てる。
pub fn plan_state(
    store: &dyn TaskStore,
    project_id: ProjectId,
) -> Result<ProjectPlanState, OpsError> {
    let plan_tasks = store
        .list_page(
            &ListFilter {
                project_id: Some(project_id),
                kinds: vec![TaskKind::Plan],
                ..ListFilter::default()
            },
            ListOrder::CreatedDesc,
            None,
            1000,
        )?
        .items;
    let mut state = ProjectPlanState::default();
    for t in plan_tasks {
        if !task_core::is_milestones_plan_task(&t) {
            continue;
        }
        let rows = store.event_rows_for(t.id, None, crate::view::ALL_EVENTS)?;
        // まだ提案を出していない（run 中・再試行待ちの）計画 run。提案を出した後は `pending` が見る。
        if !t.status.is_terminal()
            && !rows
                .iter()
                .any(|r| matches!(r.event, Event::ProjectPlanProposed { .. }))
        {
            state.in_flight_plan_tasks.push(t.id);
        }
        for row in &rows {
            match &row.event {
                Event::ProjectPlanProposed {
                    version,
                    supersedes,
                    plan,
                    milestones,
                    delta,
                    ..
                } => state.versions.push(PlanVersion {
                    version: *version,
                    plan_task_id: t.id,
                    supersedes: *supersedes,
                    plan: (**plan).clone(),
                    milestones: milestones.clone(),
                    delta: delta.as_deref().cloned(),
                    decided: None,
                }),
                Event::ProjectPlanDecided {
                    version, approved, ..
                } => {
                    if let Some(v) = state.versions.iter_mut().find(|v| v.version == *version) {
                        v.decided = Some(*approved);
                    }
                }
                _ => {}
            }
        }
    }
    state.versions.sort_by_key(|v| v.version);
    Ok(state)
}

// ---- ADR-0074 D3.5（Phase F4b (h)）: 案件ページの DAG（決定的な読み取りだけ）----

/// 節点が止まっている理由（GUI の DAG の節点に出す）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanStopReason {
    /// 工程の後の途中確認（F3 の `awaiting_human`）。
    AwaitingHuman,
    /// 人への質問で止まっている（`blocked`）。
    Question,
    Failed,
    /// 依存先は終わったが、その途中目標がまだ `reached` でない（途中目標の Go 待ち。D3.2）。
    AwaitingGo,
    /// 途中目標か案件が一時停止中。
    Paused,
}

/// 提案（replan の差分）でこの節点がどう変わるか。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanNodeChange {
    Add,
    Modify,
    Remove,
    Cancel,
}

/// DAG の 1 節点（= マイルストーン Task）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlanDagNode {
    pub key: String,
    pub title: String,
    /// 辺（同じ計画の他の節点の key）。
    pub depends_on: Vec<String>,
    pub milestone_id: MilestoneId,
    pub task_id: TaskId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestone_status: Option<MilestoneStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_status: Option<Status>,
    /// 進み具合: WU（統合 WU を除く、有効なもの）の done / total。計画の無い Task は 0 / 0。
    pub work_units_done: u32,
    pub work_units_total: u32,
    /// 子 Task（委譲・planner の children）の done / total。
    pub children_done: u32,
    pub children_total: u32,
    /// ADR-0074 D4.3: 使った quota（この Task の run の合計）。
    #[serde(default)]
    pub quota: Vec<task_core::QuotaUse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<PlanStopReason>,
    /// 提案の中の節点だけ: この提案でどう変わるか（変わらなければ省略）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<PlanNodeChange>,
}

/// 未決の提案（初回の提案か replan の差分）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlanDagProposal {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<u32>,
    pub rationale: String,
    /// 承認されたときの計画全体（外す・取り下げる節点も `change` 付きで含める）。
    pub nodes: Vec<PlanDagNode>,
}

/// `GET /projects/{id}` の `project_plan`（案件計画が無い案件では省略）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProjectPlanDagView {
    /// 現行の（承認済みの）版。まだ無ければ `None`（提案だけがある）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_version: Option<u32>,
    /// 現行の計画の節点（`plan.milestones` の順）。
    pub nodes: Vec<PlanDagNode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<PlanDagProposal>,
}

/// ADR-0074 D3.5（Phase F4b (h)）: 案件ページの DAG を組み立てる（案件計画の版が 1 つも無ければ `None`）。
pub fn dag_view(
    store: &dyn TaskStore,
    project: &Project,
) -> Result<Option<ProjectPlanDagView>, OpsError> {
    let state = plan_state(store, project.id)?;
    if state.versions.is_empty() {
        return Ok(None);
    }
    let milestones = store.milestone_list(project.id)?;
    let project_paused = project.status == ProjectStatus::Paused;
    let current = state.current();
    let nodes = match current {
        Some(v) => dag_nodes(store, project, v, &milestones, project_paused, None)?,
        None => Vec::new(),
    };
    let pending = match state.pending() {
        Some(p) => {
            let mut nodes = dag_nodes(
                store,
                project,
                p,
                &milestones,
                project_paused,
                p.delta.as_ref(),
            )?;
            // 差分で外す・取り下げる節点は、承認後の計画には無いので現行の版から足して印を付ける。
            if let (Some(delta), Some(cur)) = (&p.delta, current) {
                for (keys, change) in [
                    (&delta.remove, PlanNodeChange::Remove),
                    (&delta.cancel, PlanNodeChange::Cancel),
                ] {
                    for key in keys {
                        let dropped =
                            dag_nodes(store, project, cur, &milestones, project_paused, None)?
                                .into_iter()
                                .find(|n| &n.key == key);
                        if let Some(mut n) = dropped {
                            n.change = Some(change);
                            nodes.push(n);
                        }
                    }
                }
            }
            Some(PlanDagProposal {
                version: p.version,
                supersedes: p.supersedes,
                rationale: p.plan.rationale.clone(),
                nodes,
            })
        }
        None => None,
    };
    Ok(Some(ProjectPlanDagView {
        current_version: current.map(|v| v.version),
        nodes,
        pending,
    }))
}

fn dag_nodes(
    store: &dyn TaskStore,
    project: &Project,
    version: &PlanVersion,
    milestones: &[Milestone],
    project_paused: bool,
    delta: Option<&task_core::ProjectPlanDelta>,
) -> Result<Vec<PlanDagNode>, OpsError> {
    let mut out = Vec::with_capacity(version.plan.milestones.len());
    for spec in &version.plan.milestones {
        let Some(mapped) = version.milestones.iter().find(|m| m.key == spec.key) else {
            continue;
        };
        let task = store.get(mapped.task_id)?;
        let milestone = milestones.iter().find(|m| m.id == mapped.milestone_id);
        let (wu_done, wu_total) = match &task {
            Some(_) => {
                let units = store.work_units_for(mapped.task_id)?;
                let active: Vec<&task_core::WorkUnitRow> = units
                    .iter()
                    .filter(|u| {
                        u.status.is_active() && u.kind != task_core::WorkUnitKind::Integrate
                    })
                    .collect();
                (
                    active
                        .iter()
                        .filter(|u| u.status == task_core::WorkUnitStatus::Done)
                        .count() as u32,
                    active.len() as u32,
                )
            }
            None => (0, 0),
        };
        let children = store.children(mapped.task_id)?;
        let quota = match &task {
            Some(t) => {
                let events: Vec<Event> = store
                    .events_for(t.id)?
                    .into_iter()
                    .map(|(_, e)| e)
                    .collect();
                task_core::summarize_execution_metrics(t, &events).quota
            }
            None => Vec::new(),
        };
        let stop_reason = match &task {
            Some(t) => stop_reason_of(store, project, t, milestone, project_paused)?,
            None => None,
        };
        let change = delta.and_then(|d| {
            if d.add.iter().any(|a| a.key == spec.key) {
                Some(PlanNodeChange::Add)
            } else if d.modify.iter().any(|m| m.key == spec.key) {
                Some(PlanNodeChange::Modify)
            } else {
                None
            }
        });
        out.push(PlanDagNode {
            key: spec.key.clone(),
            title: spec.title.clone(),
            depends_on: spec.depends_on.clone(),
            milestone_id: mapped.milestone_id,
            task_id: mapped.task_id,
            milestone_status: milestone.map(|m| m.status),
            task_status: task.as_ref().map(|t| t.status),
            work_units_done: wu_done,
            work_units_total: wu_total,
            children_done: children.iter().filter(|c| c.status == Status::Done).count() as u32,
            children_total: children.len() as u32,
            quota,
            stop_reason,
            change,
        });
    }
    Ok(out)
}

fn stop_reason_of(
    store: &dyn TaskStore,
    project: &Project,
    task: &Task,
    milestone: Option<&Milestone>,
    project_paused: bool,
) -> Result<Option<PlanStopReason>, OpsError> {
    if project_paused || milestone.is_some_and(|m| m.status == MilestoneStatus::Paused) {
        return Ok(Some(PlanStopReason::Paused));
    }
    match task.status {
        Status::Failed => Ok(Some(PlanStopReason::Failed)),
        Status::Blocked => {
            let reason = store
                .events_for(task.id)?
                .into_iter()
                .rev()
                .find_map(|(_, e)| match e {
                    Event::Transitioned { reason, .. } => Some(reason),
                    _ => None,
                });
            Ok(Some(if reason.as_deref() == Some("awaiting_human") {
                PlanStopReason::AwaitingHuman
            } else {
                PlanStopReason::Question
            }))
        }
        Status::Ready if !project.auto_advance => {
            // 依存先が done で、その途中目標（案件計画のもの）がまだ reached でなければ Go 待ち。
            for dep in &task.depends_on {
                let Some(dep_task) = store.get(*dep)? else {
                    continue;
                };
                if dep_task.status != Status::Done || dep_task.milestone_id == task.milestone_id {
                    continue;
                }
                if let Some(mid) = dep_task.milestone_id
                    && let Some(m) = store.milestone_get(mid)?
                    && m.plan_key.is_some()
                    && m.status != MilestoneStatus::Reached
                {
                    return Ok(Some(PlanStopReason::AwaitingGo));
                }
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests;
