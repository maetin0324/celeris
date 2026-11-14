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

// ---- settle（ADR 2026-11-11 dispatcher-state-ownership D3） ----
//
// 既存の worker 完了の試験（`dispatcher/tests`）は dispatcher を通して 1 つの終わり方ごとに遷移を見る。分類の境界
// （requeue の上限ちょうど・infra 再試行の上限・browser の前提の block が WU の run では効かないこと・check の不合格の
// usage の補い）は経路の試験ではそれぞれ片側しか通らず、入力から提案を 1 か所で固定する試験は無かった。

use super::{
    RunFinishInput, RunFinishProposal, TreeTokens, questions_of_run, settle, stop_at_token_budget,
    trigger_starts_another_run,
};
use crate::policy::ProviderOutcome;
use task_core::browser_prerequisite::BrowserPrerequisiteCode;
use task_core::{BudgetKind, Trigger};
use task_worker::{AdapterError, RunOutcome, Terminal};

fn input(result: Result<RunOutcome, AdapterError>) -> RunFinishInput<'static> {
    RunFinishInput {
        result,
        events: &[],
        run_id: "run-1",
        conversation: false,
        work_unit: false,
        supply_failure: None,
        browser_prereq: None,
        cluster_wait: None,
        max_requeues: 3,
        infra_retry: (1, Some(std::time::Duration::from_secs(30))),
        check_failure: None,
    }
}

fn adapter_failure(
    max_requeues: u32,
    infra_retry: (u32, Option<std::time::Duration>),
    supply_failure: Option<ProviderOutcome>,
) -> RunFinishProposal {
    settle(RunFinishInput {
        supply_failure,
        max_requeues,
        infra_retry,
        ..input(Err(AdapterError::Other("boom".to_string())))
    })
}

const RETRY: (u32, Option<std::time::Duration>) = (1, Some(std::time::Duration::from_secs(30)));

/// 不変条件: 供給側の失敗は連続 requeue 数が上限未満なら attempts を消費せず requeue し、上限ちょうどで retryable な失敗
/// にする（ADR-0010 D5・ADR-0011）。認証失敗は上限に数えない（ADR 2026-10-17）。どの場合も quota と cooldown には供給側の
/// 分類を渡す。既存の試験は上限の手前か後の片側しか見ず、`<` が `<=` に変わっても、認証失敗の例外が落ちても検出できない。
#[test]
fn settle_requeues_supply_failures_below_the_limit() {
    let throttled = ProviderOutcome::Throttled {
        retry_after: std::time::Duration::from_secs(5),
    };
    let below = adapter_failure(1, RETRY, Some(throttled.clone()));
    assert_eq!(below.trigger, Trigger::Requeue);
    assert_eq!(below.outcome, "requeue: adapter: boom");
    assert_eq!(below.provider_outcome, throttled);
    assert_eq!(below.run_end, None);

    let at_limit = adapter_failure(0, RETRY, Some(throttled.clone()));
    assert_eq!(at_limit.trigger, Trigger::WorkerError { retryable: true });
    assert_eq!(
        at_limit.outcome,
        "error(retryable=true): requeue limit (0) reached: adapter: boom"
    );
    assert_eq!(at_limit.provider_outcome, throttled);

    let auth = adapter_failure(0, RETRY, Some(ProviderOutcome::AuthFailed));
    assert_eq!(auth.trigger, Trigger::Requeue);
    assert_eq!(auth.provider_outcome, ProviderOutcome::AuthFailed);
}

/// 不変条件: 分類できない失敗は、D4 retry-backoff の共有の関数 `infra_retry_after` が待ち時間を返した間だけ `InfraRequeue`
/// にし、backoff の (n, 待ち時間) を提案に載せる。待ち時間が無ければ（上限を超えた）`WorkerError{retryable:false}` で
/// `"infra failure ×N"` を付け、backoff を載せない（ADR-0070 D3）。既存の試験は dispatcher の backoff の期限か遷移の
/// 片方しか見ず、提案の (n, 待ち時間) と打ち切りの文を同時に固定しない。
#[test]
fn settle_retries_infra_failures_until_the_limit() {
    let retry = adapter_failure(3, RETRY, None);
    assert_eq!(retry.trigger, Trigger::InfraRequeue);
    assert_eq!(retry.outcome, "infra_requeue: adapter: boom");
    assert_eq!(
        retry.infra_requeue,
        Some((1, std::time::Duration::from_secs(30)))
    );
    assert_eq!(retry.run_end, None);

    let stop = adapter_failure(3, (4, None), None);
    assert_eq!(stop.trigger, Trigger::WorkerError { retryable: false });
    assert_eq!(
        stop.outcome,
        format!("{}4: adapter: boom", task_ops::derive::INFRA_FAILURE_MARKER)
    );
    assert_eq!(stop.infra_requeue, None);
    assert_eq!(stop.provider_outcome, ProviderOutcome::Ok);
}

