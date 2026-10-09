//! ADR-0079 D15（Phase R5b-prep）: 既存の task を木の子として採用する（adopt）。
//!
//! 入口は 2 つ:
//! - 人の計画（origin human）の kind task の unit の `adopt: <task_id>`（`PUT/POST /tasks/{id}/execution-plan`、
//!   `celerisctl execution plan set`）。計画の採用と同じトランザクションで結ぶ（[`apply_plan_adoptions`]）。
//! - 採用済みの計画への後からの採用 `POST /tasks/{id}/tree/adopt {task_id, stage, unit_key}`（管理系、[`adopt`]）。
//!
//! 条件（D15）: 対象は同じ案件の execute の task、計画を持つ task 自身でも祖先でもない、他の木に属さない（`tree` を
//! 持たず、自分が木の root でもない）、計画にその unit が kind task・同じ段階・`adopt: <task_id>` として書かれている。
//! **対象は終端で成果を持つもの（`done` か `failed`）だけ**（R5b-prep の決定。ADR-0079 付記「R5b-prep 実装時の
//! 逸脱・明確化」）: 採用した unit は `done` になり、段階の統合は子のブランチ `celeris/<id>` を任意の項目として扱う
//! （既に main か親のブランチに入っていれば `skipped`、無ければ飛ばす。R1c の冪等の規則）。`cancelled` は採用しない
//! （409）。終端でない task は、人の計画では unit を結ばずに待たせ（`adopted: false`）、後からの採用では 409。
//!
//! 結んだとき: unit の行は `done`・`child_task_id` = 対象（`WorkUnitTransitioned{reason: child_adopted}` と
//! `Event::ChildAdopted{plan_id, unit_key, stage, child_task_id}` を計画を持つ task に積む）、対象の `tree` =
//! `{root_id, depth, parent_unit}`（`base_commit` は無し = 木がブランチを切っていない）、`parent_id` が無ければ
//! 計画を持つ task（あれば書き換えない。BenchFS の `kind = plan` の子）、対象に `Event::Edited{fields: ["tree", ..]}`。
//! 対象の状態・履歴は変えない。LLM なし。

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::execution_plan::PlanUnitSpec;
use task_core::{
    Event, ExecutionPlanRow, ParentUnit, Status, Task, TaskId, TaskKind, TaskStore, TreeAdoption,
    TreeInfo, TreeLimits, WorkUnitKind, WorkUnitRow, WorkUnitStatus,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::error::OpsError;

/// `WorkUnitTransitioned.reason`（採用で unit を `done` にした）。
pub const CHILD_ADOPTED: &str = "child_adopted";

/// `POST /tasks/{id}/tree/adopt` の本文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdoptRequest {
    /// 採用する既存の task。
    pub task_id: TaskId,
    /// unit の段階の key（計画の unit の `stage` と一致すること）。
    pub stage: String,
    /// 計画の kind task の unit の key（`adopt: <task_id>` を持つこと）。
    pub unit_key: String,
}

/// 採用の結果（1 unit 分）。`PUT /tasks/{id}/execution-plan` の `adoptions[]` と `POST /tasks/{id}/tree/adopt` の応答。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdoptionOutcome {
    pub plan_id: String,
    pub unit_key: String,
    pub stage: String,
    pub task_id: TaskId,
    /// 結んだ（unit は `done`）。`false` は対象がまだ終端でないので unit が採用を待っている（人の計画だけ）。
    pub adopted: bool,
    /// 対象の状態（採用しても変えない）。
    pub task_status: Status,
    /// unit の行の状態（結んだ後）。
    pub unit_status: WorkUnitStatus,
    /// 人が読む 1 行。
    pub detail: String,
}

/// Validated late adoption write set, ready to apply in a caller-owned transaction.
#[derive(Debug, Clone)]
pub struct PreparedAdoption {
    pub owner_id: TaskId,
    pub unit_id: String,
    pub expect_unit_status: WorkUnitStatus,
    pub updated: Vec<WorkUnitRow>,
    pub events: Vec<Event>,
    pub adoption: TreeAdoption,
    pub outcome: AdoptionOutcome,
}

