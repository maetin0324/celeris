//! ADR 2026-10-04-multi-objective-model-routing §6・§10 Phase 3: task の events から run ごとの結果
//! （review・受け入れ検査・cash・wall・retries）を投影し、まだ追記されていない `routing_outcome_recorded` を
//! append する口。投影の規則（reward 式・supersede・未判定は None）は `task_core::model_router::feedback` が持つ。
//!
//! 呼び出し口は review の判定・WU 受け入れ検査・統合検査の完了・終端・reopen の後。同じ events から何度呼んでも、
//! 同じ `outcome_id` は二度追記しない（冪等）。events は追記のみ（書き換え・削除はしない）。

use task_core::model_router::feedback::{RewardNormalization, pending_run_outcomes};
use task_core::{Event, TaskId, TaskStore};

use crate::error::OpsError;

/// task の run の outcome のうち、まだ追記されていないものを `routing_outcome_recorded` として追記する。
/// 戻り値は今回追記した件数。
pub fn record_routing_outcomes<S: TaskStore + ?Sized>(
    store: &S,
    task_id: TaskId,
) -> Result<usize, OpsError> {
    let events: Vec<Event> = store
        .events_for(task_id)?
        .into_iter()
        .map(|(_, event)| event)
        .collect();
    let pending = pending_run_outcomes(&events, &RewardNormalization::default());
    for outcome in &pending {
        store.append_event(
            task_id,
            &Event::RoutingOutcomeRecorded {
                outcome: Box::new(outcome.clone()),
            },
        )?;
    }
    Ok(pending.len())
}

#[cfg(test)]
mod tests;
