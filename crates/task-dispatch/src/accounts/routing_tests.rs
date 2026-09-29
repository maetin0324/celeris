use super::*;

/// ADR-0074 D4.1（Phase F3）: `window_remaining` を `sources_view.rs` から移した回帰
/// （元のテスト精神と同じ規律: 新しい・有効な観測だけ、古い/未来/リセット済みは `None`）。
#[test]
fn window_remaining_table() {
    let obs = RateLimitObservation {
        five_hour: Some(RateWindow {
            utilization: 0.3,
            resets_at: 2_000,
        }),
        seven_day: None,
        status: None,
        resets_at: None,
        observed_at: 1_000,
    };
    assert_eq!(
        window_remaining(&obs, 1_100, obs.five_hour),
        Some(0.7),
        "fresh and valid"
    );
    assert_eq!(
        window_remaining(&obs, 1_301, obs.five_hour),
        None,
        "stale (>300s)"
    );
    assert_eq!(
        window_remaining(&obs, 999, obs.five_hour),
        None,
        "future observation"
    );
    assert_eq!(
        window_remaining(&obs, 1_100, obs.seven_day),
        None,
        "no window"
    );
    assert_eq!(
        window_remaining(
            &obs,
            2_000,
            Some(RateWindow {
                utilization: 0.3,
                resets_at: 2_000
            })
        ),
        None,
        "window already reset"
    );
}

#[test]
fn missing_stale_and_expired_quota_are_unknown() {
    let mut obs = RateLimitObservation {
        five_hour: Some(RateWindow {
            utilization: 0.5,
            resets_at: 2000,
        }),
        seven_day: Some(RateWindow {
            utilization: 0.8,
            resets_at: 3000,
        }),
        observed_at: 1000,
        status: None,
        resets_at: None,
    };
    assert!((measured_remaining(&obs, 1100).unwrap() - 0.2).abs() < 1e-6);
    assert_eq!(measured_remaining(&obs, 1301), None);
    assert_eq!(measured_remaining(&obs, 999), None);
    obs.observed_at = 1999;
    assert_eq!(measured_remaining(&obs, 2000), None);
    obs.seven_day = None;
    assert_eq!(measured_remaining(&obs, 1999), None);
}
