//! ADR 2026-11-11 dispatcher-state-ownership D3・D4: run の終わりの判断（純粋関数）。
//!
//! D4 の wu-failure の共有の関数 [`replan_or_ask`] を置く。自動 replan をするか人に聞くかの上限の判定は、
//! WU の失敗（worker 完了）・終わらない WU（遅延 WU の照合）・統合の失敗・仕事の残らない計画の gate・木の liveness の
//! 事実のすべてがこの 1 つで行う。replan の実行・event の追記・質問の文・決定の要求は各経路に残す。
//!
//! この module は入力の値だけを読む。永続化・file・process に触れず、`dispatcher` module を使わない（D2 の 3）。

/// [`replan_or_ask`] の入力。各経路が読んだ値を詰める。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReplanLimits {
    /// 同じ unit・同じ失敗の署名の自動 replan が進展なしに続いた回数（`task_ops::plan_gate::stalled_replans` の結果）。
    /// 失敗の署名を持たない経路（仕事の残らない計画の gate・木の liveness）は 0。
    pub stalled: u32,
    /// `[execution] max_replans` に数える replan の回数（`task_ops::plan_gate::counted_replans`。人の replan は数えない）。
    pub replans: u32,
    /// 節点の replan の上限（`[execution] max_replans` に、`limit:max_replans` の回答で上げた分を足した値）。
    pub max_replans: u32,
    /// replan を禁止する理由。無ければ `None`。
    pub ban: Option<ReplanBan>,
}

/// replan を禁止する理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReplanBan {
    /// health の Error・同じ指摘の往復・health の上限の使い切りで統合が止まった
    /// （ADR 2026-11-08-health-enforcement-loop D3.4・D4.3・D4.4。`health_review::check_item::stops_without_replan`）。
    HealthStop,
}

/// [`replan_or_ask`] が返す判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReplanOrAsk {
    /// 自動 replan をする。
    Replan,
    /// replan せず人に聞く（質問か、木の節点では `limit:max_replans` の決定の要求。どちらにするかは経路が決める）。
    Ask(AskReason),
}

/// 人に聞く理由。数（`stalled`・`replans`）は入力の [`ReplanLimits`] にある。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AskReason {
    /// 同じ失敗の自動 replan が進展なしに `task_ops::plan_gate::STALLED_REPLAN_LIMIT` 回以上続いた
    /// （ADR 2026-10-22-stalled-auto-replan D4。`max_replans` に余りがあっても止める）。
    Stalled,
    /// replan を禁止する理由がある（[`ReplanBan`]）。
    Banned,
    /// replan の上限を使い切った（ADR-0079 付記「R6-1」D3・D9）。
    Exhausted,
}

/// D4 wu-failure: 自動 replan をするか、人に聞くか。
///
/// 理由の優先は stalled → 禁止 → 上限の使い切り。stalled を先に見るのは、統合の失敗が health で禁止されていても
/// 進展の無い繰り返しなら stalled の質問文を出す今の挙動を保つため。
pub(crate) fn replan_or_ask(limits: ReplanLimits) -> ReplanOrAsk {
    if limits.stalled >= task_ops::plan_gate::STALLED_REPLAN_LIMIT {
        return ReplanOrAsk::Ask(AskReason::Stalled);
    }
    if limits.ban.is_some() {
        return ReplanOrAsk::Ask(AskReason::Banned);
    }
    if limits.replans < limits.max_replans {
        ReplanOrAsk::Replan
    } else {
        ReplanOrAsk::Ask(AskReason::Exhausted)
    }
}

#[cfg(test)]
mod tests;
