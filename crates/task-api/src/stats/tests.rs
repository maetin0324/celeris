use super::*;
use task_core::{TaskId, Usage};

fn row(id: u64, ts: &str, event: Event) -> EventRow {
    EventRow {
        id,
        task_id: TaskId::new(),
        seq: 0,
        ts: ts.to_string(),
        event,
    }
}

fn started(run_id: &str, provider: Option<&str>) -> Event {
    Event::WorkerStarted {
        run_id: run_id.into(),
        adapter: "fake".into(),
        model: "m".into(),
        provider: provider.map(str::to_string),
        account: None,
        role: None,
        task_role: None,
    }
}

fn finished(run_id: &str, outcome: &str, usage: Option<Usage>) -> Event {
    Event::WorkerFinished {
        run_id: run_id.into(),
        outcome: outcome.into(),
        usage,
        role: None,
        metrics: None,
        end: None,
    }
}

#[test]
fn outcome_prefixes_are_classified() {
    assert_eq!(classify_outcome("done: ok", None), RunOutcomeKind::Done);
    assert_eq!(
        classify_outcome("question: which?", None),
        RunOutcomeKind::Question
    );
    assert_eq!(
        classify_outcome("requeue: throttled", None),
        RunOutcomeKind::Requeue
    );
    assert_eq!(
        classify_outcome("lease_expired", None),
        RunOutcomeKind::LeaseExpired
    );
    assert_eq!(
        classify_outcome("lease_expired: x", None),
        RunOutcomeKind::Error
    );
    // ADR-0044 D2/D8（Phase 53）: 人のコメントで止めた run は失敗ではない。
    assert_eq!(
        classify_outcome("interrupted: comment", None),
        RunOutcomeKind::Interrupted
    );
    assert_eq!(
        classify_outcome("error(retryable=true): boom", None),
        RunOutcomeKind::Error
    );
    // ADR-0070 D3 / P-E0-3: `infra_requeue: ` も `Requeue` に数える。
    assert_eq!(
        classify_outcome("infra_requeue: adapter: boom", None),
        RunOutcomeKind::Requeue
    );
    // ADR-0072 D9/D11/D19（Phase E1）: `continue: ` は continuation（失敗ではない）。
    assert_eq!(
        classify_outcome(
            "continue: budget_exhausted(turns) の続き（Run #2）",
            Some(&task_core::RunEnd::BudgetExhausted {
                kind: task_core::BudgetKind::Turns
            })
        ),
        RunOutcomeKind::Continued
    );
    assert_eq!(
        classify_outcome("continue: yielded の続き（Run #2）", None),
        RunOutcomeKind::Continued,
        "end が無くても接頭辞だけで分類できる"
    );
}

#[test]
fn runs_are_attributed_to_providers_with_daily_usage() {
    let mut stats = StatsState::default();
    let usage = |i, o| {
        Some(Usage {
            input_tokens: Some(i),
            output_tokens: o,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            cost_usd: None,
        })
    };
    stats.apply(&row(
        1,
        "2026-09-13T23:00:00Z",
        started("r1", Some("claude-a")),
    ));
    stats.apply(&row(
        2,
        "2026-09-14T00:30:00+09:00",
        finished("r1", "done: ok", usage(10, Some(5))),
    ));
    stats.apply(&row(3, "2026-09-14T01:00:00Z", started("r2", None)));
    stats.apply(&row(
        4,
        "2026-09-14T02:00:00Z",
        finished("r2", "requeue: throttled", usage(1, None)),
    ));
    stats.apply(&row(
        5,
        "2026-09-14T03:00:00Z",
        started("r3", Some("claude-a")),
    ));
    stats.apply(&row(
        2,
        "2026-09-14T03:00:00Z",
        finished("r3", "done: dup", None),
    ));

    let today = Date::from_calendar_date(2026, time::Month::September, 14).unwrap_or(Date::MIN);
    let a = stats.view("claude-a", today);
    assert_eq!(
        (a.runs, a.done, a.input_tokens, a.output_tokens),
        (2, 1, 10, 5)
    );
    assert_eq!(a.by_day.len(), 1);
    assert_eq!(a.by_day[0].day, "2026-09-13");
    let unknown = stats.view("unknown", today);
    assert_eq!(
        (unknown.runs, unknown.requeue, unknown.input_tokens),
        (1, 1, 1)
    );
    assert_eq!(stats.view("nobody", today), ProviderStats::default());

    let later = Date::from_calendar_date(2026, time::Month::November, 1).unwrap_or(Date::MIN);
    assert!(stats.view("claude-a", later).by_day.is_empty());
}