fn refuse(conflict: bool, code: &'static str, detail: impl Into<String>) -> OpsError {
    OpsError::TreeAdopt {
        conflict,
        code,
        detail: detail.into(),
    }
}

fn rfc3339(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap_or_default()
}

/// 対象の今の扱い。
enum TargetState {
    /// 終端で成果を持つ（`done` / `failed`）: 今すぐ結べる。
    Adoptable,
    /// 終端でない: まだ結べない。
    NotTerminal,
}

/// 祖先（`tree.parent_unit` か、無ければ `parent_id`）を辿る。32 段・循環で打ち切り。
fn is_self_or_ancestor(
    store: &dyn TaskStore,
    owner: &Task,
    target: TaskId,
) -> Result<bool, OpsError> {
    let mut seen: BTreeSet<TaskId> = BTreeSet::new();
    let mut current = owner.clone();
    for _ in 0..32 {
        if current.id == target {
            return Ok(true);
        }
        if !seen.insert(current.id) {
            break;
        }
        let up = current
            .tree
            .as_ref()
            .and_then(|t| t.parent_unit.as_ref())
            .map(|u| u.task_id)
            .or(current.parent_id);
        let Some(up) = up else { break };
        match store.get(up)? {
            Some(t) => current = t,
            None => break,
        }
    }
    Ok(false)
}

/// D15 の対象の条件（計画の unit の側の条件は呼び出し側）。
fn check_target(
    store: &dyn TaskStore,
    owner: &Task,
    unit_key: &str,
    target: &Task,
) -> Result<TargetState, OpsError> {
    if is_self_or_ancestor(store, owner, target.id)? {
        return Err(refuse(
            false,
            "adopt_ancestor",
            format!(
                "unit {unit_key}: task {} is the plan's task itself or one of its ancestors (ADR-0079 D15)",
                target.id
            ),
        ));
    }
    if target.project_id.is_none() || target.project_id != owner.project_id {
        return Err(refuse(
            false,
            "adopt_other_project",
            format!(
                "unit {unit_key}: task {} does not belong to the same project as {} (ADR-0079 D15)",
                target.id, owner.id
            ),
        ));
    }
    if target.kind != TaskKind::Execute
        || task_core::is_conversation(target)
        || task_core::report::support_kind(target).is_some()
    {
        return Err(refuse(
            false,
            "adopt_target_kind",
            format!(
                "unit {unit_key}: task {} is not a work task (kind {:?}; conversations and support tasks cannot be adopted)",
                target.id, target.kind
            ),
        ));
    }
    if target.tree.is_some() {
        return Err(refuse(
            true,
            "adopt_target_in_tree",
            format!(
                "unit {unit_key}: task {} already belongs to a task tree (ADR-0079 D15)",
                target.id
            ),
        ));
    }
    if store
        .tree_tasks(target.id)?
        .iter()
        .any(|t| t.id != target.id)
    {
        return Err(refuse(
            true,
            "adopt_target_in_tree",
            format!(
                "unit {unit_key}: task {} is the root of another task tree (ADR-0079 D15)",
                target.id
            ),
        ));
    }
    match target.status {
        Status::Done | Status::Failed => Ok(TargetState::Adoptable),
        Status::Cancelled => Err(refuse(
            true,
            "adopt_target_cancelled",
            format!(
                "unit {unit_key}: task {} is cancelled; only a done or failed task with its output can be adopted",
                target.id
            ),
        )),
        Status::Draft | Status::Ready | Status::Running | Status::Blocked | Status::Reviewing => {
            Ok(TargetState::NotTerminal)
        }
    }
}

