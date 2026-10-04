use super::*;

fn snap(utilization: f64, resets_at: i64, observed_at: i64) -> WindowSnapshot {
    WindowSnapshot {
        utilization,
        resets_at,
        observed_at,
    }
}

// ---- weighted_tokens ----

#[test]
fn weighted_tokens_applies_the_formula() {
    let usage = Usage {
        input_tokens: Some(100),
        output_tokens: Some(10),
        cache_read_tokens: Some(1000),
        cache_creation_tokens: Some(40),
        cost_usd: None,
        duplicate_reads: None,
        session_resumed: None,
    };
    // 100 + 0.1*1000 + 1.25*40 + 5*10 = 100 + 100 + 50 + 50 = 300
    let w = weighted_tokens(&usage, 5.0);
    assert!((w - 300.0).abs() < 1e-9, "{w}");
}

#[test]
fn default_r_out_distinguishes_codex_from_claude() {
    assert_eq!(default_r_out("codex-oauth"), DEFAULT_R_OUT_CODEX);
    assert_eq!(default_r_out("gpt-6-sol"), DEFAULT_R_OUT_CODEX);
    assert_eq!(default_r_out("claude-oauth"), DEFAULT_R_OUT_CLAUDE);
    assert_eq!(
        default_r_out("openai-compatible:qwen"),
        DEFAULT_R_OUT_CLAUDE
    );
}

// ---- before_is_valid (h) ----

#[test]
fn before_is_valid_table() {
    // 消費の無い古い観測は使う（300 秒を大きく超えても、他の消費が無ければ有効）。
    assert!(before_is_valid(0, 10_000, false));
    // 他の消費があれば、新しくても使わない。
    assert!(!before_is_valid(9_999, 10_000, true));
    // 観測が未来（壁時計のずれ）なら常に無効。
    assert!(!before_is_valid(10_001, 10_000, false));
    // ちょうど同時刻は有効。
    assert!(before_is_valid(10_000, 10_000, false));
}

// ---- measured (g): measured_when_exclusive_and_fresh ----

#[test]
fn measured_when_exclusive_and_fresh() {
    let input = MeasuredInputs {
        before: Some(snap(0.10, 5_000, 100)),
        after: Some(snap(0.14, 5_000, 900)),
        before_valid: true,
        exclusive: true,
    };
    let pct = measured_used_pct(&input).expect("measured");
    assert!((pct - 4.0).abs() < 1e-9, "{pct}");

    let decided = decide_window(QuotaWindow::FiveHour, &input, None, 1_000.0, None);
    assert_eq!(decided.method, QuotaMethod::Measured);
    assert_eq!(decided.used_pct, Some(pct));
    assert_eq!(decided.resets_at, Some(5_000));
}

#[test]
fn measured_never_goes_negative() {
    // アカウントの残量がリフレッシュ・訂正などで一見「減った」ように見えても 0 未満にしない。
    let input = MeasuredInputs {
        before: Some(snap(0.50, 5_000, 100)),
        after: Some(snap(0.40, 5_000, 900)),
        before_valid: true,
        exclusive: true,
    };
    assert_eq!(measured_used_pct(&input), Some(0.0));
}

#[test]
fn measured_requires_exclusive_and_valid_before() {
    let base = MeasuredInputs {
        before: Some(snap(0.10, 5_000, 100)),
        after: Some(snap(0.20, 5_000, 900)),
        before_valid: true,
        exclusive: true,
    };
    let not_exclusive = MeasuredInputs {
        exclusive: false,
        ..base
    };
    assert_eq!(measured_used_pct(&not_exclusive), None);
    let invalid_before = MeasuredInputs {
        before_valid: false,
        ..base
    };
    assert_eq!(measured_used_pct(&invalid_before), None);
}

#[test]
fn measured_rejects_a_window_reset_between_before_and_after() {
    let input = MeasuredInputs {
        before: Some(snap(0.90, 5_000, 100)),
        after: Some(snap(0.05, 9_000, 5_100)), // 枠がリセットされ、別の resets_at になった
        before_valid: true,
        exclusive: true,
    };
    assert_eq!(measured_used_pct(&input), None);
}

