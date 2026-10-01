//! 認可（ADR-0033 D5。Phase 26）。
//!
//! run が `Question` で終わったとき（既存の「人への質問」の流れ — `Trigger::WorkerQuestion` /
//! `Status::Blocked` / `answers[]` はそのまま）に、`approvals` の行を 1 件追記する。**LLM は呼ばない**
//! （DESIGN 原則 1）: 追記する文面は run の自己申告（または部をまたぐ委譲の質問）そのままで、判断は
//! 「誰宛てにするか」（`task.assignee`、無ければ秘書）だけの決定的な処理。

use task_core::approval::{Approval, ApprovalId};
use task_core::{OrgKind, StoreError, Task, TaskStore};
use time::OffsetDateTime;

/// 質問の宛先ノード: `task.assignee`、無ければ秘書（組織に無ければ `None`）。SPEC §3.1 / ADR-0033 D5。
fn question_node_id(store: &dyn TaskStore, task: &Task) -> Result<Option<String>, StoreError> {
    if let Some(assignee) = &task.assignee {
        return Ok(Some(assignee.clone()));
    }
    let org = store.org_list()?;
    Ok(org
        .into_iter()
        .find(|n| n.kind == OrgKind::Secretary)
        .map(|n| n.id))
}

/// `Question` で終わった run を `approvals` に 1 件追記する（組織がまだ無い = 種を蒔いていない DB では
/// 宛先が決められないので何もしない）。
///
/// Phase 27: **同じタスク・同じ文面の未決の行があれば増やさない**（部をまたぐ委譲は run をやり直すたびに
/// 同じ質問が上がるので、認可の一覧が同じ行で埋まらないようにする。決定的な文字列の一致だけで判断する）。
pub(crate) fn record_question_approval(
    store: &dyn TaskStore,
    task: &Task,
    text: &str,
    now: OffsetDateTime,
) -> Result<Option<Approval>, StoreError> {
    let Some(node_id) = question_node_id(store, task)? else {
        return Ok(None);
    };
    // Phase F7: 質問の遷移と追記の間に人がタスクを取り消した（終端になった）なら、答える相手が
    // いない要求を作らない（終端への遷移は既存の未決の行を閉じるが、後から足された行は閉じられない）。
    if store
        .get(task.id)?
        .is_some_and(|current| current.status.is_terminal())
    {
        return Ok(None);
    }
    if let Some(open) = store
        .approval_list(Some(true), None, Some(&node_id))?
        .into_iter()
        .find(|a| a.task_id == Some(task.id) && a.question == text)
    {
        return Ok(Some(open));
    }
    let approval = Approval {
        id: ApprovalId::new(),
        project_id: task.project_id,
        node_id,
        task_id: Some(task.id),
        question: text.to_string(),
        decision: None,
        answer: None,
        created_at: now,
        decided_at: None,
    };
    store.approval_append(&approval)?;
    Ok(Some(approval))
}

/// Phase F7（tick の照合）: 未決のまま認可元のタスクが終端になっている要求を `withdrawn` で閉じる
/// （F7 より前の残りと、遷移と追記の競合の取りこぼし。通常は終端への遷移が同じトランザクションで閉じる）。
/// 決定的な SQL だけで、LLM は呼ばない。閉じた件数を返す。
pub(crate) fn withdraw_stale_approvals(
    store: &dyn TaskStore,
    now: OffsetDateTime,
) -> Result<usize, StoreError> {
    let withdrawn = store.approval_withdraw_stale(now)?;
    let mut n = 0;
    for w in &withdrawn {
        n += w.approval_ids.len();
        tracing::info!(
            task_id = %w.task_id,
            task_status = ?w.task_status,
            approvals = w.approval_ids.len(),
            "withdrew pending approvals of a terminal task (reconcile)"
        );
    }
    Ok(n)
}

#[cfg(test)]
mod tests;
