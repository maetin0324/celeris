use super::*;
use AttemptOutcome::*;

fn rec(lane: Tier, outcome: AttemptOutcome) -> AttemptRecord {
    AttemptRecord {
        lane: Some(lane),
        outcome,
    }
}

#[test]
fn escalates_one_step_after_repeated_review_failures_and_is_bounded() {
    let p = EscalationPolicy::default();
    assert_eq!(
        p.decide(&[], Tier::Cheap, BudgetState::Ok).lane(),
        Tier::Cheap
    );
    let h1 = [rec(Tier::Cheap, ReviewFailed)];
    assert!(matches!(
        p.decide(&h1, Tier::Cheap, BudgetState::Ok),
        RetryDecision::Retry {
            lane: Tier::Cheap,
            ..
        }
    ));
    let h2 = [
        rec(Tier::Cheap, ReviewFailed),
        rec(Tier::Cheap, VerificationFailed),
    ];
    assert_eq!(
        p.decide(&h2, Tier::Cheap, BudgetState::Ok),
        RetryDecision::Escalate {
            from: Tier::Cheap,
            to: Tier::Standard,
            reason: "2 consecutive VerificationFailed at Cheap".into()
        }
    );
    // 上げた後はその lane を保つ（base に戻らない）
    let h3 = [
        rec(Tier::Cheap, ReviewFailed),
        rec(Tier::Cheap, ReviewFailed),
        rec(Tier::Standard, ReviewFailed),
    ];
    assert_eq!(
        p.decide(&h3, Tier::Cheap, BudgetState::Ok).lane(),
        Tier::Standard
    );
    // 全体の上限で止まる
    let h4 = [
        rec(Tier::Cheap, ReviewFailed),
        rec(Tier::Cheap, ReviewFailed),
        rec(Tier::Standard, ReviewFailed),
        rec(Tier::Standard, ReviewFailed),
    ];
    assert!(matches!(
        p.decide(&h4, Tier::Cheap, BudgetState::Ok),
        RetryDecision::Stop {
            lane: Tier::Standard,
            ..
        }
    ));
    // 1 回に 1 段だけ（cheap から frontier へ飛ばない）
    for d in [
        p.decide(&h2, Tier::Cheap, BudgetState::Ok),
        p.decide(&h3, Tier::Cheap, BudgetState::Ok),
    ] {
        if let RetryDecision::Escalate { from, to, .. } = d {
            assert_eq!(lane_rank(to), lane_rank(from) + 1);
        }
    }
}

#[test]
fn never_escalates_above_the_ceiling_or_on_supply_side_failures() {
    let p = EscalationPolicy {
        ceiling: LaneCeiling {
            allowed: vec![],
            max_lane: Some(Tier::Standard),
        },
        ..EscalationPolicy::default()
    };
    let h = [
        rec(Tier::Standard, ReviewFailed),
        rec(Tier::Standard, ReviewFailed),
    ];
    let d = p.decide(&h, Tier::Standard, BudgetState::Ok);
    assert_eq!(d.lane(), Tier::Standard);
    assert!(d.reason().contains("ceiling"), "{d:?}");

    let p = EscalationPolicy::default();
    let h = [
        rec(Tier::Cheap, ReviewFailed),
        rec(Tier::Cheap, ReviewFailed),
        rec(Tier::Cheap, SupplySide),
    ];
    let d = p.decide(&h, Tier::Cheap, BudgetState::Ok);
    assert!(
        matches!(
            d,
            RetryDecision::Retry {
                lane: Tier::Cheap,
                ..
            }
        ),
        "{d:?}"
    );
    // 供給側失敗は試行に数えない
    let many = vec![rec(Tier::Cheap, SupplySide); 10];
    assert!(matches!(
        p.decide(&many, Tier::Cheap, BudgetState::Ok),
        RetryDecision::Retry { .. }
    ));
    // ワーカーの失敗では上げない
    let h = [rec(Tier::Cheap, WorkerError), rec(Tier::Cheap, WorkerError)];
    assert_eq!(
        p.decide(&h, Tier::Cheap, BudgetState::Ok).lane(),
        Tier::Cheap
    );
}