// ---- apportioned (g): apportioned_by_weighted_tokens_when_runs_overlap ----

#[test]
fn apportioned_by_weighted_tokens_when_runs_overlap() {
    let group_before = snap(0.10, 5_000, 100);
    let group_after = snap(0.40, 5_000, 900); // delta = 30 pt
    let a = ApportionedInputs {
        group_before: Some(group_before),
        group_after: Some(group_after),
        group_before_valid: true,
        member_weighted_tokens: 300.0,
        group_weighted_tokens_total: 1_000.0,
    };
    let b = ApportionedInputs {
        member_weighted_tokens: 700.0,
        ..a
    };
    let pct_a = apportioned_used_pct(&a).expect("a");
    let pct_b = apportioned_used_pct(&b).expect("b");
    assert!((pct_a - 9.0).abs() < 1e-9, "{pct_a}"); // 30 * 300/1000
    assert!((pct_b - 21.0).abs() < 1e-9, "{pct_b}"); // 30 * 700/1000
    assert!(
        (pct_a + pct_b - 30.0).abs() < 1e-9,
        "apportioned shares must sum to the delta"
    );

    let decided = decide_window(
        QuotaWindow::FiveHour,
        &not_exclusive_measured(),
        Some(&a),
        300.0,
        None,
    );
    assert_eq!(decided.method, QuotaMethod::Apportioned);
    assert_eq!(decided.used_pct, Some(pct_a));
}

fn not_exclusive_measured() -> MeasuredInputs {
    MeasuredInputs {
        before: None,
        after: None,
        before_valid: true,
        exclusive: false,
    }
}

#[test]
fn apportioned_is_none_while_the_group_has_not_closed() {
    let a = ApportionedInputs {
        group_before: Some(snap(0.10, 5_000, 100)),
        group_after: None, // まだ閉じていない
        group_before_valid: true,
        member_weighted_tokens: 300.0,
        group_weighted_tokens_total: 1_000.0,
    };
    assert_eq!(apportioned_used_pct(&a), None);
}

#[test]
fn apportioned_requires_a_valid_group_before_and_positive_weights() {
    let base = ApportionedInputs {
        group_before: Some(snap(0.10, 5_000, 100)),
        group_after: Some(snap(0.20, 5_000, 900)),
        group_before_valid: true,
        member_weighted_tokens: 300.0,
        group_weighted_tokens_total: 1_000.0,
    };
    assert!(apportioned_used_pct(&base).is_some());
    assert_eq!(
        apportioned_used_pct(&ApportionedInputs {
            group_before_valid: false,
            ..base
        }),
        None
    );
    assert_eq!(
        apportioned_used_pct(&ApportionedInputs {
            group_weighted_tokens_total: 0.0,
            ..base
        }),
        None
    );
}

// ---- estimated (g): estimated_uses_ratio_of_sums_calibration ----

#[test]
fn estimated_uses_ratio_of_sums_calibration() {
    // 3 件の measured (q, W): (4, 1000), (8, 2000), (3, 1500) => k = 15/4500 = 1/300
    let measured = [(4.0, 1_000.0), (8.0, 2_000.0), (3.0, 1_500.0)];
    let cal = calibrate(&measured).expect("calibration");
    assert_eq!(cal.samples, 3);
    assert!((cal.k - (15.0 / 4_500.0)).abs() < 1e-9, "{}", cal.k);

    let pct = estimated_used_pct(900.0, Some(cal)).expect("estimated");
    assert!((pct - (900.0 * 15.0 / 4_500.0)).abs() < 1e-9, "{pct}");

    let decided = decide_window(
        QuotaWindow::SevenDay,
        &not_exclusive_measured(),
        None,
        900.0,
        Some(cal),
    );
    assert_eq!(decided.method, QuotaMethod::Estimated);
    assert_eq!(decided.used_pct, Some(pct));
}

#[test]
fn estimated_requires_at_least_three_measured_samples() {
    let two = calibrate(&[(4.0, 1_000.0), (8.0, 2_000.0)]).expect("calibration");
    assert_eq!(two.samples, 2);
    assert_eq!(
        estimated_used_pct(500.0, Some(two)),
        None,
        "fewer than 3 samples: unknown, not estimated"
    );
}

