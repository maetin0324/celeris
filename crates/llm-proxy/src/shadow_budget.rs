//! ADR 2026-10-04 §7.1・§10 Phase 4: 実行 shadow の日次上限を共有 DB の予約（task-core の
//! `routing_shadow_reservations`、migration 0049）で数える [`ShadowBudget`]。
//!
//! 予約・確定は `SqliteStore::routing_shadow_reserve` / `routing_shadow_settle` が BEGIN IMMEDIATE の
//! 中で行う。同じ DB を開いた別 instance（daemon handoff 中の新旧）・再起動後の proxy も同じ行を
//! 数えるので、UTC 日の request/token/effective 上限を越えない。DB の誤りは fail-closed（送らない）。
//! 時計は注入する（試験は偽時計で UTC 日界を跨ぐ）。

use std::sync::Arc;

use task_core::model_router::shadow::{
    ShadowDailyCaps, ShadowPolicy, ShadowReason, ShadowReservation, ShadowReservationRequest,
    ShadowSettlement,
};
use task_core::store::SqliteStore;
use time::OffsetDateTime;

use crate::reservation::Clock;
use crate::shadow::ShadowBudget;

/// 共有 DB の日次予約。
pub struct StoreShadowBudget {
    store: Arc<SqliteStore>,
    caps: ShadowDailyCaps,
    clock: Arc<dyn Clock>,
}

impl StoreShadowBudget {
    /// `policy` が実行 shadow を許さない（off・不完全）なら `None`。
    pub fn new(
        store: Arc<SqliteStore>,
        policy: &ShadowPolicy,
        clock: Arc<dyn Clock>,
    ) -> Option<Self> {
        Some(Self {
            store,
            caps: policy.daily_caps()?,
            clock,
        })
    }

    pub fn caps(&self) -> ShadowDailyCaps {
        self.caps
    }
}

impl ShadowBudget for StoreShadowBudget {
    fn reserve(
        &self,
        request: &ShadowReservationRequest,
        now: OffsetDateTime,
    ) -> ShadowReservation {
        match self.store.routing_shadow_reserve(&self.caps, request, now) {
            Ok(r) => r,
            Err(e) => {
                // 数えられないなら上限内と言えない: 送らない。
                tracing::warn!(error = %e, "llm-proxy: shadow reservation failed; dropping");
                ShadowReservation::Denied(ShadowReason::CapExceeded)
            }
        }
    }

    fn settle(&self, reservation_id: &str, settlement: ShadowSettlement) {
        match self
            .store
            .routing_shadow_settle(reservation_id, settlement, self.clock.now())
        {
            Ok(true) => {}
            Ok(false) => tracing::warn!(
                reservation_id,
                "llm-proxy: shadow reservation already settled or missing"
            ),
            // 確定できなかった予約は reserved のまま最悪消費で数え続ける（小さく見積もらない）。
            Err(e) => tracing::warn!(error = %e, "llm-proxy: shadow settle failed"),
        }
    }
}
