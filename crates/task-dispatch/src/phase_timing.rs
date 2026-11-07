//! ADR 2026-11-07-dispatch-ready-phase-timing D2: `dispatch_ready` の内側の段の時間を名前ごとに足す表。
//! tick が `dispatch_ready.*` として `slow tick phases` に出す（判断には使わない）。
//!
//! dispatcher module の外に置き、表は thread local にする（tick は同期関数で 1 thread の中で終わる）。
//! 理由は、module の本体の行の合計（repo-health の size.responsibility-growth）を計測の道具のために
//! 増やさないため。

use std::cell::RefCell;
use std::time::Instant;

thread_local! {
    static PARTS: RefCell<Vec<(Phase, u64)>> = const { RefCell::new(Vec::new()) };
}

/// `dispatch_ready` の内側の段。log に出る名前は `dispatch_ready.<段>`。
#[derive(Clone, Copy, PartialEq)]
enum Phase {
    ReadyTasks,
    DispatchOne,
    WorktreeBase,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Phase::ReadyTasks => "dispatch_ready.ready_tasks",
            Phase::DispatchOne => "dispatch_ready.dispatch_one",
            Phase::WorktreeBase => "dispatch_ready.workspaces.worktree_base",
        }
    }
}

/// 生きている間の時間を、drop 時に `name` の段へ足す。
struct PhaseTimer {
    name: Phase,
    started: Instant,
}

fn timer(name: Phase) -> PhaseTimer {
    PhaseTimer {
        name,
        started: Instant::now(),
    }
}

/// `f` の時間を `name` の段に足して、`f` の値を返す。
fn timed<T>(name: Phase, f: impl FnOnce() -> T) -> T {
    let _t = timer(name);
    f()
}

pub(crate) fn ready_tasks<T>(f: impl FnOnce() -> T) -> T {
    timed(Phase::ReadyTasks, f)
}

pub(crate) fn dispatch_one<T>(f: impl FnOnce() -> T) -> T {
    timed(Phase::DispatchOne, f)
}

pub(crate) fn worktree_base<T>(f: impl FnOnce() -> T) -> T {
    timed(Phase::WorktreeBase, f)
}

/// 溜めた段を取り出して空にする。
pub(crate) fn take() -> Vec<(&'static str, u64)> {
    PARTS
        .with(|p| std::mem::take(&mut *p.borrow_mut()))
        .into_iter()
        .map(|(phase, us)| (phase.name(), us))
        .collect()
}

impl Drop for PhaseTimer {
    fn drop(&mut self) {
        let us = u64::try_from(self.started.elapsed().as_micros()).unwrap_or(u64::MAX);
        PARTS.with(|p| {
            let mut parts = p.borrow_mut();
            match parts.iter_mut().find(|(n, _)| *n == self.name) {
                Some((_, total)) => *total = total.saturating_add(us),
                None => parts.push((self.name, us)),
            }
        });
    }
}
