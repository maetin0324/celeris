use super::*;
const WINDOW_5H_SECONDS: f64 = 5.0 * 3600.0;

const EPS: f64 = 1e-9;

fn at(secs: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_800_000_000 + secs).unwrap()
}

fn rfc(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap()
}

fn window(
    id: &str,
    remaining: f64,
    reset: Option<OffsetDateTime>,
    observed: OffsetDateTime,
) -> QuotaWindow {
    QuotaWindow {
        account_id: "acc-1".into(),
        window_id: id.into(),
        unit: "requests".into(),
        remaining: Some(remaining),
        limit: Some(1.0),
        reset_at: reset.map(rfc),
        measured: true,
        observed_at: Some(rfc(observed)),
        window_duration_s: None,
        estimated_consumption: Some(1.0),
        reserve_value_usd: Some(10.0),
    }
}

fn state(windows: Vec<QuotaWindow>, observed: OffsetDateTime) -> SourceState {
    let mut s = SourceState::unobserved("dep-1");
    s.observed_at = Some(rfc(observed));
    s.quota_windows = windows;
    s
}

fn sub<'a>(state: &'a SourceState, now: OffsetDateTime) -> CostInputs<'a> {
    CostInputs {
        billing: Billing::Subscription,
        pricing: None,
        usage: RequestUsage::default(),
        self_host: SelfHostRates::default(),
        state,
        concurrency_limit: None,
        now,
        freshness: FreshnessPolicy::default(),
    }
}

fn close(a: Option<f64>, b: f64) -> bool {
    a.is_some_and(|v| (v - b).abs() < EPS)
}

#[test]
fn routing_effective_cost_distinguishes_cash_shadow_and_resource() {
    let now = at(0);
    let five_h = WINDOW_5H_SECONDS as i64;
    // 残量 0.5 → 0.1 で shadow が 5 倍（reset までは同じ）。
    let s_half = state(
        vec![window(
            "five_hour",
            0.5,
            Some(now + time::Duration::seconds(five_h / 2)),
            now,
        )],
        now,
    );
    let s_tenth = state(
        vec![window(
            "five_hour",
            0.1,
            Some(now + time::Duration::seconds(five_h / 2)),
            now,
        )],
        now,
    );
    let a = estimate_cost(&sub(&s_half, now)).unwrap();
    let b = estimate_cost(&sub(&s_tenth, now)).unwrap();
    let ratio = b.subscription_shadow_usd.unwrap() / a.subscription_shadow_usd.unwrap();
    assert!(
        (ratio - 5.0).abs() < EPS,
        "remaining 0.5->0.1 must multiply shadow by 5, got {ratio}"
    );
    // reset までの時間が半分なら shadow も半分（残量は同じ）。
    let full = state(
        vec![window(
            "five_hour",
            0.5,
            Some(now + time::Duration::seconds(five_h)),
            now,
        )],
        now,
    );
    let c = estimate_cost(&sub(&full, now)).unwrap();
    let half_ratio = a.subscription_shadow_usd.unwrap() / c.subscription_shadow_usd.unwrap();
    assert!(
        (half_ratio - 0.5).abs() < EPS,
        "half time to reset must halve shadow, got {half_ratio}"
    );
    // 同じ入力は同じ出力。
    assert_eq!(estimate_cost(&sub(&s_half, now)).unwrap(), a);

    // 実測 cash は窓・queue の状態に依らず不変。未知価格は 0 にしない。
    let priced = TokenPricing {
        input_usd_per_million: Some(1.0),
        cached_input_usd_per_million: None,
        output_usd_per_million: Some(2.0),
        as_of: None,
        provenance: "test".into(),
    };
    let usage = RequestUsage {
        input_tokens: Some(1_000_000.0),
        output_tokens: Some(500_000.0),
        ..Default::default()
    };
    let api = |s: &SourceState, pricing: Option<&TokenPricing>| {
        estimate_cost(&CostInputs {
            billing: Billing::MeteredApi,
            pricing,
            usage,
            self_host: SelfHostRates::default(),
            state: s,
            concurrency_limit: None,
            now,
            freshness: FreshnessPolicy::default(),
        })
        .unwrap()
    };
    let p1 = api(&s_half, Some(&priced));
    let p2 = api(&s_tenth, Some(&priced));
    assert!(close(p1.cash_usd, 2.0), "cash = 1.0 + 0.5*2.0");
    assert_eq!(
        p1.cash_usd, p2.cash_usd,
        "cash must not depend on quota state"
    );
    assert_eq!(
        api(&s_half, None).cash_usd,
        None,
        "unknown price is None, not 0"
    );
    assert!(
        api(&s_half, None)
            .assumptions
            .contains(&"price_unknown".to_string())
    );

    // self-host: queue 待ちが増えると resource と pressure が増える。
    let rates = SelfHostRates {
        usd_per_gpu_second: Some(0.001),
        usd_per_wait_second: Some(0.0001),
    };
    let mut busy = SourceState::unobserved("gpu-1");
    busy.observed_at = Some(rfc(now));
    busy.queue_limit = Some(4);
    let sh = |depth: u32, wait: f64| {
        let mut s = busy.clone();
        s.queue_depth = Some(depth);
        estimate_cost(&CostInputs {
            billing: Billing::SelfHosted,
            pricing: None,
            usage: RequestUsage {
                gpu_seconds: Some(100.0),
                queue_wait_seconds: Some(wait),
                ..Default::default()
            },
            self_host: rates,
            state: &s,
            concurrency_limit: None,
            now,
            freshness: FreshnessPolicy::default(),
        })
        .unwrap()
    };
    let low = sh(1, 10.0);
    let high = sh(3, 30.0);
    assert!(close(low.self_host_resource_usd, 0.1 + 0.001));
    assert!(high.self_host_resource_usd > low.self_host_resource_usd);
    assert!(close(low.pressure, 0.25) && close(high.pressure, 0.75));
    assert_eq!(low.cash_usd, Some(0.0), "self-host has no API cash");
    // 測定・設定が無ければ resource は unknown。
    let no_rate = estimate_cost(&CostInputs {
        billing: Billing::SelfHosted,
        pricing: None,
        usage: RequestUsage {
            gpu_seconds: Some(1.0),
            queue_wait_seconds: Some(1.0),
            ..Default::default()
        },
        self_host: SelfHostRates::default(),
        state: &busy,
        concurrency_limit: None,
        now,
        freshness: FreshnessPolicy::default(),
    })
    .unwrap();
    assert_eq!(no_rate.self_host_resource_usd, None);
    assert_eq!(no_rate.effective_usd, None);
}

