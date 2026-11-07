//! ADR 2026-11-07-dispatch-ready-phase-timing D2: `dispatch_ready` の内側の段の時間を名前ごとに足す表。
//! tick が `dispatch_ready.*` として `slow tick phases` に出す（判断には使わない）。

use std::sync::{Mutex, PoisonError};
use std::time::Instant;

#[derive(Default)]
pub(super) struct ReadyPhases(Mutex<Vec<(&'static str, u64)>>);

impl ReadyPhases {
    /// `started` からの時間を `name` の段に足す。
    pub(super) fn add(&self, name: &'static str, started: Instant) {
        let us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        let mut parts = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        match parts.iter_mut().find(|(n, _)| *n == name) {
            Some((_, total)) => *total = total.saturating_add(us),
            None => parts.push((name, us)),
        }
    }

    /// 溜めた段を取り出して空にする。
    pub(super) fn take(&self) -> Vec<(&'static str, u64)> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}