/// 不変条件: 台帳由来の browser の前提の失敗は atomic の run だけ `BrowserPrereqBlock` で止め、同じ transaction に残す
/// event を提案に載せる。WU の run では infra 再試行のまま（ADR 2026-10-08-browser-prod-enablement D2）。既存の試験は
/// atomic の run しか通らず、WU の run で block しないことを検出できない。
#[test]
fn settle_blocks_on_browser_prerequisites_only_for_atomic_runs() {
    let code = BrowserPrerequisiteCode::Missing;
    let atomic = settle(RunFinishInput {
        browser_prereq: Some(code),
        ..input(Err(AdapterError::Other("boom".to_string())))
    });
    assert_eq!(atomic.trigger, Trigger::BrowserPrereqBlock);
    assert_eq!(
        atomic.outcome,
        format!("browser_prerequisite: {}: adapter: boom", code.as_str())
    );
    assert_eq!(
        atomic.browser_prereq,
        Some(Event::BrowserPrerequisiteBlocked {
            code,
            message: code.message().to_string(),
        })
    );
    assert_eq!(atomic.infra_requeue, None);

    let unit = settle(RunFinishInput {
        browser_prereq: Some(code),
        work_unit: true,
        ..input(Err(AdapterError::Other("boom".to_string())))
    });
    assert_eq!(unit.trigger, Trigger::InfraRequeue);
    assert_eq!(unit.browser_prereq, None);
}

/// 不変条件: 構造化されていない retryable な `Error` でも予算切れの語彙なら `RunEnd::BudgetExhausted` にし、retryable で
/// ない `Error` は語彙に依らず `Failed` にする（ADR-0072 D7）。check の不合格ですり替えた run は worker の usage を残し、
/// outcome の文に足す要約を返す（ADR-0079 付記 R7-5 D2・D4）。既存の試験は語彙の分類を retryable の片側でしか見ない。
#[test]
fn settle_classifies_errors_and_check_failures() {
    let error = |message: &str, retryable: bool| {
        Ok(RunOutcome {
            terminal: Terminal::Error {
                message: message.to_string(),
                retryable,
                usage: None,
            },
            exit_code: None,
        })
    };
    let budget = settle(input(error("hit max_turns", true)));
    assert_eq!(
        budget.run_end,
        Some(RunEnd::BudgetExhausted {
            kind: BudgetKind::Turns
        })
    );
    assert_eq!(budget.trigger, Trigger::WorkerError { retryable: true });
    let fatal = settle(input(error("hit max_turns", false)));
    assert_eq!(fatal.run_end, Some(RunEnd::Failed { retryable: false }));
    assert!(!fatal.misplaced);

    let usage = Usage {
        input_tokens: Some(3),
        ..Usage::default()
    };
    let checked = settle(RunFinishInput {
        check_failure: Some((Some(usage), "checks failed in /w: exit 1".to_string())),
        ..input(error("checks failed", true))
    });
    assert_eq!(checked.usage, Some(usage));
    assert_eq!(
        checked.check_failure_suffix,
        ": checks failed in /w: exit 1"
    );
    assert_eq!(settle(input(error("x", true))).check_failure_suffix, "");
}

/// 不変条件: 部をまたぐ質問は run の自己申告の終わり方より優先し、`WorkerQuestion` でない終わり方を置き換える。既に
/// 質問なら置き換えず、run の質問の後に部またぎの質問を足す（ADR-0033 D4・Phase 27）。既存の試験は置き換える側しか見ない。
#[test]
fn questions_of_run_puts_cross_department_questions_first_in_priority() {
    let cross = vec!["部をまたぐ: A".to_string()];
    let replaced = questions_of_run(&Trigger::WorkerDone, "done: ok", &cross);
    assert_eq!(
        replaced.replaced_outcome.as_deref(),
        Some("question: 部をまたぐ: A")
    );
    assert_eq!(replaced.questions, cross);

    let asked = questions_of_run(&Trigger::WorkerQuestion, "question: Q", &cross);
    assert_eq!(asked.replaced_outcome, None);
    assert_eq!(
        asked.questions,
        vec!["Q".to_string(), "部をまたぐ: A".to_string()]
    );

    let none = questions_of_run(&Trigger::WorkerDone, "done: ok", &[]);
    assert_eq!(none.replaced_outcome, None);
    assert!(none.questions.is_empty());
}

/// 不変条件: 木の消費（この run の usage を含む）が `max_tokens` ちょうどで止め、1 手前では止めない
/// （ADR 2026-10-20-mlsys-main2-fixes D1-4）。止めるのは次の run か reviewer を起こす trigger だけ。既存の試験は上限を
/// 超えた場合しか見ず、比較が `>` に変わっても、この run の usage を足し忘れても検出できない。
#[test]
fn stop_at_token_budget_stops_at_the_limit_including_this_run() {
    let run = Some(Usage {
        input_tokens: Some(4),
        output_tokens: Some(1),
        ..Usage::default()
    });
    let stop = stop_at_token_budget(TreeTokens {
        prior: 5,
        run,
        max: 10,
    });
    assert_eq!(
        stop.map(|s| (s.trigger, s.outcome)),
        Some((
            Trigger::TokenBudgetExhausted,
            "token_budget_exhausted: 10/10".to_string()
        ))
    );
    assert_eq!(
        stop_at_token_budget(TreeTokens {
            prior: 4,
            run,
            max: 10
        }),
        None
    );
    assert!(trigger_starts_another_run(&Trigger::InfraRequeue));
    assert!(!trigger_starts_another_run(&Trigger::WorkerQuestion));
    assert!(!trigger_starts_another_run(&Trigger::WorkerError {
        retryable: false
    }));
}
