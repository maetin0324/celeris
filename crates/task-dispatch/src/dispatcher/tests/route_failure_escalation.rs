//! ADR 2026-10-15-mlsys-min-routing-policy D6/D7: 失敗経路の分類・限定エスカレーション・利用可否の配線。
//! 時刻は注入時計（`set_now_unix_fn`）。待機はしない。

use super::*;
use crate::dispatcher::route_failure::{FailureAction, FailureOrigin, classify_failure};
use crate::dispatcher::route_launch::{LaunchAdmission, admit_by_availability};
use task_core::route_select::{
    Availability, AvailabilityInput, EscalationAttempts, EscalationLimits, FailureClass,
    LaneLimits, Remaining, ReplanReason, UnavailableReason, availability,
};

fn test_dispatcher(now: i64) -> Dispatcher {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter: Arc<dyn WorkerAdapter> = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "x".into(),
            usage: None,
            evidence: vec![],
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store, adapter, 1);
    d.set_now_unix_fn(Arc::new(move || now));
    d
}

fn attempts(lane: Tier, esc: u32, infra: u32) -> EscalationAttempts {
    EscalationAttempts {
        lane,
        escalations_done: esc,
        infra_retries: infra,
    }
}

#[test]
fn infra_failures_wait_in_same_lane_with_backoff_and_never_escalate() {
    let d = test_dispatcher(1_000);
    let limits = EscalationLimits::default();
    let throttled = AdapterError::Throttled {
        retry_after: Duration::from_secs(1),
    };
    let io = AdapterError::Io(std::io::Error::other("boom"));
    let timeout = FailureOrigin::WorkerError {
        message: "request timed out (503)",
        retryable: false,
    };
    for origin in [
        FailureOrigin::Adapter(&throttled),
        FailureOrigin::Adapter(&io),
        timeout,
    ] {
        for (n, delay) in [(0, 30), (1, 120), (2, 300)] {
            let a = d.decide_route_failure(origin, &attempts(Tier::Cheap, 0, n), &limits, false);
            assert_eq!(
                a,
                FailureAction::WaitRetry {
                    lane: Tier::Cheap,
                    not_before: 1_000 + delay
                }
            );
        }
        // 上限超過は昇格ではなく replan。
        let a = d.decide_route_failure(origin, &attempts(Tier::Cheap, 0, 3), &limits, false);
        assert_eq!(
            a,
            FailureAction::Replan(ReplanReason::InfraRetriesExhausted)
        );
    }
}

#[test]
fn content_failure_escalates_one_step_then_requests_replan() {
    let d = test_dispatcher(0);
    let limits = EscalationLimits::default();
    let o = FailureOrigin::ContentRejected;
    assert_eq!(
        d.decide_route_failure(o, &attempts(Tier::Cheap, 0, 0), &limits, false),
        FailureAction::Escalate { to: Tier::Standard }
    );
    assert_eq!(
        d.decide_route_failure(o, &attempts(Tier::Standard, 1, 0), &limits, false),
        FailureAction::Replan(ReplanReason::EscalationLimit)
    );
}

#[test]
fn explicit_model_and_allowed_tiers_are_never_overridden() {
    let d = test_dispatcher(0);
    let o = FailureOrigin::ContentRejected;
    // 明示 model は昇格せず失敗を報告する。
    assert_eq!(
        d.decide_route_failure(
            o,
            &attempts(Tier::Cheap, 0, 0),
            &EscalationLimits::default(),
            true
        ),
        FailureAction::ReportFailure
    );
    // 一段上が許可外なら、さらに上へ飛ばさず replan。
    let limits = EscalationLimits {
        lane_limits: LaneLimits {
            allowed: vec![Tier::Cheap, Tier::Frontier],
            max_lane: None,
        },
        ..Default::default()
    };
    assert_eq!(
        d.decide_route_failure(o, &attempts(Tier::Cheap, 0, 0), &limits, false),
        FailureAction::Replan(ReplanReason::NotPermitted)
    );
    // 天井を超える昇格もしない。
    let limits = EscalationLimits {
        max_escalations: 2,
        lane_limits: LaneLimits {
            allowed: vec![],
            max_lane: Some(Tier::Standard),
        },
        ..Default::default()
    };
    assert_eq!(
        d.decide_route_failure(o, &attempts(Tier::Standard, 0, 0), &limits, false),
        FailureAction::Replan(ReplanReason::NotPermitted)
    );
}

#[test]
fn classification_defaults_toward_waiting_not_spending() {
    let auth = AdapterError::AuthFailed("x".into());
    assert_eq!(
        classify_failure(FailureOrigin::Adapter(&auth)),
        FailureClass::TransientInfra
    );
    assert_eq!(
        classify_failure(FailureOrigin::WorkerError {
            message: "tests failed",
            retryable: false
        }),
        FailureClass::Content
    );
}

fn input(quota: Remaining, cost: Remaining, budget: Remaining) -> AvailabilityInput {
    AvailabilityInput {
        compatible: true,
        cooling_down: false,
        in_use: 0,
        max_concurrent: None,
        quota_remaining: quota,
        estimated_cost: cost,
        budget_remaining: budget,
    }
}

#[test]
fn unknown_quota_or_cost_is_not_treated_as_zero_or_launched() {
    let known = Remaining::Known(10.0);
    let unk = Remaining::Unknown;
    assert_eq!(
        availability(&input(unk, known, known)),
        Availability::Unknown
    );
    assert_eq!(
        availability(&input(known, unk, known)),
        Availability::Unknown
    );
    assert_eq!(
        availability(&input(known, known, unk)),
        Availability::Unknown
    );
    assert_eq!(
        availability(&input(known, known, known)),
        Availability::Available
    );
    // Unknown だけなら起動せず Hold。確認できた Available が後ろにあればそちらを選ぶ。
    assert_eq!(
        admit_by_availability(vec![("a", Availability::Unknown)]),
        LaunchAdmission::Hold { unconfirmed: 1 }
    );
    assert_eq!(
        admit_by_availability(vec![
            ("a", Availability::Unknown),
            (
                "b",
                Availability::Unavailable(UnavailableReason::QuotaExhausted)
            ),
            ("c", Availability::Available),
        ]),
        LaunchAdmission::Launch("c")
    );
    assert_eq!(
        admit_by_availability::<&str>(vec![(
            "b",
            Availability::Unavailable(UnavailableReason::CoolingDown)
        )]),
        LaunchAdmission::Hold { unconfirmed: 0 }
    );
}
