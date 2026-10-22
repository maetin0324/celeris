//! harness の試行順を残量スコアで並べ替える純粋関数と、その切替の guardrail
//! （ADR 2026-10-22-harness-balance-and-lane-first-routing D1/D2）。I/O・時計・LLM は無い。
use std::collections::BTreeMap;

/// 残量スコア `scores`（harness → ADR-0024 D3 の最良 account スコア）で `declared` を並べ替える。
///
/// 全ての harness のスコアが分かるときだけ並べ替える（1 つでも不明なら宣言順のまま）。最大スコアから
/// `tolerance` 以内の harness は同等で、同等の中では宣言順。同等集合の先頭を取り出して残りで繰り返す。
pub fn order_harnesses(
    declared: &[String],
    scores: &BTreeMap<String, f64>,
    tolerance: f64,
) -> Vec<String> {
    if declared.len() < 2 || declared.iter().any(|h| !scores.contains_key(h)) {
        return declared.to_vec();
    }
    let tolerance = tolerance.max(0.0);
    let mut rest: Vec<String> = declared.to_vec();
    let mut out = Vec::with_capacity(rest.len());
    while !rest.is_empty() {
        let best = rest
            .iter()
            .map(|h| scores[h])
            .fold(f64::NEG_INFINITY, f64::max);
        let index = rest
            .iter()
            .position(|h| scores[h] >= best - tolerance)
            .unwrap_or(0);
        out.push(rest.remove(index));
    }
    out
}

/// guardrail が見る完了単位 1 件（`completion_outcomes` の投影）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BalanceOutcome {
    /// この単位の run で、このクラスの並べ替えが先頭を変えた。
    pub switched: bool,
    /// 失敗、または検査失敗・review 差し戻しが 1 回以上。
    pub bad: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BalanceTrip {
    pub switched_n: u32,
    pub switched_bad_rate: f64,
    pub other_n: u32,
    pub other_bad_rate: f64,
}

/// 切替した単位が `guard_min` 件以上で、悪い率が切替しなかった単位より `margin` を超えて高ければ止める。
/// 切替しなかった単位が無いときは比べる相手が無いので止めない（捏造しない）。
pub fn evaluate_balance_guardrail(
    outcomes: &[BalanceOutcome],
    guard_min: u32,
    margin: f64,
) -> Option<BalanceTrip> {
    let (switched, other): (Vec<_>, Vec<_>) = outcomes.iter().partition(|o| o.switched);
    if switched.len() < guard_min.max(1) as usize || other.is_empty() {
        return None;
    }
    let rate = |v: &[&BalanceOutcome]| v.iter().filter(|o| o.bad).count() as f64 / v.len() as f64;
    let (s, o) = (rate(&switched), rate(&other));
    (s - o > margin).then_some(BalanceTrip {
        switched_n: switched.len() as u32,
        switched_bad_rate: s,
        other_n: other.len() as u32,
        other_bad_rate: o,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scores(items: &[(&str, f64)]) -> BTreeMap<String, f64> {
        items.iter().map(|(k, v)| ((*k).into(), *v)).collect()
    }
    fn declared() -> Vec<String> {
        vec!["claude_code".into(), "codex".into()]
    }

    #[test]
    fn the_harness_with_more_headroom_goes_first() {
        let s = scores(&[("claude_code", 0.2), ("codex", 0.9)]);
        assert_eq!(
            order_harnesses(&declared(), &s, 0.15),
            ["codex", "claude_code"]
        );
    }

    #[test]
    fn a_difference_inside_the_tolerance_keeps_the_declared_order() {
        let s = scores(&[("claude_code", 0.8), ("codex", 0.9)]);
        assert_eq!(
            order_harnesses(&declared(), &s, 0.15),
            ["claude_code", "codex"]
        );
        // 幅ちょうどは同等、超えると入れ替わる。
        let s = scores(&[("claude_code", 0.75), ("codex", 0.9)]);
        assert_eq!(
            order_harnesses(&declared(), &s, 0.15),
            ["claude_code", "codex"]
        );
        let s = scores(&[("claude_code", 0.7), ("codex", 0.9)]);
        assert_eq!(
            order_harnesses(&declared(), &s, 0.15),
            ["codex", "claude_code"]
        );
    }

    #[test]
    fn an_unknown_score_keeps_the_declared_order() {
        let s = scores(&[("codex", 0.9)]);
        assert_eq!(
            order_harnesses(&declared(), &s, 0.15),
            ["claude_code", "codex"]
        );
    }

    #[test]
    fn three_harnesses_are_ordered_by_repeated_selection() {
        let d: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        let s = scores(&[("a", 0.1), ("b", 0.5), ("c", 0.55)]);
        assert_eq!(order_harnesses(&d, &s, 0.15), ["b", "c", "a"]);
    }

    fn outcomes(switched: (u32, u32), other: (u32, u32)) -> Vec<BalanceOutcome> {
        let mut v = Vec::new();
        for (n, bad, switch) in [(switched.0, switched.1, true), (other.0, other.1, false)] {
            for i in 0..n {
                v.push(BalanceOutcome {
                    switched: switch,
                    bad: i < bad,
                });
            }
        }
        v
    }

    #[test]
    fn guardrail_trips_when_switched_units_are_clearly_worse() {
        let trip = evaluate_balance_guardrail(&outcomes((10, 5), (20, 2)), 10, 0.15).unwrap();
        assert_eq!(trip.switched_n, 10);
        assert!((trip.switched_bad_rate - 0.5).abs() < 1e-9);
        assert!((trip.other_bad_rate - 0.1).abs() < 1e-9);
    }

    #[test]
    fn guardrail_does_not_trip_on_few_samples_or_a_small_difference() {
        assert!(evaluate_balance_guardrail(&outcomes((9, 9), (20, 0)), 10, 0.15).is_none());
        assert!(evaluate_balance_guardrail(&outcomes((10, 3), (20, 4)), 10, 0.15).is_none());
        assert!(evaluate_balance_guardrail(&outcomes((10, 10), (0, 0)), 10, 0.15).is_none());
    }
}
