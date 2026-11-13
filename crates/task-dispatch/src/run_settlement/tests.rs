//! [`super::replan_or_ask`] の単体試験。
//!
//! 既存の試験（`dispatcher/tests` の WU の失敗・遅延 WU・統合の失敗・木の liveness）は経路ごとに 1 つの理由だけを
//! 通して見る。理由が重なったときの優先と、上限のちょうどの境界を経路をまたいで 1 か所で固定する試験は無かった
//! （判定が 3 経路に写されていたため）。ここでその契約を固定する。

use super::{AskReason, ReplanBan, ReplanLimits, ReplanOrAsk, replan_or_ask};
use task_ops::plan_gate::STALLED_REPLAN_LIMIT;

fn limits(stalled: u32, replans: u32, max_replans: u32, ban: Option<ReplanBan>) -> ReplanLimits {
    ReplanLimits {
        stalled,
        replans,
        max_replans,
        ban,
    }
}

/// 不変条件: 上限に余りがあり、stalled が上限未満で、禁止が無ければ replan する（`replans < max_replans`）。
/// 上限ちょうど（`replans == max_replans`）と上限 0 は使い切り。既存の経路の試験は上限の手前か後の片側しか見ず、
/// 比較が `<=` に変わっても検出できない。
#[test]
fn replan_or_ask_replans_only_below_the_limit() {
    assert_eq!(replan_or_ask(limits(0, 0, 1, None)), ReplanOrAsk::Replan);
    assert_eq!(replan_or_ask(limits(0, 2, 3, None)), ReplanOrAsk::Replan);
    assert_eq!(
        replan_or_ask(limits(0, 3, 3, None)),
        ReplanOrAsk::Ask(AskReason::Exhausted)
    );
    assert_eq!(
        replan_or_ask(limits(0, 4, 3, None)),
        ReplanOrAsk::Ask(AskReason::Exhausted)
    );
    assert_eq!(
        replan_or_ask(limits(0, 0, 0, None)),
        ReplanOrAsk::Ask(AskReason::Exhausted)
    );
}

/// 不変条件: 同じ失敗の自動 replan が `STALLED_REPLAN_LIMIT` 回続いたら、上限に余りがあっても人に聞く
/// （ADR 2026-10-22-stalled-auto-replan D4）。1 回手前は replan する。stalled は禁止と使い切りより先に見る
/// （統合の失敗が health で禁止されていても stalled の質問文を出す）。既存の試験は stalled と禁止が重なる場合を見ない。
#[test]
fn replan_or_ask_stalled_wins_over_ban_and_limit() {
    let below = STALLED_REPLAN_LIMIT - 1;
    assert_eq!(
        replan_or_ask(limits(below, 0, 5, None)),
        ReplanOrAsk::Replan
    );
    assert_eq!(
        replan_or_ask(limits(STALLED_REPLAN_LIMIT, 0, 5, None)),
        ReplanOrAsk::Ask(AskReason::Stalled)
    );
    assert_eq!(
        replan_or_ask(limits(
            STALLED_REPLAN_LIMIT,
            0,
            5,
            Some(ReplanBan::HealthStop)
        )),
        ReplanOrAsk::Ask(AskReason::Stalled)
    );
    assert_eq!(
        replan_or_ask(limits(STALLED_REPLAN_LIMIT + 1, 5, 5, None)),
        ReplanOrAsk::Ask(AskReason::Stalled)
    );
}

/// 不変条件: replan を禁止する理由（health の停止）があれば、上限に余りがあっても replan しない。禁止は上限の
/// 使い切りより先に見る（統合の失敗の質問は health の文のまま）。既存の試験は禁止と使い切りが重なる場合を見ない。
#[test]
fn replan_or_ask_ban_wins_over_the_limit() {
    let ban = Some(ReplanBan::HealthStop);
    assert_eq!(
        replan_or_ask(limits(0, 0, 5, ban)),
        ReplanOrAsk::Ask(AskReason::Banned)
    );
    assert_eq!(
        replan_or_ask(limits(0, 5, 5, ban)),
        ReplanOrAsk::Ask(AskReason::Banned)
    );
}