/// ADR-0014 D1（P-G14）: Reviewer run（role: reviewer）もプロバイダの集計に入る。
#[test]
fn reviewer_runs_are_counted_for_their_provider() {
    let mut stats = StatsState::default();
    let role = Some(task_core::RunRole::Reviewer);
    stats.apply(&row(
        1,
        "2026-09-14T00:00:00Z",
        Event::WorkerStarted {
            run_id: "rev".into(),
            adapter: "fake".into(),
            model: "m".into(),
            provider: Some("claude-b".into()),
            account: None,
            role,
            task_role: None,
        },
    ));
    stats.apply(&row(
        2,
        "2026-09-14T00:01:00Z",
        Event::WorkerFinished {
            run_id: "rev".into(),
            outcome: "done: reviewed".into(),
            usage: Some(Usage {
                input_tokens: Some(3),
                output_tokens: Some(4),
                cache_read_tokens: None,
                cache_creation_tokens: None,
                cost_usd: None,
            }),
            role,
            metrics: None,
            end: None,
        },
    ));
    let today = Date::from_calendar_date(2026, time::Month::September, 14).unwrap_or(Date::MIN);
    let b = stats.view("claude-b", today);
    assert_eq!(
        (b.runs, b.done, b.input_tokens, b.output_tokens),
        (1, 1, 3, 4)
    );
}

fn started_with_account(run_id: &str, provider: Option<&str>, account: Option<&str>) -> Event {
    Event::WorkerStarted {
        run_id: run_id.into(),
        adapter: "claude-code".into(),
        model: "m".into(),
        provider: provider.map(str::to_string),
        account: account.map(str::to_string),
        role: None,
        task_role: None,
    }
}

/// ADR-0024: `WorkerStarted.account` と対応する `WorkerFinished` からアカウント別の集計を作る。
/// プールを使わない run（`account: None`）は集計に入らない。
#[test]
fn account_stats_are_attributed_by_account_and_ignore_pool_less_runs() {
    let mut stats = AccountStatsState::default();
    let usage = |i, o| {
        Some(Usage {
            input_tokens: Some(i),
            output_tokens: o,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            cost_usd: None,
        })
    };
    stats.apply(&row(
        1,
        "2026-09-14T00:00:00Z",
        started_with_account("r1", Some("pool"), Some("b")),
    ));
    stats.apply(&row(
        2,
        "2026-09-14T00:01:00Z",
        finished("r1", "done: ok", usage(10, Some(5))),
    ));
    stats.apply(&row(
        3,
        "2026-09-14T00:02:00Z",
        started_with_account("r2", Some("pool"), Some("b")),
    ));
    stats.apply(&row(
        4,
        "2026-09-14T00:03:00Z",
        finished("r2", "error(retryable=false): boom", None),
    ));
    // プールを使わない run: account が無いので集計に入らない。
    stats.apply(&row(
        5,
        "2026-09-14T00:04:00Z",
        started_with_account("r3", Some("other"), None),
    ));
    stats.apply(&row(
        6,
        "2026-09-14T00:05:00Z",
        finished("r3", "done: ok", None),
    ));

    let b = stats.view("claude-code", "b");
    assert_eq!(
        (b.runs, b.done, b.error, b.input_tokens, b.output_tokens),
        (2, 1, 1, 10, 5)
    );
    assert_eq!(
        stats.view("claude-code", "a"),
        crate::types::AccountStats::default()
    );
    // 違うアダプタの同じ id は別のアカウントとして扱う（ADR-0025 D1）。
    assert_eq!(
        stats.view("codex", "b"),
        crate::types::AccountStats::default()
    );
}

/// S5: `error` は `RunOutcomeKind::Error` だけを数える。`question`/`requeue`/`lease_expired` はエラーではない
/// （§5.8 のプロバイダ集計と同じ規則）。
#[test]
fn account_stats_error_only_counts_the_error_outcome_kind() {
    let mut stats = AccountStatsState::default();
    stats.apply(&row(
        1,
        "2026-09-14T00:00:00Z",
        started_with_account("r1", Some("pool"), Some("b")),
    ));
    stats.apply(&row(
        2,
        "2026-09-14T00:01:00Z",
        finished("r1", "question: which?", None),
    ));
    stats.apply(&row(
        3,
        "2026-09-14T00:02:00Z",
        started_with_account("r2", Some("pool"), Some("b")),
    ));
    stats.apply(&row(
        4,
        "2026-09-14T00:03:00Z",
        finished("r2", "requeue: throttled", None),
    ));
    stats.apply(&row(
        5,
        "2026-09-14T00:04:00Z",
        started_with_account("r3", Some("pool"), Some("b")),
    ));
    stats.apply(&row(
        6,
        "2026-09-14T00:05:00Z",
        finished("r3", "lease_expired", None),
    ));
    stats.apply(&row(
        7,
        "2026-09-14T00:06:00Z",
        started_with_account("r4", Some("pool"), Some("b")),
    ));
    stats.apply(&row(
        8,
        "2026-09-14T00:07:00Z",
        finished("r4", "error(retryable=true): boom", None),
    ));

    let b = stats.view("claude-code", "b");
    assert_eq!((b.runs, b.done, b.error), (4, 0, 1));
}
