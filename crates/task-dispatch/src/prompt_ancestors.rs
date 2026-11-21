//! ADR 2026-11-21-ancestor-objective-in-prompts D1: run の `RunContext.lineage.ancestors`（祖先の task の objective と受入
//! 条件の全文）を store から組む。worker・planner・WU の run（`Dispatcher::run_extras`）、final review の run
//! （`Dispatcher::review_human_inputs`）、設計 review の run（`health_review::design_gate`）が同じこの関数を使う。store を
//! 読むだけで、LLM を呼ばない。ADR 2026-11-21-strategy-evidence-contract D5（U3）: 同じ列から案件の `Project.request`
//! （`RunContext.lineage.project`）も組む。

use task_core::{SqliteStore, StoreError, Task};
use task_worker::protocol::{
    AncestorContext, ChildSummary, ProjectRequestContext, TaskLineageContext,
};

/// 対象 task から root への列の長さの上限（循環・深すぎる木に備える。ADR-0117 D1 の人の決定・回答の収集と同じ値）。
const MAX_CHAIN: usize = 32;

/// `task` から root へ辿った列（対象 task 自身が先頭、root が末尾）。上は木の親（`tree_parent`）、無ければ `parent_id`。
/// store に無い祖先・循環・上限で止める（辿れた所まで）。ADR-0117 D1 の人の決定・回答の収集と祖先の節が同じ列を使う。
pub(crate) fn chain_to_root(store: &SqliteStore, task: &Task) -> Result<Vec<Task>, StoreError> {
    let mut chain = vec![task.clone()];
    let mut cursor = task_core::tree::tree_parent(task).or(task.parent_id);
    while let Some(id) = cursor {
        if chain.iter().any(|t| t.id == id) || chain.len() >= MAX_CHAIN {
            break;
        }
        let Some(up) = store.get(id)? else {
            break;
        };
        cursor = task_core::tree::tree_parent(&up).or(up.parent_id);
        chain.push(up);
    }
    Ok(chain)
}

/// `chain_to_root` の列の祖先（対象 task 自身を除く）を root が先頭、親が末尾の順に。親の無い task では空。
pub(crate) fn ancestor_contexts(chain: &[Task]) -> Vec<AncestorContext> {
    chain
        .iter()
        .skip(1)
        .rev()
        .map(AncestorContext::from)
        .collect()
}

/// ADR 2026-11-21-strategy-evidence-contract D5.2・D5.3: `chain_to_root` の列（対象 task が先頭）で最初に `project_id` を
/// 持つ task の案件の `Project.request` の全文と sha256。案件に属さない列では `None`。案件を持つのに行を読めない
/// （store の失敗・行が無い）ときは `Err` で、呼び出し側は run を起こさない（人の元の目的の無い prompt で走らせない）。
fn project_request(
    store: &SqliteStore,
    chain: &[Task],
) -> Result<Option<ProjectRequestContext>, StoreError> {
    let Some((task, project_id)) = chain.iter().find_map(|t| t.project_id.map(|p| (t.id, p)))
    else {
        return Ok(None);
    };
    match store.project_get(project_id)? {
        Some(project) => Ok(Some(ProjectRequestContext::from(&project))),
        None => Err(StoreError::Invalid(format!(
            "project {project_id} of task {task} is not in the store; the run's Project.request cannot be read"
        ))),
    }
}

/// `chain_to_root` の列の `RunContext.lineage`（祖先と案件の依頼。子の要約は空）。final review の reviewer の run
/// （`Dispatcher::review_human_inputs`）はこれをそのまま、worker・planner・WU の run（`lineage`）は子の要約を足して使う。
pub(crate) fn chain_lineage(
    store: &SqliteStore,
    chain: &[Task],
) -> Result<TaskLineageContext, StoreError> {
    Ok(TaskLineageContext {
        ancestors: ancestor_contexts(chain),
        children: Vec::new(),
        project: project_request(store, chain)?,
    })
}

/// worker・planner・WU の run の `RunContext.lineage`（`task` の祖先、案件の依頼と、`run_extras` が組んだ子の要約）。
pub(crate) fn lineage(
    store: &SqliteStore,
    task: &Task,
    children: Vec<ChildSummary>,
) -> Result<TaskLineageContext, StoreError> {
    Ok(TaskLineageContext {
        children,
        ..chain_lineage(store, &chain_to_root(store, task)?)?
    })
}
