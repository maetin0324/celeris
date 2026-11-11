//! ADR 2026-11-11-unit-task-dependencies: 計画の unit の `task:<id>` 依存の事実を読む（store と git の読み取りだけ。LLM・書き込みなし）。
//!
//! 判断は task-core の純粋関数（`resolve_task_dependencies`・`task_dependency_errors`）。ここは事実を集めて渡すだけで、
//! 取り込みの判定は M10 の [`crate::dep_integration::dependency_integration`] をそのまま呼ぶ（判定のコードを写さない）。

use std::collections::{BTreeMap, BTreeSet};

use task_core::{
    ExecutionPlanSpec, PlanValidationError, StoreError, Task, TaskDependencyContext,
    TaskDependencyFact, TaskDependencyResolution, TaskId, TaskStore, TaskWait, WorkUnitRow,
    WorkUnitStatus,
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
        .flat_map(|u| task_core::task_deps::unit_task_dependencies(u).collect::<Vec<_>>())
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

/// D5: 解決のまだ満たされない最初の依存先（task_id 順）を M11 の待ちの理由にする。
pub fn first_wait(resolution: &TaskDependencyResolution) -> Option<TaskWait> {
    resolution.waits.first().map(|w| TaskWait::TaskDependency {
        task_id: w.task_id,
        cause: w.cause,
        detail: w.detail.clone(),
        unit_keys: w.unit_keys.clone(),
    })
}

/// D4: 採用の前の検証。計画を持つ `task` の木・案件と、依存先から辿れる「待つ」辺を store から読み、task-core の
/// `task_dependency_errors` に渡す。`task:` 依存の無い計画は何も読まない。
pub fn plan_errors(
    store: &dyn TaskStore,
    task: &Task,
    spec: &ExecutionPlanSpec,
) -> Result<Vec<PlanValidationError>, StoreError> {
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
    for (_, id) in &deps {
        if ctx.targets.contains_key(id) {
            continue;
        }
        if let Some(t) = store.get(*id)? {
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
            out.extend(task_core::task_deps::unit_task_dependencies(&u).map(|d| (d, ())));
        }
    }
    Ok(out.into_keys().collect())
}
