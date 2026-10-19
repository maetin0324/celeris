//! variant 付き試行の割当前検査（campaign の variant 配線。dispatcher は matching を変えず、前後で検査するだけ）。
//!
//! 検査するのは `CampaignOrgPin` が持つ値（node 集合・coding 課の実効 profile hash）と、割当先が
//! pin の範囲に収まることだけ。全 node digest は観測証拠で、他 node・モデル・情報・予算・認可・grader が
//! 不変であることの証明にはしない（それは campaign 側の封印期待値との照合 = campaign-proof が行う）。

use super::*;
use task_core::variant_wire::CampaignOrgPin;

/// 割当先（matching の結果、または retry で既存の assignee）が pin の範囲か。
/// node 集合が根＋単一作業課（spec-off）なら `coding_node` 以外は拒否する。
pub(super) fn check_assignee(pin: &CampaignOrgPin, assignee: &str) -> Result<(), String> {
    if !pin.nodes.iter().any(|n| n == assignee) {
        return Err(format!(
            "assignee {assignee} is not in the pinned org nodes"
        ));
    }
    if pin.nodes.len() <= 2 && assignee != pin.coding_node {
        return Err(format!(
            "single-section variant: assignee {assignee} is not the pinned coding node {}",
            pin.coding_node
        ));
    }
    Ok(())
}

impl Dispatcher {
    /// task の解決済み campaign 識別の `variant_org`。識別なし・variant なしは `Ok(None)`。
    /// 祖先との矛盾・循環など解決できないときは `Err`（黙って通常 task 扱いにしない）。
    pub(super) fn variant_org_pin(&self, task: &Task) -> Result<Option<CampaignOrgPin>, String> {
        let id = self
            .store
            .task_campaign_resolve(&task.id.to_string())
            .map_err(|e| format!("campaign identity could not be resolved: {e}"))?;
        Ok(id.and_then(|i| i.variant_org))
    }

    /// 割当てずに `ready → blocked` にし、理由を event に残す。人への質問（approval）は作らない。
    pub(super) fn block_variant_task(
        &mut self,
        task: &Task,
        reason: String,
    ) -> Result<Option<Task>, DispatchError> {
        let text = format!("variant pin check failed; not assigned: {reason}");
        let events = vec![Event::QuestionRaised {
            run_id: format!("variant-pin-{}", task.id),
            text,
        }];
        match self
            .store
            .apply_transition_with_events(task.id, Trigger::Unroutable, events)
        {
            Ok(_) => {
                tracing::warn!(task_id = %task.id, %reason, "variant pin check failed; task blocked")
            }
            Err(StoreError::InvalidTransition(e)) => {
                tracing::warn!(task_id = %task.id, error = %e, "variant block transition could not be applied")
            }
            Err(e) => return Err(e.into()),
        }
        Ok(None)
    }
}
