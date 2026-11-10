//! ADR 2026-11-08-health-enforcement-loop D7.3・決定 p9-scope: 依存先の終端からの復帰で戻した後続を受信箱に出す。
//!
//! - [`attention_items`]: 受信箱の `dependents_reopened`（`AttentionItem::DependentsReopened`）を、各依存先 A の最新の
//!   `Event::DependentsReopened`（`TaskStore::latest_event_rows` の部分 index。events の全走査をしない）から導く。
//!   戻した後続（`restored`）のうち、まだ動いていないもの（今 draft・ready で、最後の遷移が `dependency_reopened`）が
//!   1 件以上あるときだけ出し、それらが動き出すか終端になれば消える。人は重複した後続を cancel する（答えは
//!   `POST /tasks/{id}/cancel`）。
//! - 戻さなかった後続（`kept`）は項目の中に理由と並べるだけで、`kept` だけの event は項目を出さない。retry の後継が
//!   ある後続は重複を防いだので答えることが無く、ほかの依存先が未解決の後続はその依存先が戻ったときに `restored`
//!   として出る（ADR-0133 の「答えると消える」）。
//!
//! 新しい表・config の欄は無く、正本は追記専用の events。
use std::collections::{HashMap, HashSet};

use schemars::JsonSchema;
use serde::Serialize;
use task_core::store::LatestEventKind;
use task_core::{Event, Status, Task, TaskId, TaskStore, Trigger};

use super::AttentionItem;
use crate::error::OpsError;
use crate::view::{self, TaskRef};

/// `AttentionItem::DependentsReopened.pending[]`: 戻した後続のうちまだ動いていない 1 件。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct PendingDependent {
    pub task: TaskRef,
    /// 戻した時点の後続の依存先（`RestoredDependent.depends_on`）。
    pub depends_on: Vec<TaskId>,
}

/// 受信箱の `dependents_reopened` の元。依存先 A ごとに最新の `DependentsReopened` だけを使う（1 回の問い合わせ）。
/// 最後の遷移を読むのは、`restored` のうち今 draft・ready の後続だけ（1 件につき 1 回）。`focus` の組み立て（P4-C）
/// では、A か `restored` の後続が範囲にある event だけを見る（項目が触れる task は A・A の root・戻した後続）。
pub(super) fn attention_items(
    store: &dyn TaskStore,
    by_id: &HashMap<TaskId, Task>,
    focus: Option<&HashSet<TaskId>>,
) -> Result<Vec<AttentionItem>, OpsError> {
    let mut items = Vec::new();
    for row in store.latest_event_rows(LatestEventKind::DependentsReopened)? {
        let Event::DependentsReopened { restored, kept } = row.event else {
            continue;
        };
        let Some(dependency) = by_id.get(&row.task_id) else {
            continue;
        };
        if focus.is_some_and(|f| {
            !f.contains(&dependency.id) && !restored.iter().any(|r| f.contains(&r.task_id))
        }) {
            continue;
        }
        let mut pending = Vec::new();
        for r in restored {
            let Some(dependent) = by_id.get(&r.task_id) else {
                continue;
            };
            if matches!(dependent.status, Status::Draft | Status::Ready)
                && last_reason_is_reopen(store, dependent.id)?
            {
                pending.push(PendingDependent {
                    task: view::task_ref(dependent),
                    depends_on: r.depends_on,
                });
            }
        }
        if pending.is_empty() {
            continue;
        }
        items.push(AttentionItem::DependentsReopened {
            task: view::task_ref(dependency),
            pending,
            kept,
            seq: row.seq,
            at: row.ts,
        });
    }
    Ok(items)
}

/// 後続の最後の `Transitioned` の理由が `dependency_reopened` か。戻した後に一度でも動いた（dispatch・人の遷移・
/// もう一度の cancel）後続は、今また draft・ready でも戻した後続として出さない。
fn last_reason_is_reopen(store: &dyn TaskStore, id: TaskId) -> Result<bool, OpsError> {
    let reopened = Trigger::DependencyReopened { draft: false }.name();
    let rows = store.event_rows_for(id, None, view::ALL_EVENTS)?;
    let last = rows.iter().rev().find_map(|r| match &r.event {
        Event::Transitioned { reason, .. } => Some(reason.as_str()),
        _ => None,
    });
    Ok(last == Some(reopened))
}

#[cfg(test)]
mod tests;
