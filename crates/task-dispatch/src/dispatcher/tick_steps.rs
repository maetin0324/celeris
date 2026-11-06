//! 第 4 フェーズ P4-A（ADR 2026-11-05-phase4-p4a-baseline-contract D3）: tick の段ごとの所要時間。
//! blocked の task の数に比例して伸びる段を特定するための計測の口で、判断には使わない
//! （`TickReport::step_us`、`crates/celeris/tests/tick_scale_baseline.rs`）。遅い tick の警告もこの値を出す。

use std::time::{Duration, Instant};

/// 遅い tick の内訳を警告に出す閾値（ADR-0015 D2）。
pub(super) const SLOW_TICK: Duration = Duration::from_secs(1);

pub(super) struct TickSteps {
    started: Instant,
    at: Instant,
    out: Vec<(&'static str, u64)>,
}

impl TickSteps {
    pub(super) fn start() -> Self {
        let now = Instant::now();
        Self {
            started: now,
            at: now,
            out: Vec::new(),
        }
    }

    /// 前の印からの時間を `name` の段として積む。
    pub(super) fn mark(&mut self, name: &'static str) {
        let us = u64::try_from(self.at.elapsed().as_micros()).unwrap_or(u64::MAX);
        self.out.push((name, us));
        self.at = Instant::now();
    }

    /// 段の内側の小段を、現在の `mark` とは別に `name.sub` の名前で積む（時間は足さない。内訳の観測用）。
    pub(super) fn sub(&mut self, parts: Vec<(&'static str, u64)>) {
        self.out.extend(parts);
    }

    /// 段の列を返し、tick が遅ければ内訳を警告に出す。
    pub(super) fn finish(self) -> Vec<(&'static str, u64)> {
        let total = self.started.elapsed();
        if total >= SLOW_TICK {
            tracing::warn!(
                total_ms = total.as_millis() as u64,
                steps_us = ?self.out,
                "slow tick phases"
            );
        }
        self.out
    }
}
