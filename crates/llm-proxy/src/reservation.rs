//! 同時要求の枠の予約表（ADR 2026-10-04-multi-objective-model-routing Phase 2・p2-proxy-select）。
//!
//! state 選択（[`crate::selection::state`]）が選んだ候補は、上流へ送る前に枠を予約する。枠は 2 種で、
//! **別々に数える**:
//! - account の枠（CLI/proxy が共有する OAuth account の window。`source_ref` + account id）
//! - 共有 GPU の枠（self-host の resource group。同じ group の deployment は 1 つの枠を共有する）
//!
//! 1 要求が同じ枠を二重に取ることはない（枠の列は重複を除いてから数える）。予約は全枠を
//! まとめて取るか、1 つも取らないか（途中まで取って残すことはない）。in-process の `Mutex` で守る。
//!
//! 競合（選んだ時点では空いていたが、予約の時点で埋まっていた）で落ちたら、**1 回だけ**選び直す
//! （[`reserve_with_reselect`]）。2 回目も落ちたら選び直さずに競合として返す。

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use time::OffsetDateTime;

/// 時計の注入口（試験は固定の時刻を差し込む）。
pub trait Clock: Send + Sync {
    fn now(&self) -> OffsetDateTime;
}

/// 実時計。
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

/// 固定時計（試験・再現用）。
#[derive(Debug, Clone, Copy)]
pub struct FixedClock(pub OffsetDateTime);

impl Clock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        self.0
    }
}

/// 予約の枠。account と共有 GPU は別の種類として数える。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SlotKey {
    /// OAuth account の window（`source_ref` は `claude-oauth` / `codex-oauth` など）。
    Account {
        source_ref: String,
        account_id: String,
    },
    /// 共有 GPU の resource group。
    ResourceGroup(String),
}

impl SlotKey {
    pub fn account(source_ref: impl Into<String>, account_id: impl Into<String>) -> Self {
        Self::Account {
            source_ref: source_ref.into(),
            account_id: account_id.into(),
        }
    }

    pub fn group(id: impl Into<String>) -> Self {
        Self::ResourceGroup(id.into())
    }
}

/// 枠ごとの上限。未設定の枠は上限なし（数えるだけ）。
#[derive(Debug, Clone, Default)]
pub struct CapacityLimits {
    /// account 1 つあたりの同時数（`max_concurrent_per_account`）。
    pub per_account: Option<u32>,
    /// resource group ごとの同時数。
    pub resource_groups: HashMap<String, u32>,
}

impl CapacityLimits {
    fn limit(&self, slot: &SlotKey) -> Option<u32> {
        match slot {
            SlotKey::Account { .. } => self.per_account,
            SlotKey::ResourceGroup(id) => self.resource_groups.get(id).copied(),
        }
    }
}

#[derive(Debug, Default)]
struct Counts {
    held: HashMap<SlotKey, u32>,
    peak: HashMap<SlotKey, u32>,
}

/// 予約の競合（どの枠が埋まっていたか）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("reservation conflict: {full:?}")]
pub struct Conflict {
    pub full: Vec<SlotKey>,
}

/// in-process の予約表。`Arc` で共有する。
#[derive(Debug)]
pub struct ReservationTable {
    limits: CapacityLimits,
    counts: Mutex<Counts>,
}