#[test]
fn calibrate_of_empty_or_zero_weight_is_none() {
    assert_eq!(calibrate(&[]), None);
    assert_eq!(calibrate(&[(1.0, 0.0), (2.0, 0.0)]), None);
}

// ---- unknown (g): unknown_is_never_zero ----

#[test]
fn unknown_is_never_zero() {
    let decided = decide_window(
        QuotaWindow::FiveHour,
        &not_exclusive_measured(),
        None,
        500.0,
        None,
    );
    assert_eq!(decided.method, QuotaMethod::Unknown);
    assert_eq!(decided.used_pct, None, "unknown must be None, never 0.0");
}

#[test]
fn unknown_when_apportioned_group_never_closes() {
    let a = ApportionedInputs {
        group_before: Some(snap(0.10, 5_000, 100)),
        group_after: None,
        group_before_valid: true,
        member_weighted_tokens: 300.0,
        group_weighted_tokens_total: 1_000.0,
    };
    let decided = decide_window(
        QuotaWindow::FiveHour,
        &not_exclusive_measured(),
        Some(&a),
        300.0,
        None,
    );
    assert_eq!(decided.method, QuotaMethod::Unknown);
    assert_eq!(decided.used_pct, None);
}

// ---- free ----

#[test]
fn free_window_is_zero_not_unknown() {
    let w = free_window(QuotaWindow::FiveHour);
    assert_eq!(w.method, QuotaMethod::Free);
    assert_eq!(w.used_pct, Some(0.0));
}

// ---- priority order across all methods (table test, D4.2) ----

#[test]
fn priority_order_prefers_measured_over_apportioned_over_estimated_over_unknown() {
    let measured = MeasuredInputs {
        before: Some(snap(0.10, 5_000, 100)),
        after: Some(snap(0.20, 5_000, 900)),
        before_valid: true,
        exclusive: true,
    };
    let apportioned = ApportionedInputs {
        group_before: Some(snap(0.10, 5_000, 100)),
        group_after: Some(snap(0.50, 5_000, 900)),
        group_before_valid: true,
        member_weighted_tokens: 300.0,
        group_weighted_tokens_total: 1_000.0,
    };
    let calibration = calibrate(&[(4.0, 1_000.0), (8.0, 2_000.0), (3.0, 1_500.0)]);

    // measured が使えるなら、apportioned/estimated の入力があっても measured が勝つ。
    let d = decide_window(
        QuotaWindow::FiveHour,
        &measured,
        Some(&apportioned),
        300.0,
        calibration,
    );
    assert_eq!(d.method, QuotaMethod::Measured);

    // measured が使えず apportioned が閉じていれば apportioned。
    let d = decide_window(
        QuotaWindow::FiveHour,
        &not_exclusive_measured(),
        Some(&apportioned),
        300.0,
        calibration,
    );
    assert_eq!(d.method, QuotaMethod::Apportioned);

    // apportioned の入力が無ければ estimated。
    let d = decide_window(
        QuotaWindow::FiveHour,
        &not_exclusive_measured(),
        None,
        300.0,
        calibration,
    );
    assert_eq!(d.method, QuotaMethod::Estimated);

    // 較正も無ければ unknown。
    let d = decide_window(
        QuotaWindow::FiveHour,
        &not_exclusive_measured(),
        None,
        300.0,
        None,
    );
    assert_eq!(d.method, QuotaMethod::Unknown);
}

// ---- representative_method ----

#[test]
fn representative_method_prefers_the_most_confident_window() {
    let windows = vec![
        free_window(QuotaWindow::FiveHour),
        QuotaWindowUse {
            window: QuotaWindow::SevenDay,
            before: None,
            after: None,
            resets_at: None,
            used_pct: None,
            method: QuotaMethod::Unknown,
        },
    ];
    assert_eq!(representative_method(&windows), QuotaMethod::Free);
    assert_eq!(representative_method(&[]), QuotaMethod::Unknown);
}

// ---- aggregate_quota_use ----