/// unit を結ぶ行・event・対象の書き換え（純粋。`unit_row` は今の行）。
fn link(
    owner: &Task,
    plan_id: &str,
    unit_row: &WorkUnitRow,
    stage: &str,
    target: &Task,
    by: &str,
    now: OffsetDateTime,
) -> (WorkUnitRow, Vec<Event>, TreeAdoption) {
    let mut row = unit_row.clone();
    row.status = WorkUnitStatus::Done;
    row.blocked_reason = None;
    row.child_task_id = Some(target.id.to_string());
    row.clear_lease();
    row.updated_at = rfc3339(now);
    let events = vec![
        Event::WorkUnitTransitioned {
            work_unit_id: unit_row.id.clone(),
            key: unit_row.key.clone(),
            from: unit_row.status,
            to: WorkUnitStatus::Done,
            reason: CHILD_ADOPTED.to_string(),
            run_id: None,
        },
        Event::ChildAdopted {
            plan_id: plan_id.to_string(),
            unit_key: unit_row.key.clone(),
            stage: stage.to_string(),
            child_task_id: target.id,
        },
    ];
    let mut adopted = target.clone();
    adopted.tree = Some(TreeInfo::child_of(
        owner,
        ParentUnit {
            task_id: owner.id,
            plan_id: plan_id.to_string(),
            unit_key: unit_row.key.clone(),
            stage: stage.to_string(),
            attempt: 1,
        },
        None,
    ));
    let mut fields = vec!["tree".to_string()];
    if adopted.parent_id.is_none() {
        adopted.parent_id = Some(owner.id);
        fields.push("parent_id".to_string());
    }
    adopted.updated_at = now;
    let adoption = TreeAdoption {
        task: adopted,
        expect_status: target.status,
        event: Event::Edited {
            fields,
            by: by.to_string(),
        },
    };
    (row, events, adoption)
}

/// 依存が満たされた `pending` の行を `ready` にする（段階の障壁は `newly_ready` が見る）。`rows` を書き換え、
/// 書き換えた行の id と event を返す。
fn release_dependents(rows: &mut [WorkUnitRow], now: OffsetDateTime) -> (Vec<String>, Vec<Event>) {
    let ids = task_core::newly_ready(rows);
    let mut events = Vec::new();
    for id in &ids {
        if let Some(r) = rows.iter_mut().find(|r| &r.id == id) {
            r.status = WorkUnitStatus::Ready;
            r.updated_at = rfc3339(now);
            events.push(Event::WorkUnitTransitioned {
                work_unit_id: r.id.clone(),
                key: r.key.clone(),
                from: WorkUnitStatus::Pending,
                to: WorkUnitStatus::Ready,
                reason: "dependency_ready".to_string(),
                run_id: None,
            });
        }
    }
    (ids, events)
}

/// [`apply_plan_adoptions`] の結果。
#[derive(Debug, Default)]
pub struct PlanAdoptions {
    /// 計画を持つ task に `ExecutionPlanned` の後で積む event（unit の遷移・`ChildAdopted`・依存の解放）。
    pub events: Vec<Event>,
    pub adoptions: Vec<TreeAdoption>,
    pub outcomes: Vec<AdoptionOutcome>,
}

