use super::*;

/// `QuotaCalibrationBook::calibration` 自体は最小件数を強制しない（`task_core::quota::calibrate`
/// と同じ契約）。3 件未満でも較正値を返すが、`estimated_used_pct` 側が
/// `MIN_CALIBRATION_SAMPLES` 未満を拒む（`quota.rs` の `estimated_requires_at_least_three_measured_samples`
/// で確認済み）。ここでは `samples` が正しく積み上がることだけを確かめる。
#[test]
fn calibration_accumulates_samples_and_computes_the_ratio_of_sums() {
    let mut book = QuotaCalibrationBook::new();
    book.record(
        "claude-oauth",
        task_core::QuotaWindow::FiveHour,
        4.0,
        1_000.0,
    );
    book.record(
        "claude-oauth",
        task_core::QuotaWindow::FiveHour,
        8.0,
        2_000.0,
    );
    let two = book
        .calibration("claude-oauth", task_core::QuotaWindow::FiveHour)
        .expect("2 samples still produce a ratio");
    assert_eq!(two.samples, 2);
    assert!(
        task_core::quota::estimated_used_pct(500.0, Some(two)).is_none(),
        "but estimated_used_pct rejects fewer than 3 samples"
    );

    book.record(
        "claude-oauth",
        task_core::QuotaWindow::FiveHour,
        3.0,
        1_500.0,
    );
    let cal = book
        .calibration("claude-oauth", task_core::QuotaWindow::FiveHour)
        .expect("3 samples");
    assert_eq!(cal.samples, 3);
    assert!((cal.k - (15.0 / 4_500.0)).abs() < 1e-9, "{}", cal.k);
    assert!(task_core::quota::estimated_used_pct(500.0, Some(cal)).is_some());
}

#[test]
fn calibration_is_scoped_to_source_and_window() {
    let mut book = QuotaCalibrationBook::new();
    book.record(
        "claude-oauth",
        task_core::QuotaWindow::FiveHour,
        4.0,
        1_000.0,
    );
    book.record(
        "claude-oauth",
        task_core::QuotaWindow::SevenDay,
        1.0,
        1_000.0,
    );
    book.record(
        "codex-oauth",
        task_core::QuotaWindow::FiveHour,
        9.0,
        1_000.0,
    );
    // seven_day/codex-oauth への record はここでは効かない: five_hour/claude-oauth はまだ 1 件。
    let one = book
        .calibration("claude-oauth", task_core::QuotaWindow::FiveHour)
        .expect("1 sample still returns a ratio");
    assert_eq!(one.samples, 1);
    assert_eq!(
        book.calibration("codex-oauth", task_core::QuotaWindow::SevenDay),
        None,
        "no samples recorded for this (source, window)"
    );
}

#[test]
fn ring_buffer_caps_at_the_maximum_sample_count() {
    let mut book = QuotaCalibrationBook::new();
    for i in 0..(task_core::quota::CALIBRATION_MAX_SAMPLES + 5) {
        book.record(
            "claude-oauth",
            task_core::QuotaWindow::FiveHour,
            i as f64,
            100.0,
        );
    }
    let samples = book.by_key.get("claude-oauth:five_hour").expect("key");
    assert_eq!(samples.len(), task_core::quota::CALIBRATION_MAX_SAMPLES);
    // 最も古い 5 件が落ちている（先頭は 5 のはず）。
    assert_eq!(samples.front().copied().map(|(q, _)| q), Some(5.0));
}

#[test]
fn zero_weighted_tokens_are_not_recorded() {
    let mut book = QuotaCalibrationBook::new();
    book.record("claude-oauth", task_core::QuotaWindow::FiveHour, 4.0, 0.0);
    assert!(
        book.by_key
            .get("claude-oauth:five_hour")
            .is_none_or(|q| q.is_empty())
    );
}

/// ADR-0089（Phase R6-5）: CoS run のアカウント選び。
fn cand(id: &str, in_use: usize) -> AccountCandidate<'_> {
    AccountCandidate {
        id,
        logged_in: true,
        in_use,
    }
}

#[test]
fn least_loaded_picks_the_account_with_fewest_runs_even_at_max_plus_one() {
    let mut book = AccountBook::new_in_memory();
    // `a` は残量が多い（通常の選び方なら `a`）が、走っている run は多い。
    book.record_observation(
        "b",
        RateLimitObservation {
            one_month: None,
            five_hour: Some(RateWindow {
                utilization: 0.9,
                resets_at: 9_000,
            }),
            seven_day: None,
            status: None,
            resets_at: None,
            observed_at: 1_000,
        },
        ObservationSource::Run,
    );
    let cands = vec![cand("a", 2), cand("b", 1)];
    assert_eq!(
        select_account_least_loaded(&cands, &book, 3, 1_000),
        Some("b".to_string())
    );
    // 両方 max (= 2) 本走っていても、上限 +1 (= 3) なら選べる。同数は id 昇順（スコア同点）。
    let full = vec![cand("b", 2), cand("a", 2)];
    let empty = AccountBook::new_in_memory();
    assert_eq!(
        select_account_least_loaded(&full, &empty, 3, 1_000),
        Some("a".to_string())
    );
    // +1 を使い切れば選べない。
    let over = vec![cand("a", 3), cand("b", 3)];
    assert_eq!(select_account_least_loaded(&over, &empty, 3, 1_000), None);
}