impl ReservationTable {
    pub fn new(limits: CapacityLimits) -> Arc<Self> {
        Arc::new(Self {
            limits,
            counts: Mutex::new(Counts::default()),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Counts> {
        // 数え上げだけなので、毒された lock でも中身は整合している（更新は 1 文ずつ）。
        self.counts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 今の保持数。
    pub fn held(&self, slot: &SlotKey) -> u32 {
        self.lock().held.get(slot).copied().unwrap_or(0)
    }

    /// これまでの最大保持数（上限を超えていないことの確かめ用）。
    pub fn peak(&self, slot: &SlotKey) -> u32 {
        self.lock().peak.get(slot).copied().unwrap_or(0)
    }

    /// 埋まっている枠（`slots` のうち、あと 1 つ取ると上限を超えるもの）。空なら予約できる見込み。
    pub fn full_slots(&self, slots: &[SlotKey]) -> Vec<SlotKey> {
        let counts = self.lock();
        dedup(slots)
            .into_iter()
            .filter(|s| self.is_full(&counts, s))
            .collect()
    }

    fn is_full(&self, counts: &Counts, slot: &SlotKey) -> bool {
        self.limits
            .limit(slot)
            .is_some_and(|max| counts.held.get(slot).copied().unwrap_or(0) >= max)
    }

    /// 全枠をまとめて取る。1 つでも埋まっていれば何も取らずに `Conflict`。
    pub fn try_reserve(self: &Arc<Self>, slots: &[SlotKey]) -> Result<Reservation, Conflict> {
        let slots = dedup(slots);
        let mut counts = self.lock();
        let full: Vec<_> = slots
            .iter()
            .filter(|s| self.is_full(&counts, s))
            .cloned()
            .collect();
        if !full.is_empty() {
            return Err(Conflict { full });
        }
        for s in &slots {
            let held = counts.held.entry(s.clone()).or_insert(0);
            *held += 1;
            let now = *held;
            let peak = counts.peak.entry(s.clone()).or_insert(0);
            *peak = (*peak).max(now);
        }
        Ok(Reservation {
            table: Arc::clone(self),
            slots,
        })
    }

    fn release(&self, slots: &[SlotKey]) {
        let mut counts = self.lock();
        for s in slots {
            if let Some(held) = counts.held.get_mut(s) {
                *held = held.saturating_sub(1);
            }
        }
    }
}

fn dedup(slots: &[SlotKey]) -> Vec<SlotKey> {
    slots
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// 取った枠。drop で返す。
#[derive(Debug)]
pub struct Reservation {
    table: Arc<ReservationTable>,
    slots: Vec<SlotKey>,
}

impl Reservation {
    pub fn slots(&self) -> &[SlotKey] {
        &self.slots
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.table.release(&self.slots);
    }
}

/// 予約の対象（選択の結果）。`T` は呼び出し側の選択（state 選択の pick など）。
pub trait Reservable {
    fn slots(&self) -> Vec<SlotKey>;
}

/// 予約付きの選択の結果。
#[derive(Debug)]
pub struct Reserved<T> {
    pub pick: T,
    pub reservation: Reservation,
    /// 競合で選び直した回数（0 か 1）。
    pub reselects: u32,
}

/// 予約できなかった理由。
#[derive(Debug, PartialEq, Eq)]
pub enum ReserveError {
    /// 選べる候補が無い（`attempt` 回目の選択で）。
    NoCandidate { attempt: u32 },
    /// 選び直した後も競合した（選び直しは 1 回だけ）。
    Conflict { conflict: Conflict, reselects: u32 },
}

/// 選択 → 予約。競合したら 1 回だけ選び直す（`select(1, table)`）。2 回目の競合は選び直さない。
///
/// `select(attempt, table)` は `attempt` = 0（初回）か 1（選び直し）で呼ばれる。選び直しでは
/// 予約表の今の埋まり具合（[`ReservationTable::full_slots`]）を見て、埋まった枠の候補を落とす。
pub fn reserve_with_reselect<T: Reservable>(
    table: &Arc<ReservationTable>,
    mut select: impl FnMut(u32, &ReservationTable) -> Option<T>,
) -> Result<Reserved<T>, ReserveError> {
    let first = select(0, table).ok_or(ReserveError::NoCandidate { attempt: 0 })?;
    match table.try_reserve(&first.slots()) {
        Ok(reservation) => Ok(Reserved {
            pick: first,
            reservation,
            reselects: 0,
        }),
        Err(_) => {
            let second = select(1, table).ok_or(ReserveError::NoCandidate { attempt: 1 })?;
            match table.try_reserve(&second.slots()) {
                Ok(reservation) => Ok(Reserved {
                    pick: second,
                    reservation,
                    reselects: 1,
                }),
                Err(conflict) => Err(ReserveError::Conflict {
                    conflict,
                    reselects: 1,
                }),
            }
        }
    }
}

#[cfg(test)]
#[path = "reservation_tests.rs"]
mod tests;
