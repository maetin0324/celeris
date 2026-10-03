//! 工程の効き目の A/B 試験（phase_effect_ab）。同じ scenario を新しい仕組みを切った variant（off）と
//! 入れた variant（on）で走らせ、run 数・壁時計・入力 token・新しい session の数を `AbMetric` に集め、
//! 1 行 `ab-metric <scenario> <off|on> runs=… wall_secs=… input_tokens=… fresh_sessions=…` を stdout に出す。
//! 偽のアダプタと注入した時計だけを使い、実 claude・外部ネットワーク・userns には出ない。
//! scenario ごとの本体は submodule に置く（後続の scenario は submodule の中だけを埋める）。

/// atomic coding task の planner なし直行経路（ADR-0124）。
mod atomic_route;
/// continuation（予算切れ・yield の続き）の同一 session resume（ADR-0140 D1）。
mod continuation;
/// review 前の target 同期（ADR-0118）と、その衝突の IntegrationRepair（ADR-0120、Phase 2）。
mod review_sync;
/// review 前 sync の stale 優先（ADR-0130 D5、Phase 5）。
mod stale_priority;
/// expected write-set の重なりで run を待たせる gate（ADR-0130 D3、Phase 5）。
mod write_set;

/// 1 つの variant の測り値。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct AbMetric {
    pub(super) runs: u64,
    pub(super) wall_secs: u64,
    pub(super) input_tokens: u64,
    pub(super) fresh_sessions: u64,
}

/// A/B の片側。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Variant {
    Off,
    On,
}

impl Variant {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Variant::Off => "off",
            Variant::On => "on",
        }
    }
}

/// `ab-metric` の 1 行（集計台本が grep する形）。
pub(super) fn ab_metric_line(scenario: &str, variant: Variant, m: &AbMetric) -> String {
    format!(
        "ab-metric {scenario} {} runs={} wall_secs={} input_tokens={} fresh_sessions={}",
        variant.as_str(),
        m.runs,
        m.wall_secs,
        m.input_tokens,
        m.fresh_sessions
    )
}

/// `ab_metric_line` の行末に scenario 固有の欄（`conflicts=…` など）を足した行。既存の欄の形は変えない。
pub(super) fn ab_metric_line_with(
    scenario: &str,
    variant: Variant,
    m: &AbMetric,
    extra: &[(&str, u64)],
) -> String {
    let mut line = ab_metric_line(scenario, variant, m);
    for (key, value) in extra {
        line.push_str(&format!(" {key}={value}"));
    }
    line
}

/// `ab_metric_line_with` の 1 行を stdout に出す。
pub(super) fn print_ab_metric_with(
    scenario: &str,
    variant: Variant,
    m: &AbMetric,
    extra: &[(&str, u64)],
) {
    println!("{}", ab_metric_line_with(scenario, variant, m, extra));
}

/// `ab-metric` の 1 行を stdout に出す。
pub(super) fn print_ab_metric(scenario: &str, variant: Variant, m: &AbMetric) {
    println!("{}", ab_metric_line(scenario, variant, m));
}

/// on が off より run 数か新しい session の数を少なくしていることを確かめる。
pub(super) fn assert_on_reduces_runs_or_fresh_sessions(
    scenario: &str,
    off: &AbMetric,
    on: &AbMetric,
) {
    assert!(
        on.runs < off.runs || on.fresh_sessions < off.fresh_sessions,
        "{scenario}: on must reduce runs or fresh_sessions (off={off:?}, on={on:?})"
    );
}

#[test]
fn phase_effect_ab_metric_line_format() {
    let m = AbMetric {
        runs: 3,
        wall_secs: 90,
        input_tokens: 30_000,
        fresh_sessions: 3,
    };
    assert_eq!(
        ab_metric_line("continuation", Variant::Off, &m),
        "ab-metric continuation off runs=3 wall_secs=90 input_tokens=30000 fresh_sessions=3"
    );
}

#[test]
fn phase_effect_ab_metric_line_with_extra_fields() {
    let m = AbMetric {
        runs: 2,
        wall_secs: 120,
        input_tokens: 80_000,
        fresh_sessions: 2,
    };
    assert_eq!(
        ab_metric_line_with(
            "write_set",
            Variant::On,
            &m,
            &[("conflicts", 0), ("repairs", 0)]
        ),
        "ab-metric write_set on runs=2 wall_secs=120 input_tokens=80000 fresh_sessions=2 conflicts=0 repairs=0"
    );
}