#[test]
fn budget_guard_stops_escalation() {
    let p = EscalationPolicy::default();
    let h = [
        rec(Tier::Cheap, ReviewFailed),
        rec(Tier::Cheap, ReviewFailed),
    ];
    assert!(matches!(
        p.decide(&h, Tier::Cheap, BudgetState::Defer),
        RetryDecision::Stop {
            lane: Tier::Cheap,
            ..
        }
    ));
    assert!(matches!(
        p.decide(&h, Tier::Cheap, BudgetState::Exhausted),
        RetryDecision::Stop { .. }
    ));
    let h = [rec(Tier::Cheap, BudgetExhausted)];
    let d = p.decide(&h, Tier::Cheap, BudgetState::Ok);
    assert!(matches!(d, RetryDecision::Stop { .. }), "{d:?}");
}

#[test]
fn for_task_never_exceeds_max_retries_and_honours_the_profile() {
    let mut t = crate::model_policy::tests::task("x", vec![]);
    t.budget.max_retries = 1;
    assert_eq!(EscalationPolicy::for_task(&t, None).max_total_attempts, 2);
    t.budget.max_retries = 9;
    assert_eq!(
        EscalationPolicy::for_task(&t, None).max_total_attempts,
        DEFAULT_MAX_TOTAL_ATTEMPTS
    );
    let profile = crate::profile::EffectiveProfile {
        max_attempts: Some(3),
        review_escalate_on_fail: Some(false),
        max_lane: Some(Tier::Standard),
        ..Default::default()
    };
    let p = EscalationPolicy::for_task(&t, Some(&profile));
    assert_eq!(p.max_total_attempts, 3);
    assert!(!p.escalate_on.contains(&ReviewFailed));
    assert!(p.escalate_on.contains(&VerificationFailed));
    assert_eq!(p.ceiling.max_lane, Some(Tier::Standard));
    let thresholds = EscalationThresholds {
        max_total_attempts: 6,
        ..EscalationThresholds::default()
    };
    assert_eq!(
        EscalationPolicy::for_task_with_thresholds(&t, None, &thresholds).max_total_attempts,
        6
    );
}

#[test]
fn history_is_derived_from_events() {
    use crate::model::{Criterion, Status};
    let mut t = crate::model_policy::tests::task("x", vec![]);
    t.acceptance = vec![
        Criterion {
            text: "tests".into(),
            check: Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
        },
        Criterion {
            text: "judge".into(),
            check: Check::Reviewer,
        },
    ];
    let tr = |reason: &str| Event::Transitioned {
        from: Status::Reviewing,
        to: Status::Ready,
        reason: reason.into(),
    };
    let verdict = |idx: usize| Event::ReviewVerdict {
        run_id: "r".into(),
        criterion_idx: idx,
        pass: false,
        reason: "no".into(),
    };
    let finished = |outcome: &str| Event::WorkerFinished {
        run_id: "w".into(),
        outcome: outcome.into(),
        usage: None,
        role: None,
        metrics: None,
        end: None,
    };
    let events = vec![
        verdict(0),
        tr("review_fail"),
        verdict(1),
        tr("review_fail"),
        tr("requeue"),
        finished("error(retryable=true): wall-clock budget exceeded"),
        tr("worker_error"),
        finished("error(retryable=true): crashed"),
        tr("worker_error"),
    ];
    let h = attempt_history(&t, &events);
    let outcomes: Vec<AttemptOutcome> = h.iter().map(|r| r.outcome).collect();
    assert_eq!(
        outcomes,
        vec![
            VerificationFailed,
            ReviewFailed,
            SupplySide,
            BudgetExhausted,
            WorkerError
        ]
    );
    assert!(h.iter().all(|r| r.lane.is_none()));
    let mut with_reopen = events.clone();
    with_reopen.push(tr("reopen"));
    assert!(attempt_history(&t, &with_reopen).is_empty());
    // ADR 2026-11-14-review-verdict-after-plan-reopen D3: 計画が開き直された review の破棄は試行に数えない。
    let reopened = crate::transition::Trigger::ReviewPlanReopened.reason();
    let mut with_plan_reopened = events.clone();
    with_plan_reopened.push(tr(reopened));
    assert_eq!(attempt_history(&t, &with_plan_reopened), h);
}

