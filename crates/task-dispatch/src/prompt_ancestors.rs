//! ADR 2026-11-21-ancestor-objective-in-prompts D1: run の `RunContext.lineage.ancestors`（祖先の task の objective と受入
//! 条件の全文）を store から組む。worker・planner・WU の run（`Dispatcher::run_extras`）、final review の run
//! （`Dispatcher::review_human_inputs`）、設計 review の run（`health_review::design_gate`）が同じこの関数を使う。store を
//! 読むだけで、LLM を呼ばない。

use task_core::{SqliteStore, StoreError, Task};
use task_worker::protocol::{AncestorContext, ChildSummary, TaskLineageContext};

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

/// worker・planner・WU の run の `RunContext.lineage`（`task` の祖先と、`run_extras` が組んだ子の要約）。
pub(crate) fn lineage(
    store: &SqliteStore,
    task: &Task,
    children: Vec<ChildSummary>,
) -> Result<TaskLineageContext, StoreError> {
    Ok(TaskLineageContext {
        ancestors: ancestor_contexts(&chain_to_root(store, task)?),
        children,
    })
}
