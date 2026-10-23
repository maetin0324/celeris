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
