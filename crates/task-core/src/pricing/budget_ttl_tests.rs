//! ADR 2026-11-06-phase5-n0-usd-accounting-contract D3 の試験（`pricing.rs` の inline 試験の行数の上限のため別 file）。

use super::*;

fn usage_cache_write(tokens: u64) -> Usage {
    Usage {
        input_tokens: Some(0),
        output_tokens: Some(0),
        cache_read_tokens: Some(0),
        cache_creation_tokens: Some(tokens),
        ..Usage::default()
    }
}

/// N0 ADR D3: Claude Code は cache を 1 時間 TTL（input の 2 倍）で書くが、表の cache write は 5 分 TTL（1.25 倍）。
/// 予算判定は大きい方で数え、5 分 TTL の単価（小さい方）では数えない。表示・campaign 用の `estimate_cost_usd` は変えない。
#[test]
fn budget_cost_counts_cache_creation_at_one_hour_write_price() {
    let rates = BudgetRates::default();
    let u = usage_cache_write(1_000_000);
    for (model, budget_micros, estimate_usd) in [
        ("claude-opus-5-5", 8_000_000, 5.0),
        ("claude-fable-5-1", 20_000_000, 12.5),
        ("claude-sonnet-5-5", 4_000_000, 2.5),
        ("claude-haiku-4-5", 2_000_000, 1.25),
    ] {
        assert_eq!(
            run_budget_cost(Some(model), Some(&u), &rates),
            RunBudgetCost::Known(budget_micros),
            "{model}"
        );
        let est = estimate_cost_usd(model, &u).unwrap_or_else(|| panic!("{model}"));
        assert!((est - estimate_usd).abs() < 1e-9, "{model}: {est}");
    }
    // cache write の単価が無い行（gpt-5）は従来どおり費用不明（2 倍の input で埋めない）。
    assert_eq!(
        run_budget_cost(Some("gpt-5"), Some(&u), &rates),
        RunBudgetCost::Unknown
    );
    // budget_rates の行は人が書いた cache write の単価のまま（input 1.0 の 2 倍にしない）。
    let local = BudgetRates::new([(
        "qwen".to_string(),
        BudgetRate {
            input_per_million: 1.0,
            output_per_million: 2.0,
            cache_read_per_million: 0.5,
            cache_write_per_million: 0.5,
        },
    )])
    .expect("valid");
    assert_eq!(
        run_budget_cost(Some("qwen3"), Some(&u), &local),
        RunBudgetCost::Known(500_000)
    );
}
