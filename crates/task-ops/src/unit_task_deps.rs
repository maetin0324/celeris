//! ADR 2026-11-11-unit-task-dependencies: 計画の unit の `task:<id>` 依存の事実を読む（store と git の読み取りだけ。LLM・書き込みなし）。
//!
//! 判断は task-core の純粋関数（`resolve_task_dependencies`・`task_dependency_errors`）。ここは事実を集めて渡すだけで、
//! 取り込みの判定は M10 の [`crate::dep_integration::dependency_integration`] をそのまま呼ぶ（判定のコードを写さない）。

use std::collections::{BTreeMap, BTreeSet};

use task_core::{
    Event, ExecutionPlanSpec, StoreError, Task, TaskDependencyContext, TaskDependencyError,
    TaskDependencyFact, TaskDependencyResolution, TaskId, TaskStore, WorkUnitBlockedReason,
    WorkUnitRow, WorkUnitStatus,
};

use crate::dep_integration::dependency_integration;

/// 検証の循環の探索で辿る task の上限（深すぎる鎖で採用を止めない。超えた先は辿らない）。
const MAX_CYCLE_WALK: usize = 512;

/// D2: `task` の pending の unit が待つ `task:` 依存を解く（`units` は `task` の `work_units_for`）。依存元は計画を持つ `task`。
/// `integrated` は M10 と同じ取り込み済みの記憶（呼び出し側が持つ）。
pub fn resolve(
    store: &dyn TaskStore,
    task: &Task,
    units: &[WorkUnitRow],
    integrated: &mut BTreeSet<TaskId>,
) -> Result<TaskDependencyResolution, StoreError> {
    let ids: BTreeSet<TaskId> = units
        .iter()
        .filter(|u| u.status == WorkUnitStatus::Pending)
        .flat_map(|u| task_core::unit_task_dependencies(u).collect::<Vec<_>>())
        .collect();
    if ids.is_empty() {
        return Ok(TaskDependencyResolution::default());
    }
    let mut facts = Vec::with_capacity(ids.len());
    for id in ids {
        let Some(dep) = store.get(id)? else {
            continue;
        };
        let integration = dependency_integration(store, task, &dep, integrated)?;
        facts.push(TaskDependencyFact {
            task_id: dep.id,
            status: dep.status,
            integration,
        });
    }
    Ok(task_core::resolve_task_dependencies(units, &facts))
}

/// [`settle_external`] の結果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExternalSettle {
    /// unit の行を書き換えた（呼び出し側は読み直す）。
    pub changed: bool,
    /// まだ終わっていない子（`child:<key>`）を待つ pending の unit がある。
    pub waiting_on_children: bool,
}

/// ADR-0074 D3.7（Phase F4b (f)）: `depends_on: ["child:<key>"]` の WU を、子 Task（`child-<key>` の印を持つ
/// `parent_id = task_id` の Task）の状態で進める。子が `done` なら（他の依存も満たされていれば）`pending → ready`、
/// 子が `failed` / `cancelled` なら `pending → blocked(dependency_failed)`（D17 の replan の対象）。
/// ADR 2026-11-11-unit-task-dependencies D2・D3: `task:<id>`（木の外の task）の依存も同じ所で解く（[`resolve`]）。満たされた
/// unit は `pending → ready`（`task_dependency_met`）。依存先の失敗では上げも止めもしない（人に知らせるのは生存確認）。
/// dispatcher の `resolve_child_dependencies` が呼ぶ（`integrated` は M10 と同じ取り込み済みの記憶の写し）。
pub fn settle_external(
    store: &dyn TaskStore,
    task_id: TaskId,
    units: &[WorkUnitRow],
    integrated: &BTreeSet<TaskId>,
) -> Result<ExternalSettle, StoreError> {
    let prefix = task_core::CHILD_DEP_PREFIX;
    let has_child_deps = |u: &WorkUnitRow| u.depends_on.iter().any(|d| d.starts_with(prefix));
    let has_task_deps = |u: &WorkUnitRow| task_core::unit_task_dependencies(u).next().is_some();
    if !units
        .iter()
        .any(|u| u.status == WorkUnitStatus::Pending && (has_child_deps(u) || has_task_deps(u)))
    {
        return Ok(ExternalSettle::default());
    }
    let task_deps = match store.get(task_id)? {
        Some(task) => resolve(store, &task, units, &mut integrated.clone())?,
        None => TaskDependencyResolution::default(),
    };
    // ADR 2026-10-11-task-wait-reasons D3: 子の状態での解決と「子待ち」は task-core の `resolve_children`
    // （一覧・木の `Children` と同じ関数）。
    let children: Vec<task_core::ChildFacts> = store
        .children(task_id)?
        .into_iter()
        .map(|c| task_core::ChildFacts {
            task_id: c.id,
            status: c.status,
            labels: c.labels,
        })
        .collect();
    let task_core::ChildResolution {
        mut done,
        failed,
        waiting_units,
    } = task_core::resolve_children(units, &children);
    done.extend(task_deps.met.iter().cloned());
    let mut changed = false;
    for u in units.iter().filter(|u| {
        u.status == WorkUnitStatus::Pending && u.depends_on.iter().any(|d| failed.contains(d))
    }) {
        let mut row = u.clone();
        row.status = WorkUnitStatus::Blocked;
        row.blocked_reason = Some(WorkUnitBlockedReason::DependencyFailed);
        transition(store, task_id, u, row, "dependency_failed")?;
        changed = true;
    }
    if !changed {
        for id in task_core::newly_ready_with(units, &done) {
            let Some(u) = units.iter().find(|u| u.id == id) else {
                continue;
            };
            let reason = if has_child_deps(u) {
                "child_done"
            } else if has_task_deps(u) {
                "task_dependency_met"
            } else {
                continue;
            };
            let mut row = u.clone();
            row.status = WorkUnitStatus::Ready;
            transition(store, task_id, u, row, reason)?;
            changed = true;
        }
    }
    Ok(ExternalSettle {
        changed,
        waiting_on_children: !waiting_units.is_empty(),
    })
}

