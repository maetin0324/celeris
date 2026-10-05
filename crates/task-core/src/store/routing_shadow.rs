//! ADR 2026-10-04-multi-objective-model-routing §7.1・Phase 4: 実行 shadow の UTC 日次上限の共有予約
//! （`routing_shadow_reservations`、migration 0049）。
//!
//! 予約・確定は BEGIN IMMEDIATE の中でその日の和を読み直してから書く。同じ DB を開いた別接続・
//! 再起動後の store・handoff 後の daemon も同じ行を数えるので、日次の request/token/effective 上限を
//! 越えない。時計は呼び手が `now` で渡す（試験は偽時計で日界を跨ぐ）。

use rusqlite::{OptionalExtension, TransactionBehavior, params};
use time::OffsetDateTime;
use ulid::Ulid;

use crate::model_router::shadow::{
    ShadowDailyCaps, ShadowDailyUsage, ShadowPolicy, ShadowReason, ShadowReservation,
    ShadowReservationRequest, ShadowSettlement, ShadowTarget, usd_cap_to_micros_floor,
    usd_to_micros_ceil, utc_day,
};

use super::{SqliteStore, StoreError, format_rfc3339};

fn to_i64(name: &str, v: u64) -> Result<i64, StoreError> {
    i64::try_from(v).map_err(|_| StoreError::Invalid(format!("{name} out of range: {v}")))
}

fn to_u64(v: i64) -> u64 {
    u64::try_from(v).unwrap_or(0)
}

fn usage_in(conn: &rusqlite::Connection, day: &str) -> Result<ShadowDailyUsage, StoreError> {
    let (requests, tokens, micros, open): (i64, i64, i64, i64) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(charged_tokens), 0), COALESCE(SUM(charged_micro_usd), 0), \
         COALESCE(SUM(state = 'reserved'), 0) FROM routing_shadow_reservations WHERE day = ?1",
        params![day],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    Ok(ShadowDailyUsage {
        requests: to_u64(requests),
        tokens: to_u64(tokens),
        effective_usd: to_u64(micros) as f64 / 1_000_000.0,
        open_reservations: to_u64(open),
    })
}

impl SqliteStore {
    /// 最悪消費を `now` の UTC 日に予約する。未知の費用は `Denied(UnknownCost)`、どれか 1 つの上限を
    /// 越えるなら `Denied(CapExceeded)`（どちらも行を書かない）。
    pub fn routing_shadow_reserve(
        &self,
        caps: &ShadowDailyCaps,
        request: &ShadowReservationRequest,
        now: OffsetDateTime,
    ) -> Result<ShadowReservation, StoreError> {
        let Some(worst_micros) = request.worst_effective_usd.and_then(usd_to_micros_ceil) else {
            return Ok(ShadowReservation::Denied(ShadowReason::UnknownCost));
        };
        let worst_tokens = to_i64("worst_tokens", request.worst_tokens)?;
        let worst_micros_i = to_i64("worst_effective_usd", worst_micros)?;
        let day = utc_day(now);
        let at = format_rfc3339(now)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (requests, tokens, micros): (i64, i64, i64) = tx.query_row(
            "SELECT COUNT(*), COALESCE(SUM(charged_tokens), 0), COALESCE(SUM(charged_micro_usd), 0) \
             FROM routing_shadow_reservations WHERE day = ?1",
            params![day],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let cap_micros = usd_cap_to_micros_floor(caps.max_effective_usd);
        let fits = to_u64(requests).saturating_add(1) <= caps.max_requests
            && to_u64(tokens).saturating_add(request.worst_tokens) <= caps.max_tokens
            && to_u64(micros).saturating_add(worst_micros) <= cap_micros;
        if !fits {
            // 書かずに終える（drop で rollback）。
            return Ok(ShadowReservation::Denied(ShadowReason::CapExceeded));
        }
        let id = Ulid::new().to_string();
        tx.execute(
            "INSERT INTO routing_shadow_reservations (id, day, shadow_id, owner, state, \
             reserved_tokens, reserved_micro_usd, charged_tokens, charged_micro_usd, reserved_at) \
             VALUES (?1, ?2, ?3, ?4, 'reserved', ?5, ?6, ?5, ?6, ?7)",
            params![
                id,
                day,
                request.shadow_id,
                request.owner,
                worst_tokens,
                worst_micros_i,
                at
            ],
        )?;
        tx.commit()?;
        Ok(ShadowReservation::Reserved {
            reservation_id: id,
            day,
        })
    }

    /// 入口の判定（off・allowlist 外・標本外）を先に行い、通ったときだけ予約する。判定で落ちたものは
    /// 行を書かない（予約 0）。
    pub fn routing_shadow_admit_and_reserve(
        &self,
        policy: &ShadowPolicy,
        target: &ShadowTarget,
        decision_id: &str,
        request: &ShadowReservationRequest,
        now: OffsetDateTime,
    ) -> Result<ShadowReservation, StoreError> {
        if let Err(reason) = policy.admit(target, decision_id) {
            return Ok(ShadowReservation::Denied(reason));
        }
        let Some(caps) = policy.daily_caps() else {
            return Ok(ShadowReservation::Denied(ShadowReason::Off));
        };
        self.routing_shadow_reserve(&caps, request, now)
    }

    /// 予約を確定する（予約した日に計上する）。既に確定済み・存在しない予約は `Ok(false)`（冪等）。
    pub fn routing_shadow_settle(
        &self,
        reservation_id: &str,
        settlement: ShadowSettlement,
        now: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let at = format_rfc3339(now)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let reserved: Option<(i64, i64)> = tx
            .query_row(
                "SELECT reserved_tokens, reserved_micro_usd FROM routing_shadow_reservations \
                 WHERE id = ?1 AND state = 'reserved'",
                params![reservation_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((reserved_tokens, reserved_micros)) = reserved else {
            return Ok(false);
        };
        let (state, tokens, micros) = match settlement {
            ShadowSettlement::Completed {
                tokens,
                effective_usd,
            } => {
                // 実測の費用が測れないなら予約額のまま数える（小さく見積もらない）。
                let micros = usd_to_micros_ceil(effective_usd)
                    .map(|m| to_i64("effective_usd", m))
                    .transpose()?
                    .unwrap_or(reserved_micros);
                ("completed", to_i64("tokens", tokens)?, micros)
            }
            ShadowSettlement::Failed {
                tokens,
                effective_usd,
            } => {
                let t = tokens.map(|t| to_i64("tokens", t)).transpose()?;
                let m = effective_usd
                    .and_then(usd_to_micros_ceil)
                    .map(|m| to_i64("effective_usd", m))
                    .transpose()?;
                (
                    "failed",
                    t.map_or(reserved_tokens, |t| t.max(reserved_tokens)),
                    m.map_or(reserved_micros, |m| m.max(reserved_micros)),
                )
            }
            ShadowSettlement::TimedOut => ("timed_out", reserved_tokens, reserved_micros),
        };
        tx.execute(
            "UPDATE routing_shadow_reservations SET state = ?2, charged_tokens = ?3, \
             charged_micro_usd = ?4, settled_at = ?5 WHERE id = ?1",
            params![reservation_id, state, tokens, micros, at],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// `now` の UTC 日の消費（予約中 + 確定）。
    pub fn routing_shadow_usage(
        &self,
        now: OffsetDateTime,
    ) -> Result<ShadowDailyUsage, StoreError> {
        let day = utc_day(now);
        self.with_read_conn(|conn| usage_in(conn, &day))
    }
}