#[test]
fn routing_source_state_stale_quota_is_unknown() {
    let now = at(0);
    let ttl = FreshnessPolicy::default().observation_ttl_seconds as i64;
    // 偽時計: TTL を 1 秒超えた観測。残量 0.5 を満タン扱いせず shadow は unknown。
    let old = state(
        vec![window(
            "five_hour",
            0.5,
            Some(now + time::Duration::hours(1)),
            now - time::Duration::seconds(ttl + 1),
        )],
        now - time::Duration::seconds(ttl + 1),
    );
    let stale = estimate_cost(&sub(&old, now)).unwrap();
    assert_eq!(
        stale.subscription_shadow_usd, None,
        "stale remaining must not become zero shadow"
    );
    assert!(
        exclusion_reasons(&old, now, FreshnessPolicy::default())
            .unwrap()
            .is_empty()
    );

    // reset 境界を越えた窓（観測は新しくても）も満タンではなく unknown。
    let past_reset = state(
        vec![window(
            "five_hour",
            0.5,
            Some(now - time::Duration::seconds(1)),
            now,
        )],
        now,
    );
    assert_eq!(
        estimate_cost(&sub(&past_reset, now))
            .unwrap()
            .subscription_shadow_usd,
        None
    );

    // 古い観測の 0 は枯渇として除外しない（reset 前でも TTL 外なら unknown）。
    let stale_zero = state(
        vec![window(
            "five_hour",
            0.0,
            Some(now + time::Duration::hours(1)),
            now - time::Duration::seconds(ttl + 1),
        )],
        now,
    );
    assert!(
        exclusion_reasons(&stale_zero, now, FreshnessPolicy::default())
            .unwrap()
            .is_empty()
    );

    // 新鮮な枯渇が 1 窓でもあれば除外（他の窓が十分でも）。
    let exhausted = state(
        vec![
            window("five_hour", 0.0, Some(now + time::Duration::hours(1)), now),
            window("seven_day", 0.9, Some(now + time::Duration::days(3)), now),
        ],
        now,
    );
    assert_eq!(
        exclusion_reasons(&exhausted, now, FreshnessPolicy::default()).unwrap(),
        vec!["quota_exhausted"]
    );

    // 観測時刻が無い・未来の観測は既知にしない。
    let mut no_time = state(vec![window("monthly", 0.5, None, now)], now);
    no_time.observed_at = None;
    no_time.quota_windows[0].observed_at = None;
    assert_eq!(
        estimate_cost(&sub(&no_time, now))
            .unwrap()
            .subscription_shadow_usd,
        None
    );
    let future = state(
        vec![window(
            "monthly",
            0.5,
            None,
            now + time::Duration::seconds(5),
        )],
        now,
    );
    assert_eq!(
        estimate_cost(&sub(&future, now))
            .unwrap()
            .subscription_shadow_usd,
        None
    );

    // 数値の検証: NaN・負・分母 0 は拒否。
    let mut nan = window("five_hour", 0.5, Some(now + time::Duration::hours(1)), now);
    nan.estimated_consumption = Some(f64::NAN);
    assert_eq!(
        estimate_cost(&sub(&state(vec![nan], now), now)).unwrap_err(),
        "invalid_estimate"
    );
    let mut zero_duration = window("custom", 0.5, Some(now + time::Duration::hours(1)), now);
    zero_duration.window_duration_s = Some(0.0);
    assert_eq!(
        estimate_cost(&sub(&state(vec![zero_duration], now), now)).unwrap_err(),
        "zero_denominator"
    );
    let mut zero_queue = SourceState::unobserved("gpu-1");
    zero_queue.queue_depth = Some(1);
    zero_queue.queue_limit = Some(0);
    assert_eq!(pressure(&zero_queue, None).unwrap_err(), "zero_denominator");
}