fn transition(
    store: &dyn TaskStore,
    task_id: TaskId,
    from: &WorkUnitRow,
    row: WorkUnitRow,
    reason: &str,
) -> Result<(), StoreError> {
    let to = row.status;
    store.work_unit_transition(
        task_id,
        row,
        Event::WorkUnitTransitioned {
            work_unit_id: from.id.clone(),
            key: from.key.clone(),
            from: from.status,
            to,
            reason: reason.to_string(),
            run_id: None,
        },
    )
}

/// D4: 採用の前の検証。計画を持つ `task` の木・案件と、依存先から辿れる「待つ」辺を store から読み、task-core の
/// `task_dependency_errors` に渡す。`task:` 依存の無い計画は何も読まない。
pub fn plan_errors(
    store: &dyn TaskStore,
    task: &Task,
    spec: &ExecutionPlanSpec,
) -> Result<Vec<TaskDependencyError>, StoreError> {
    let deps = task_core::plan_task_dependencies(spec);
    if deps.is_empty() {
        return Ok(Vec::new());
    }
    let root = task_core::tree::root_id_of(task);
    let mut own_tree = BTreeSet::from([root, task.id]);
    let mut frontier = vec![root];
    while let Some(id) = frontier.pop() {
        for child in store.children(id)? {
            if own_tree.insert(child.id) {
                frontier.push(child.id);
            }
        }
    }
    let mut ctx = TaskDependencyContext {
        project: task.project_id,
        own_tree,
        ..Default::default()
    };
    let mut queue: Vec<TaskId> = Vec::new();
    for id in deps.iter().filter_map(|(_, _, id)| *id) {
        if ctx.targets.contains_key(&id) {
            continue;
        }
        if let Some(t) = store.get(id)? {
            ctx.targets.insert(t.id, t.project_id);
            queue.push(t.id);
        }
    }
    let mut seen: BTreeSet<TaskId> = queue.iter().copied().collect();
    while let Some(id) = queue.pop() {
        if ctx.waits_on.len() >= MAX_CYCLE_WALK {
            break;
        }
        let edges = waits_on(store, id)?;
        for next in &edges {
            if seen.insert(*next) && !ctx.own_tree.contains(next) {
                queue.push(*next);
            }
        }
        ctx.waits_on.insert(id, edges);
    }
    Ok(task_core::task_dependency_errors(spec, &ctx))
}

/// `id` が待つ task（`depends_on` と、有効な計画の終端でない unit の `task:` 依存）。
fn waits_on(store: &dyn TaskStore, id: TaskId) -> Result<Vec<TaskId>, StoreError> {
    let Some(t) = store.get(id)? else {
        return Ok(Vec::new());
    };
    let mut out: BTreeMap<TaskId, ()> = t.depends_on.iter().map(|d| (*d, ())).collect();
    for u in store.work_units_for(id)? {
        if u.status.is_active() && !u.status.is_terminal() {
            out.extend(task_core::unit_task_dependencies(&u).map(|d| (d, ())));
        }
    }
    Ok(out.into_keys().collect())
}
