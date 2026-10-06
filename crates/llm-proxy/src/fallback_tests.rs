use super::*;
use time::macros::datetime;

const T0: OffsetDateTime = datetime!(2026-10-05 00:00:00 UTC);

fn secs(n: i64) -> OffsetDateTime {
    T0 + TimeDuration::seconds(n)
}

#[test]
fn classifies_401_429_5xx_network_and_client_errors() {
    assert_eq!(
        FailureClass::of(&SourceError::Unauthorized),
        FailureClass::Unauthorized
    );
    assert_eq!(
        FailureClass::of(&SourceError::RateLimited {
            retry_after: Some(3)
        }),
        FailureClass::RateLimited
    );
    let upstream = |status| SourceError::Upstream {
        status,
        summary: String::new(),
    };
    assert_eq!(FailureClass::of(&upstream(503)), FailureClass::Server);
    assert_eq!(FailureClass::of(&upstream(500)), FailureClass::Server);
    assert_eq!(FailureClass::of(&upstream(400)), FailureClass::Client);
    assert_eq!(
        FailureClass::of(&SourceError::Network("x".into())),
        FailureClass::Network
    );
    assert!(FailureClass::Server.trips_breaker());
    assert!(FailureClass::Network.trips_breaker());
    assert!(!FailureClass::Unauthorized.trips_breaker());
    assert!(!FailureClass::RateLimited.trips_breaker());
}

#[test]
fn budget_enforces_class_limits_total_limit_and_deadline() {
    let settings = FallbackSettings {
        limits: RetryLimits {
            server: 1,
            total_attempts: 3,
            ..RetryLimits::default()
        },
        deadline_secs: 10,
        ..FallbackSettings::default()
    };
    // 分類別の上限: 5xx は 1 回だけ倒す。
    let mut b = FallbackBudget::new(&settings, T0);
    assert!(b.begin_attempt());
    assert_eq!(
        b.after_failure(FailureClass::Server, T0),
        AfterFailure::Fallback
    );
    assert!(b.begin_attempt());
    assert_eq!(
        b.after_failure(FailureClass::Server, T0),
        AfterFailure::Stop(StopReason::ClassLimit(FailureClass::Server))
    );

    // 総上限: 3 回送ったら分類の残りがあっても止まり、4 回目は送らない。
    let mut b = FallbackBudget::new(&settings, T0);
    for _ in 0..2 {
        assert!(b.begin_attempt());
        assert_eq!(
            b.after_failure(FailureClass::RateLimited, T0),
            AfterFailure::Fallback
        );
    }
    assert!(b.begin_attempt());
    assert_eq!(
        b.after_failure(FailureClass::Unauthorized, T0),
        AfterFailure::Stop(StopReason::TotalLimit)
    );
    assert!(!b.begin_attempt());
    assert_eq!(b.attempts(), 3);

    // deadline: 注入した時刻で境界ちょうどから止まる。
    let mut b = FallbackBudget::new(&settings, T0);
    assert!(b.begin_attempt());
    assert_eq!(
        b.after_failure(FailureClass::Network, secs(9)),
        AfterFailure::Fallback
    );
    assert!(b.begin_attempt());
    assert_eq!(
        b.after_failure(FailureClass::Network, secs(10)),
        AfterFailure::Stop(StopReason::Deadline)
    );

    // 4xx（401/429 以外）は既定で倒さない。
    let mut b = FallbackBudget::new(&FallbackSettings::default(), T0);
    assert!(b.begin_attempt());
    assert_eq!(
        b.after_failure(FailureClass::Client, T0),
        AfterFailure::Stop(StopReason::ClassLimit(FailureClass::Client))
    );
}

#[test]
fn breaker_opens_after_threshold_then_half_opens_with_one_probe() {
    let breakers = Breakers::new(BreakerSettings {
        failure_threshold: 2,
        open_secs: 30,
    });
    let d = "openai-compatible:r1";

    // closed: 1 回目の失敗では開かない。401/429（neutral）は数えない。
    breakers.admit(d, T0).expect("closed").failure(T0);
    breakers.admit(d, T0).expect("closed").neutral(T0);
    assert_eq!(
        breakers.state(d),
        BreakerState::Closed {
            consecutive_failures: 1
        }
    );
    // 2 回目で open。deadline までは送らない。
    breakers.admit(d, T0).expect("closed").failure(secs(1));
    assert_eq!(breakers.state(d), BreakerState::Open { until: secs(31) });
    assert!(breakers.admit(d, secs(30)).is_none());
    assert_eq!(
        breakers.earliest_reopen(&[d.to_string()], secs(30)),
        Some(secs(31))
    );

    // deadline ちょうどで half_open。試し打ちは同時 1 件だけ。
    let probe = breakers.admit(d, secs(31)).expect("probe");
    assert_eq!(probe.admission(), Admission::Probe);
    assert!(breakers.admit(d, secs(31)).is_none());
    assert!(breakers.admit(d, secs(99)).is_none());

    // 試し打ちの失敗で再び open（新しい deadline）。
    probe.failure(secs(40));
    assert_eq!(breakers.state(d), BreakerState::Open { until: secs(70) });

    // 試し打ちが結果を返さずに落ちたら（drop）枠だけ返し、次の 1 件が試し打ちになる。
    let probe = breakers.admit(d, secs(70)).expect("probe");
    drop(probe);
    assert_eq!(
        breakers.state(d),
        BreakerState::HalfOpen {
            probe_in_flight: false
        }
    );
    let probe = breakers.admit(d, secs(70)).expect("probe again");
    assert_eq!(probe.admission(), Admission::Probe);
    assert!(breakers.admit(d, secs(70)).is_none());

    // 試し打ちの成功で closed。
    probe.success(secs(71));
    assert_eq!(
        breakers.state(d),
        BreakerState::Closed {
            consecutive_failures: 0
        }
    );
    assert_eq!(
        breakers.admit(d, secs(71)).expect("closed").admission(),
        Admission::Normal
    );
}

#[test]
fn breaker_threshold_zero_never_opens() {
    let breakers = Breakers::new(BreakerSettings {
        failure_threshold: 0,
        open_secs: 30,
    });
    for _ in 0..5 {
        breakers
            .admit("d", T0)
            .expect("always admitted")
            .failure(T0);
    }
    assert!(breakers.admit("d", T0).is_some());
}

#[test]
fn constraints_follow_the_requested_scope() {
    let only_qwen = RequestConstraints::from_request(&ModelRequest::Tiered {
        scope: SourceScope::Only(SourceKind::Qwen),
        tier: task_core::Tier::Cheap,
    });
    assert!(only_qwen.admits(SourceKind::Qwen));
    assert!(!only_qwen.admits(SourceKind::Claude));
    let explicit = RequestConstraints::from_request(&ModelRequest::Explicit {
        source: SourceKind::Claude,
        model: "m".into(),
    });
    assert!(explicit.admits(SourceKind::Claude));
    assert!(!explicit.admits(SourceKind::Gpt));
    let any = RequestConstraints::from_request(&ModelRequest::Tiered {
        scope: SourceScope::Any,
        tier: task_core::Tier::Cheap,
    });
    assert!(any.admits(SourceKind::Gpt));
}