#[test]
fn aggregate_quota_use_sums_by_source_account_and_window() {
    let w1 = vec![QuotaWindowUse {
        window: QuotaWindow::FiveHour,
        before: Some(0.1),
        after: Some(0.14),
        resets_at: Some(5_000),
        used_pct: Some(4.0),
        method: QuotaMethod::Measured,
    }];
    let w2 = vec![QuotaWindowUse {
        window: QuotaWindow::FiveHour,
        before: None,
        after: None,
        resets_at: None,
        used_pct: None,
        method: QuotaMethod::Unknown,
    }];
    let records = vec![
        QuotaRunRecord {
            source: "claude-oauth",
            account: Some("a"),
            windows: &w1,
            role: RunRole::Worker,
        },
        QuotaRunRecord {
            source: "claude-oauth",
            account: Some("a"),
            windows: &w2,
            role: RunRole::Reviewer,
        },
    ];
    let agg = aggregate_quota_use(records);
    assert_eq!(agg.len(), 1, "{agg:?}");
    let row = &agg[0];
    assert_eq!(row.source, "claude-oauth");
    assert_eq!(row.account.as_deref(), Some("a"));
    assert_eq!(row.window, QuotaWindow::FiveHour);
    assert_eq!(row.runs, 2);
    assert_eq!(
        row.used_pct,
        Some(4.0),
        "unknown run contributes 0 to the sum"
    );
    assert_eq!(row.method_counts.get("measured"), Some(&1));
    assert_eq!(row.method_counts.get("unknown"), Some(&1));
    assert_eq!(row.runs_by_role.get("worker"), Some(&1));
    assert_eq!(row.runs_by_role.get("reviewer"), Some(&1));
}

// ---- merge_quota_use ----

#[test]
fn merge_quota_use_sums_rows_from_different_tasks() {
    let a = QuotaUse {
        source: "claude-oauth".to_string(),
        account: Some("x".to_string()),
        window: QuotaWindow::FiveHour,
        used_pct: Some(3.0),
        runs: 1,
        method_counts: BTreeMap::from([("measured".to_string(), 1)]),
        runs_by_role: BTreeMap::from([("worker".to_string(), 1)]),
    };
    let b = QuotaUse {
        used_pct: Some(2.0),
        runs: 1,
        method_counts: BTreeMap::from([("measured".to_string(), 1)]),
        runs_by_role: BTreeMap::from([("planner".to_string(), 1)]),
        ..a.clone()
    };
    let c_unknown = QuotaUse {
        used_pct: None,
        runs: 1,
        method_counts: BTreeMap::from([("unknown".to_string(), 1)]),
        ..a.clone()
    };
    let other_source = QuotaUse {
        source: "codex-oauth".to_string(),
        ..a.clone()
    };
    let merged = merge_quota_use(vec![a, b, c_unknown, other_source]);
    assert_eq!(merged.len(), 2, "{merged:?}");
    let claude = merged
        .iter()
        .find(|r| r.source == "claude-oauth")
        .expect("claude row");
    assert_eq!(claude.runs, 3);
    assert_eq!(
        claude.used_pct,
        Some(5.0),
        "unknown row contributes nothing"
    );
    assert_eq!(claude.method_counts.get("measured"), Some(&2));
    assert_eq!(claude.method_counts.get("unknown"), Some(&1));
    assert_eq!(claude.runs_by_role.get("worker"), Some(&2));
    assert_eq!(claude.runs_by_role.get("planner"), Some(&1));
    let codex = merged
        .iter()
        .find(|r| r.source == "codex-oauth")
        .expect("codex row");
    assert_eq!(codex.runs, 1);
}

#[test]
fn quota_use_without_runs_by_role_reads_as_empty() {
    let legacy = r#"{"source":"claude-oauth","account":"x","window":"five_hour","used_pct":1.5,"runs":2,"method_counts":{"measured":2}}"#;
    let row: QuotaUse = serde_json::from_str(legacy).expect("legacy QuotaUse JSON");
    assert_eq!(row.runs, 2);
    assert!(row.runs_by_role.is_empty());
}

#[test]
fn merge_quota_use_of_empty_is_empty() {
    assert!(merge_quota_use(std::iter::empty()).is_empty());
}