#[test]
fn routing_trajectory_escalates_one_lane_and_respects_caps() {
    use crate::model::Status;

    let policy = EscalationPolicy::default();
    let thresholds = EscalationThresholds::default();
    let failures = [
        rec(Tier::Cheap, ReviewFailed),
        rec(Tier::Cheap, VerificationFailed),
    ];
    let audit = policy.decide_trajectory(
        &failures,
        Tier::Cheap,
        TierSource::Default,
        BudgetState::Ok,
        &thresholds,
        "initial",
    );
    assert_eq!(audit.requested_lane, Tier::Cheap);
    assert_eq!(audit.previous_lane, Some(Tier::Cheap));
    assert_eq!(audit.selected_lane, Tier::Standard);
    assert_eq!(audit.counted_failures, 2);
    assert_eq!(audit.interval_id, "initial");

    for source in [TierSource::Human, TierSource::System] {
        let explicit = policy.decide_trajectory(
            &failures,
            Tier::Cheap,
            source,
            BudgetState::Ok,
            &thresholds,
            "initial",
        );
        assert_eq!(explicit.selected_lane, Tier::Cheap);
    }
    for cap in 0..3 {
        let mut limited = thresholds.clone();
        let ceiling = LaneCeiling {
            max_lane: Some(Tier::Cheap),
            ..LaneCeiling::default()
        };
        match cap {
            0 => limited.org_ceiling = ceiling,
            1 => limited.task_ceiling = ceiling,
            _ => limited.work_unit_ceiling = ceiling,
        }
        assert_eq!(
            policy
                .decide_trajectory(
                    &failures,
                    Tier::Cheap,
                    TierSource::Default,
                    BudgetState::Ok,
                    &limited,
                    "initial",
                )
                .selected_lane,
            Tier::Cheap
        );
    }
    let mut tuned = thresholds.clone();
    tuned.quality_failures_per_lane = 3;
    assert_eq!(
        policy
            .decide_trajectory(
                &failures,
                Tier::Cheap,
                TierSource::Default,
                BudgetState::Ok,
                &tuned,
                "initial",
            )
            .selected_lane,
        Tier::Cheap
    );
    let supply = [failures[0], failures[1], rec(Tier::Cheap, SupplySide)];
    assert_eq!(
        policy
            .decide_trajectory(
                &supply,
                Tier::Cheap,
                TierSource::Default,
                BudgetState::Ok,
                &thresholds,
                "initial",
            )
            .selected_lane,
        Tier::Cheap
    );
    for outcome in [WorkerError, BudgetExhausted, LowQuality] {
        let history = [rec(Tier::Cheap, outcome), rec(Tier::Cheap, outcome)];
        assert_eq!(
            policy
                .decide_trajectory(
                    &history,
                    Tier::Cheap,
                    TierSource::Default,
                    BudgetState::Ok,
                    &thresholds,
                    "initial",
                )
                .selected_lane,
            Tier::Cheap
        );
    }
    let second_lane = [failures[0], failures[1], rec(Tier::Standard, ReviewFailed)];
    assert_eq!(
        policy
            .decide_trajectory(
                &second_lane,
                Tier::Cheap,
                TierSource::Default,
                BudgetState::Ok,
                &thresholds,
                "initial",
            )
            .selected_lane,
        Tier::Standard
    );

    let task = crate::model_policy::tests::task("reopen", vec![]);
    let reopen = Event::Transitioned {
        from: Status::Done,
        to: Status::Ready,
        reason: "reopen".into(),
    };
    let review_fail = Event::Transitioned {
        from: Status::Reviewing,
        to: Status::Ready,
        reason: "review_fail".into(),
    };
    let events = [
        review_fail.clone(),
        review_fail.clone(),
        reopen,
        review_fail,
    ];
    let (history, interval_id) = attempt_history_with_interval(&task, &events);
    assert_eq!(interval_id, "reopen:2");
    assert_eq!(history.len(), 1);
    assert_eq!(
        policy
            .decide_trajectory(
                &history,
                Tier::Cheap,
                TierSource::Default,
                BudgetState::Ok,
                &thresholds,
                &interval_id,
            )
            .counted_failures,
        1
    );
}

