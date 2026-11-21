//! ADR 2026-11-21-ancestor-objective-in-prompts D1: run の `RunContext.ancestors`（祖先の task の objective と受入条件の全文）
//! を store から組む。worker・planner・WU の run（`dispatcher::worker_task`）と reviewer の run（`dispatcher::review_spawn`）が
//! 同じこの関数を使う。store を読むだけで、LLM を呼ばない。

use task_core::{SqliteStore, Task};
use task_worker::protocol::AncestorContext;

/// `task` の祖先（`parent_id` を root まで辿る。`task_ops::delegate::ancestors` と同じ 64 hop の上限）を root が先頭の順で
/// 返す。親の無い task は空。store に無い祖先は飛ばす（辿れた所まで）。
pub(crate) fn ancestor_contexts(
    store: &SqliteStore,
    task: &Task,
) -> Result<Vec<AncestorContext>, task_ops::OpsError> {
    let mut out = Vec::new();
    for id in task_ops::delegate::ancestors(store, task)?
        .into_iter()
        .rev()
    {
        if let Some(ancestor) = store.get(id)? {
            out.push(AncestorContext::from(&ancestor));
        }
    }
    Ok(out)
}