/// 人の計画の `adopt` の unit を、採用する前に組み立てた行（`rows`）の上で結ぶ（書き込みは呼び出し側が計画の採用と
/// 同じトランザクションで行う）。条件に合わない unit があれば計画全体を拒否する（422 / 409）。対象がまだ終端でない
/// unit は結ばずに待たせる（`adopted: false`。後で `POST /tasks/{id}/tree/adopt`）。
pub fn apply_plan_adoptions(
    store: &dyn TaskStore,
    owner: &Task,
    plan: &ExecutionPlanRow,
    rows: &mut [WorkUnitRow],
    by: &str,
    now: OffsetDateTime,
) -> Result<PlanAdoptions, OpsError> {
    let mut out = PlanAdoptions::default();
    let mut seen: BTreeSet<TaskId> = BTreeSet::new();
    let units: Vec<&PlanUnitSpec> = plan
        .spec
        .units
        .iter()
        .filter(|u| u.adopt.is_some())
        .collect();
    for unit in units {
        let Some(target_id) = unit.adopt else {
            continue;
        };
        if !seen.insert(target_id) {
            return Err(refuse(
                false,
                "adopt_duplicate",
                format!(
                    "unit {}: task {target_id} is adopted by more than one unit of the plan",
                    unit.key
                ),
            ));
        }
        let Some(target) = store.get(target_id)? else {
            return Err(refuse(
                false,
                "adopt_target_not_found",
                format!("unit {}: task {target_id} does not exist", unit.key),
            ));
        };
        let state = check_target(store, owner, &unit.key, &target)?;
        let Some(idx) = rows
            .iter()
            .position(|r| r.key == unit.key && r.kind == WorkUnitKind::Task)
        else {
            continue;
        };
        match state {
            TargetState::Adoptable => {
                let (row, events, adoption) =
                    link(owner, &plan.id, &rows[idx], &unit.stage, &target, by, now);
                out.outcomes.push(AdoptionOutcome {
                    plan_id: plan.id.clone(),
                    unit_key: unit.key.clone(),
                    stage: unit.stage.clone(),
                    task_id: target.id,
                    adopted: true,
                    task_status: target.status,
                    unit_status: WorkUnitStatus::Done,
                    detail: format!(
                        "task {} ({:?}) adopted as unit {}; its output is integrated at the end of stage {} (skipped when already in the base)",
                        target.id, target.status, unit.key, unit.stage
                    ),
                });
                rows[idx] = row;
                out.events.extend(events);
                out.adoptions.push(adoption);
            }
            TargetState::NotTerminal => {
                out.outcomes.push(AdoptionOutcome {
                    plan_id: plan.id.clone(),
                    unit_key: unit.key.clone(),
                    stage: unit.stage.clone(),
                    task_id: target.id,
                    adopted: false,
                    task_status: target.status,
                    unit_status: rows[idx].status,
                    detail: format!(
                        "task {} is {:?}; unit {} waits and is adopted with POST /tasks/{}/tree/adopt once the task is done or failed",
                        target.id, target.status, unit.key, owner.id
                    ),
                });
            }
        }
    }
    if !out.adoptions.is_empty() {
        let (_, events) = release_dependents(rows, now);
        out.events.extend(events);
    }
    Ok(out)
}

/// D15: `POST /tasks/{owner}/tree/adopt`。採用済みの /3 の計画の kind task の unit に、`adopt` に書かれた既存の task を
/// 結ぶ（上の条件）。`limits` は daemon の実効の `[execution.tree]`（無効なら 422 `tree_disabled`）。
pub fn adopt(
    store: &dyn TaskStore,
    owner_id: TaskId,
    req: &AdoptRequest,
    limits: &TreeLimits,
    by: &str,
    now: OffsetDateTime,
) -> Result<AdoptionOutcome, OpsError> {
    let prepared = prepare_adopt(store, owner_id, req, limits, by, now)?;
    if !store.tree_adopt_apply(
        prepared.owner_id,
        &prepared.unit_id,
        prepared.expect_unit_status,
        prepared.updated,
        prepared.events,
        prepared.adoption,
    )? {
        return Err(refuse(
            true,
            "adopt_conflict",
            "the task, the unit or the adopted task changed concurrently; read them again",
        ));
    }
    Ok(prepared.outcome)
}

