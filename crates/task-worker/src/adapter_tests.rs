//! 実行共通処理（ADR 2026-11-10-adapter-result-contract D2・(4) 4b）の試験のうち `adapter.rs` に置いた
//! `deadline_terminal`・`with_estimated_cost` の分（`prepare_run`・`finish_run` の試験は `subprocess/tests.rs`）。
//! harness adapter 5 本はこれらを 1 か所で呼ぶので、締め切りと cost の推定の規則はここで正本を直接試す。
//! adapter ごとの試験は結線だけを見る（fake の harness で時間切れを起こすには実時間が要り、規則の境界
//! 〈両方の超過・実測の cost・model の不明〉を 1 件ずつ固定できない）。

use super::*;

const LONG: Duration = Duration::from_secs(1_000);

fn ago(secs: u64) -> Instant {
    Instant::now()
        .checked_sub(Duration::from_secs(secs))
        .expect("the monotonic clock is past the test offset")
}

fn usage(cost_usd: Option<f64>) -> Option<Usage> {
    Some(Usage {
        input_tokens: Some(1_000),
        output_tokens: Some(2_000),
        cost_usd,
        ..Default::default()
    })
}

/// 不変条件: wall clock の超過は `BudgetExhausted{WallClock}`、文言 `wall clock exceeded`（dispatcher の
/// `classify_budget_kind_from_text`・`is_budget_outcome` が見る語）で、idle も超過していても wall clock が勝つ。
/// adapter の試験は実時間の打ち切りを 1 経路ずつしか見ず、両方の超過の優先順位を固定していなかった。
#[test]
fn run_common_deadline_wall_clock_is_budget_exhausted() {
    let expected = Terminal::BudgetExhausted {
        kind: task_core::BudgetKind::WallClock,
        message: "wall clock exceeded".into(),
        usage: None,
    };
    let wall_only = deadline_terminal(ago(10), Instant::now(), Duration::from_secs(5), LONG);
    assert_eq!(wall_only, Err(Box::new(expected.clone())));
    let both = deadline_terminal(
        ago(10),
        ago(10),
        Duration::from_secs(5),
        Duration::from_secs(5),
    );
    assert_eq!(both, Err(Box::new(expected)));
}

/// 不変条件: idle の超過は harness_error に分類変更しない retryable な `Error`（文言 `idle timeout`、
/// ADR-0072 §7 U7）。どちらも超過していなければ、次に待つ時間は wall clock と idle の残りの小さい方。
#[test]
fn run_common_deadline_idle_is_retryable_error() {
    let idle = deadline_terminal(ago(1), ago(10), LONG, Duration::from_secs(5));
    assert_eq!(
        idle,
        Err(Box::new(Terminal::Error {
            message: "idle timeout".into(),
            retryable: true,
            usage: None,
        }))
    );
    // idle の残り（8 秒以下）が wall clock の残り（999 秒以下）より小さい。
    let wait = deadline_terminal(ago(1), ago(2), LONG, Duration::from_secs(10))
        .expect("neither limit is exceeded");
    assert!(wait <= Duration::from_secs(8), "{wait:?}");
    assert!(wait > Duration::ZERO);
    // wall clock の残りの方が小さいときはそちら。
    let wait = deadline_terminal(ago(98), Instant::now(), Duration::from_secs(100), LONG)
        .expect("neither limit is exceeded");
    assert!(wait <= Duration::from_secs(2), "{wait:?}");
}

/// 不変条件: harness が報告した実測の cost（aider の `Cost:` 行）は上書きしない。cost が無く model が単価表に
/// あれば埋め、model が無い・単価表に無いなら `None` のまま（不明を推測で埋めない）。claude_code・codex は
/// これまで常に上書きしていたので、実測を優先する規則はどの adapter の試験にも無かった。
#[test]
fn run_common_cost_estimate_keeps_reported_cost() {
    let reported = with_estimated_cost(usage(Some(0.5)), Some("claude-sonnet-5"));
    assert_eq!(reported.and_then(|u| u.cost_usd), Some(0.5));

    let estimated = with_estimated_cost(usage(None), Some("claude-sonnet-5"));
    let expected = usage(None).and_then(|u| task_core::estimate_cost_usd("claude-sonnet-5", &u));
    assert!(expected.is_some());
    assert_eq!(estimated.and_then(|u| u.cost_usd), expected);

    assert_eq!(with_estimated_cost(usage(None), None), usage(None));
    assert_eq!(
        with_estimated_cost(usage(None), Some("some-local-llm")),
        usage(None)
    );
    assert_eq!(with_estimated_cost(None, Some("claude-sonnet-5")), None);
}
