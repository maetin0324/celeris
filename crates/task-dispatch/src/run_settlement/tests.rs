//! [`super::replan_or_ask`] と [`super::run_end_record`] の単体試験。
//!
//! 既存の試験（`dispatcher/tests` の WU の失敗・遅延 WU・統合の失敗・木の liveness）は経路ごとに 1 つの理由だけを
//! 通して見る。理由が重なったときの優先と、上限のちょうどの境界を経路をまたいで 1 か所で固定する試験は無かった
//! （判定が 3 経路に写されていたため）。ここでその契約を固定する。

use super::{AskReason, ReplanBan, ReplanLimits, ReplanOrAsk, replan_or_ask, run_end_record};
use task_core::{Event, HarnessErrorClass, RunEnd, RunMetrics, RunRole, Usage};
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

/// 不変条件（D4 run-close）: 終了記録は呼ぶ経路が渡した run id・outcome の文・usage・role・metrics・終端を
/// そのまま運ぶ。欄を落とさず、取り違えず、終端も role も補わない（`None` は `None` のまま。reviewer run の終端は
/// 後で経路が書き戻す）。既存の試験は経路ごとに dispatcher を通して events を見るので、共有の組み立てが欄を落とす・
/// 取り違える変化は、その欄をその経路で assert する試験があるときだけ検出される。全欄を経路によらず 1 か所で固定する
/// 試験は無かった（組み立てが閉じる経路ごとに写されていたため）。
#[test]
fn run_end_record_carries_every_input_as_given() {
    let usage = Usage {
        input_tokens: Some(120),
        output_tokens: Some(30),
        cost_usd: Some(0.25),
        ..Usage::default()
    };
    let metrics = RunMetrics {
        wall_ms: 4_000,
        retries: 2,
        turns: Some(7),
        ..RunMetrics::default()
    };
    let end = RunEnd::HarnessError {
        class: HarnessErrorClass::Infra,
    };
    let record = run_end_record(
        "run-a",
        "infra_requeue: lease expired".to_string(),
        Some(usage),
        Some(RunRole::Planner),
        Some(metrics),
        Some(end),
    );
    assert_eq!(
        record,
        Event::WorkerFinished {
            run_id: "run-a".to_string(),
            outcome: "infra_requeue: lease expired".to_string(),
            usage: Some(usage),
            role: Some(RunRole::Planner),
            metrics: Some(metrics),
            end: Some(end),
        }
    );

    let bare = run_end_record(
        "run-b",
        "interrupted: comment".to_string(),
        None,
        None,
        None,
        None,
    );
    assert_eq!(
        bare,
        Event::WorkerFinished {
            run_id: "run-b".to_string(),
            outcome: "interrupted: comment".to_string(),
            usage: None,
            role: None,
            metrics: None,
            end: None,
        }
    );
}