/// ADR 2026-11-15-research-adapter-deadline D5 の付記: 試行の履歴は追記専用の events の
/// `WorkerFinished.outcome` の文で予算切れを数える（`end` は読まない）。研究 harness の wall clock を
/// `BudgetExhausted{WallClock}` に揃え（D1）、continuation から外した（D3）後も、過去と今の文の分類は
/// 変わらない。既存の `history_is_derived_from_events` は文 1 つ（`wall-clock budget exceeded`）しか見ず、
/// `end` の無い古い研究 harness の文・構造化した予算切れの文・`(continuation disabled)` の文・yield の文の
/// 分類を固定していなかった。
#[test]
fn past_worker_finished_outcomes_keep_their_attempt_classification() {
    use crate::model::Status;
    let t = crate::model_policy::tests::task("x", vec![]);
    let cases: [(&str, AttemptOutcome); 9] = [
        // D1 より前の研究 harness・`run_subprocess` の wall clock（文言の `Error`）。
        (
            "error(retryable=true): wall clock exceeded",
            BudgetExhausted,
        ),
        // 構造化した予算切れ（`settle_terminal` の `BudgetExhausted` の枝）。
        (
            "error(retryable=true): budget exhausted (WallClock): wall clock exceeded",
            BudgetExhausted,
        ),
        (
            "error(retryable=true): budget exhausted (Turns): claude result: error_max_turns",
            BudgetExhausted,
        ),
        // D3: continuation を使えない run（研究 harness・`[execution] continuation = false`）。
        (
            "error(retryable=true): budget_exhausted(wall_clock) (continuation disabled)",
            BudgetExhausted,
        ),
        // yield は予算切れの語を持たないので失敗として数える（これまでと同じ）。
        (
            "error(retryable=true): yielded (continuation disabled)",
            WorkerError,
        ),
        // context 超過の語彙（`looks_like_context_exceeded`）。
        (
            "error(retryable=true): acp agent returned an error for prompt: prompt is too long",
            BudgetExhausted,
        ),
        ("error(retryable=true): idle timeout", WorkerError),
        (
            "error(retryable=false): result.json misplaced: expected a; found b, c; reports were not adopted",
            WorkerError,
        ),
        (
            "error(retryable=true): paperqa adapter could not write artifacts/result.json: denied",
            WorkerError,
        ),
    ];
    for (outcome, expected) in cases {
        let events = vec![
            Event::WorkerFinished {
                run_id: "w".into(),
                outcome: outcome.into(),
                usage: None,
                role: None,
                metrics: None,
                end: None,
            },
            Event::Transitioned {
                from: Status::Running,
                to: Status::Ready,
                reason: "worker_error".into(),
            },
        ];
        let got: Vec<AttemptOutcome> = attempt_history(&t, &events)
            .iter()
            .map(|r| r.outcome)
            .collect();
        assert_eq!(got, vec![expected], "{outcome}");
    }
}