/// Read and validate a late adoption without writing. The returned set can be applied atomically
/// with operation audit rows by `tree_adopt_apply_tx`.
pub fn prepare_adopt(
    store: &dyn TaskStore,
    owner_id: TaskId,
    req: &AdoptRequest,
    limits: &TreeLimits,
    by: &str,
    now: OffsetDateTime,
) -> Result<PreparedAdoption, OpsError> {
    if !limits.enabled {
        return Err(refuse(
            false,
            "tree_disabled",
            "[execution.tree] enabled = false: task trees are disabled (ADR-0079 D2)",
        ));
    }
    let Some(owner) = store.get(owner_id)? else {
        return Err(OpsError::NotFound(owner_id));
    };
    if owner.status.is_terminal() {
        return Err(refuse(
            true,
            "adopt_owner_terminal",
            format!(
                "task {owner_id} is {:?}; it cannot adopt a child",
                owner.status
            ),
        ));
    }
    let Some(plan) = store
        .execution_plan_active(owner_id)?
        .filter(|p| p.spec.schema == task_core::EXECUTION_PLAN_SCHEMA_V3)
    else {
        return Err(refuse(
            false,
            "adopt_no_tree_plan",
            format!("task {owner_id} has no active celeris.execution-plan/3 plan"),
        ));
    };
    let Some(unit) = plan.spec.units.iter().find(|u| u.key == req.unit_key) else {
        return Err(refuse(
            false,
            "adopt_unit_not_found",
            format!("the active plan has no unit {}", req.unit_key),
        ));
    };
    if !unit.is_task() {
        return Err(refuse(
            false,
            "adopt_unit_not_task",
            format!(
                "unit {} is a leaf (kind {:?}); only a kind task unit can adopt a task",
                unit.key, unit.kind
            ),
        ));
    }
    if unit.stage != req.stage {
        return Err(refuse(
            false,
            "adopt_stage_mismatch",
            format!(
                "unit {} is in stage {}, not {}",
                unit.key, unit.stage, req.stage
            ),
        ));
    }
    if unit.adopt != Some(req.task_id) {
        return Err(refuse(
            false,
            "adopt_id_mismatch",
            format!(
                "unit {} does not name task {} in its adopt field (it names {}); write adopt in the human plan first (ADR-0079 D2 / D15)",
                unit.key,
                req.task_id,
                unit.adopt
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| "none".to_string())
            ),
        ));
    }
    let Some(target) = store.get(req.task_id)? else {
        return Err(OpsError::NotFound(req.task_id));
    };
    match check_target(store, &owner, &unit.key, &target)? {
        TargetState::Adoptable => {}
        TargetState::NotTerminal => {
            return Err(refuse(
                true,
                "adopt_target_not_terminal",
                format!(
                    "task {} is {:?}; only a done or failed task can be adopted in this phase (ADR-0079 R5b-prep)",
                    target.id, target.status
                ),
            ));
        }
    }
    let mut rows = store.work_units_for(owner_id)?;
    let Some(idx) = rows
        .iter()
        .position(|r| r.key == unit.key && r.kind == WorkUnitKind::Task)
    else {
        return Err(refuse(
            true,
            "adopt_unit_not_open",
            format!("unit {} has no work unit row", unit.key),
        ));
    };
    let current = rows[idx].clone();
    if current.child_task_id.is_some()
        || !matches!(
            current.status,
            WorkUnitStatus::Pending | WorkUnitStatus::Ready
        )
    {
        return Err(refuse(
            true,
            "adopt_unit_not_open",
            format!(
                "unit {} is {:?}{}; only a pending or ready unit without a child can adopt a task",
                unit.key,
                current.status,
                current
                    .child_task_id
                    .as_deref()
                    .map(|c| format!(" with child {c}"))
                    .unwrap_or_default()
            ),
        ));
    }
    let (row, mut events, adoption) =
        link(&owner, &plan.id, &current, &unit.stage, &target, by, now);
    rows[idx] = row.clone();
    let (released, more) = release_dependents(&mut rows, now);
    events.extend(more);
    let mut updated = vec![row];
    updated.extend(rows.iter().filter(|r| released.contains(&r.id)).cloned());
    let outcome = AdoptionOutcome {
        plan_id: plan.id.clone(),
        unit_key: unit.key.clone(),
        stage: unit.stage.clone(),
        task_id: target.id,
        adopted: true,
        task_status: target.status,
        unit_status: WorkUnitStatus::Done,
        detail: format!(
            "task {} ({:?}) adopted as unit {}; its output is integrated at the end of stage {} (skipped when already in the base)",
            target.id, target.status, unit.key, unit.stage
        ),
    };
    Ok(PreparedAdoption {
        owner_id,
        unit_id: current.id,
        expect_unit_status: current.status,
        updated,
        events,
        adoption,
        outcome,
    })
}
