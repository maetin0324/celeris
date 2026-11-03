//! ADR 2026-11-02-ts-control-layer-boundary 付記 S5-7: 表 `script_guard_state`（migration 0105）。
//!
//! - (domain, generation_id) ごとの計数は再起動の後も残す。数え始めた mode と今の mode が違えば数え直す。
//! - 判定は `crate::script_guard::evaluate`。当たったら同じ transaction で mode の行を `rust` に戻し、
//!   `system:script-guard` の記録を追記する。`compare` は計数だけを残す。自動で昇格する経路は無い。
//! - 領域 `*` の行は全体の停止 `halted`。解くのは人だけ。

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use time::OffsetDateTime;

use super::script_domains::{
    append_generation_event_tx, invalidate_tx, read_generation_tx, read_mode_tx, write_mode_tx,
};
use super::{SqliteStore, StoreError, format_rfc3339};
use crate::model::TaskId;
use crate::script_compare::{
    CosDigestCompareRecord, ScriptCompareRecord, ScriptDomain, ScriptDomainError,
    ScriptGenerationEvent, ScriptGenerationEventKind, ScriptGenerationState, ScriptMode,
    actor_may_enable_ts,
};
use crate::script_guard::{
    SCRIPT_GUARD_ACTOR, ScriptGuardCall, ScriptGuardCounters, ScriptGuardObservation,
    ScriptGuardOutcome, ScriptGuardStop, evaluate,
};

const HALT_DOMAIN: &str = "*";
const HALT_GENERATION: &str = "";
/// 全体の停止の記録の `generation_id`。
const HALT_EVENT_GENERATION: &str = "*";

impl SqliteStore {
    /// 呼び出し 1 件を数え、(f) を当てる（S5-7）。`canary`・`ts` で条件に当たれば同じ transaction で `rust` へ戻し、
    /// 安全違反なら mode にかかわらず generation を `invalid` にして指す全領域を戻す。
    pub fn script_guard_observe(
        &self,
        domain: ScriptDomain,
        generation_id: &str,
        observation: ScriptGuardObservation,
        now: OffsetDateTime,
    ) -> Result<ScriptGuardOutcome, ScriptDomainError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let outcome = observe_tx(&tx, domain, generation_id, observation, now)?;
        tx.commit()?;
        Ok(outcome)
    }

    /// `script_compare_recorded` の追記と計数を 1 つの transaction で行う（S5-7）。重複で追記しなかった記録と
    /// `unavailable` は数えない（S2-8）。戻り値は追記したかと計数の結果。
    pub fn script_compare_append_observed(
        &self,
        task_id: TaskId,
        record: &ScriptCompareRecord,
        ts_side: bool,
        now: OffsetDateTime,
    ) -> Result<(bool, Option<ScriptGuardOutcome>), ScriptDomainError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let appended = Self::script_compare_append_tx(&tx, task_id, record, now)?;
        let call = ScriptGuardCall::classify(
            record.outcome,
            record.error.as_ref().map(|e| e.kind.as_str()),
        );
        let outcome = match call.filter(|_| appended) {
            Some(call) => Some(observe_tx(
                &tx,
                record.domain,
                &record.generation,
                ScriptGuardObservation {
                    call,
                    safety_violation: !record.safety_violations.is_empty(),
                    ts_side,
                },
                now,
            )?),
            None => None,
        };
        tx.commit()?;
        Ok((appended, outcome))
    }

    /// `script_compare_log` の追記と計数を 1 つの transaction で行う（`cos_digest`。S5-7）。
    pub fn script_compare_log_append_observed(
        &self,
        record: &CosDigestCompareRecord,
        ts_side: bool,
        now: OffsetDateTime,
    ) -> Result<(bool, Option<ScriptGuardOutcome>), ScriptDomainError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let appended = Self::script_compare_log_append_tx(&tx, record, now)?;
        let call = ScriptGuardCall::classify(
            record.outcome,
            record.error.as_ref().map(|e| e.kind.as_str()),
        );
        let outcome = match call.filter(|_| appended) {
            Some(call) => Some(observe_tx(
                &tx,
                record.domain,
                &record.generation,
                ScriptGuardObservation {
                    call,
                    safety_violation: !record.safety_violations.is_empty(),
                    ts_side,
                },
                now,
            )?),
            None => None,
        };
        tx.commit()?;
        Ok((appended, outcome))
    }

    /// (domain, generation_id) の計数と、数え始めた mode。
    pub fn script_guard_counters(
        &self,
        domain: ScriptDomain,
        generation_id: &str,
    ) -> Result<Option<(ScriptMode, ScriptGuardCounters)>, StoreError> {
        self.with_read_conn(|conn| read_counters_tx(conn, domain, generation_id))
    }

    /// 全体の停止の理由（止まっていなければ `None`）。
    pub fn script_guard_halted(&self) -> Result<Option<String>, StoreError> {
        self.with_read_conn(read_halted_tx)
    }

    /// 全領域を `rust` に戻して全体を止める（S5-7。Rust の検証を通らない副作用を検出したとき）。戻した領域を返す。
    /// すでに止まっていれば何も変えない。
    pub fn script_guard_halt(
        &self,
        reason: &str,
        now: OffsetDateTime,
    ) -> Result<Vec<ScriptDomain>, ScriptDomainError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if read_halted_tx(&tx)?.is_some() {
            tx.commit()?;
            return Ok(Vec::new());
        }
        let ts = format_rfc3339(now)?;
        let mut rolled_back = Vec::new();
        for domain in ScriptDomain::ALL {
            if let Some(row) = read_mode_tx(&tx, domain)?
                && row.mode != ScriptMode::Rust
            {
                write_mode_tx(
                    &tx,
                    &row,
                    ScriptMode::Rust,
                    None,
                    None,
                    SCRIPT_GUARD_ACTOR,
                    reason,
                    &ts,
                )?;
                rolled_back.push(domain);
            }
        }
        write_halt_tx(&tx, true, Some(reason), &ts)?;
        append_halt_event_tx(
            &tx,
            ScriptGenerationEventKind::ScriptGuardHalted,
            SCRIPT_GUARD_ACTOR,
            reason,
            &ts,
        )?;
        tx.commit()?;
        Ok(rolled_back)
    }

    /// 全体の停止を解く（S5-4。人の actor だけ）。止まっていなければ `Ok(false)` で何も変えない。
    pub fn script_guard_clear(
        &self,
        actor: &str,
        reason: &str,
        now: OffsetDateTime,
    ) -> Result<bool, ScriptDomainError> {
        if !actor_may_enable_ts(actor) {
            return Err(ScriptDomainError::GuardClearRequiresHuman(actor.to_owned()));
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if read_halted_tx(&tx)?.is_none() {
            tx.commit()?;
            return Ok(false);
        }
        let ts = format_rfc3339(now)?;
        write_halt_tx(&tx, false, None, &ts)?;
        append_halt_event_tx(
            &tx,
            ScriptGenerationEventKind::ScriptGuardCleared,
            actor,
            reason,
            &ts,
        )?;
        tx.commit()?;
        Ok(true)
    }
}

fn observe_tx(
    tx: &Connection,
    domain: ScriptDomain,
    generation_id: &str,
    observation: ScriptGuardObservation,
    now: OffsetDateTime,
) -> Result<ScriptGuardOutcome, ScriptDomainError> {
    let current = read_mode_tx(tx, domain)?
        .ok_or_else(|| ScriptDomainError::UnknownDomain(domain.as_str().to_owned()))?;
    let points_here = current.generation_id.as_deref() == Some(generation_id);
    let stored = read_counters_tx(tx, domain, generation_id)?;
    // 切替の後に届いた in-flight の結果は、数え始めた mode のまま足す。
    let (mode, mut counters) = match stored {
        Some((mode, counters)) if !points_here || mode == current.mode => (mode, counters),
        _ => (current.mode, ScriptGuardCounters::default()),
    };
    counters.observe(observation, now);
    let ts = format_rfc3339(now)?;
    write_counters_tx(tx, domain, generation_id, mode, &counters, &ts)?;

    let stop = evaluate(&counters, now);
    let mut outcome = ScriptGuardOutcome {
        stop,
        counters,
        rolled_back: Vec::new(),
        invalidated: false,
    };
    let Some(stop) = stop else {
        return Ok(outcome);
    };
    let reason = stop_reason(stop, &outcome.counters);
    if stop == ScriptGuardStop::GuardSafetyViolation {
        if let Some(generation) = read_generation_tx(tx, generation_id)?
            && generation.state != ScriptGenerationState::Invalid
        {
            let (_, rolled_back) =
                invalidate_tx(tx, &generation, SCRIPT_GUARD_ACTOR, &reason, &ts)?;
            outcome.rolled_back = rolled_back;
            outcome.invalidated = true;
        }
        return Ok(outcome);
    }
    if points_here && matches!(current.mode, ScriptMode::Canary | ScriptMode::Ts) {
        write_mode_tx(
            tx,
            &current,
            ScriptMode::Rust,
            None,
            None,
            SCRIPT_GUARD_ACTOR,
            &reason,
            &ts,
        )?;
        outcome.rolled_back.push(domain);
    }
    Ok(outcome)
}

fn stop_reason(stop: ScriptGuardStop, counters: &ScriptGuardCounters) -> String {
    format!(
        "{}: calls {}, window {}, mismatches {}, timeouts {}, timeouts_10min {}, failures {}, consecutive_failures {}, safety_violations {}",
        stop.code(),
        counters.calls,
        counters.window.len(),
        counters.mismatches_in_window(),
        counters.timeouts_in_window(),
        counters.timeouts_10min.len(),
        counters.failures_in_window(),
        counters.consecutive_failures,
        counters.safety_violations,
    )
}

/// 昇格の前提に使う計数。数え始めた mode が `mode` でなければ 0 から（今の mode の分はまだ無い）。
pub(super) fn counters_for_mode_tx(
    tx: &Connection,
    domain: ScriptDomain,
    generation_id: &str,
    mode: ScriptMode,
) -> Result<ScriptGuardCounters, StoreError> {
    Ok(read_counters_tx(tx, domain, generation_id)?
        .filter(|(counted, _)| *counted == mode)
        .map(|(_, counters)| counters)
        .unwrap_or_default())
}

fn read_counters_tx(
    conn: &Connection,
    domain: ScriptDomain,
    generation_id: &str,
) -> Result<Option<(ScriptMode, ScriptGuardCounters)>, StoreError> {
    let raw = conn
        .query_row(
            "SELECT mode, \"window\", consecutive_failures, timeouts_10min, clean_streak, \
                    ts_side_clean_streak, calls, safety_violations \
             FROM script_guard_state WHERE domain = ?1 AND generation_id = ?2",
            params![domain.as_str(), generation_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            },
        )
        .optional()?;
    let Some((mode, window, failures, timeouts, clean, ts_clean, calls, safety)) = raw else {
        return Ok(None);
    };
    let mode = ScriptMode::parse(&mode)
        .ok_or_else(|| StoreError::Invalid(format!("unknown script mode `{mode}`")))?;
    Ok(Some((
        mode,
        ScriptGuardCounters {
            window,
            consecutive_failures: to_u64(failures)?,
            timeouts_10min: serde_json::from_str(&timeouts)?,
            clean_streak: to_u64(clean)?,
            ts_side_clean_streak: to_u64(ts_clean)?,
            calls: to_u64(calls)?,
            safety_violations: to_u64(safety)?,
        },
    )))
}

fn write_counters_tx(
    tx: &Connection,
    domain: ScriptDomain,
    generation_id: &str,
    mode: ScriptMode,
    counters: &ScriptGuardCounters,
    ts: &str,
) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO script_guard_state \
            (domain, generation_id, mode, \"window\", consecutive_failures, timeouts_10min, \
             clean_streak, ts_side_clean_streak, calls, safety_violations, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) \
         ON CONFLICT (domain, generation_id) DO UPDATE SET \
            mode = excluded.mode, \"window\" = excluded.\"window\", \
            consecutive_failures = excluded.consecutive_failures, \
            timeouts_10min = excluded.timeouts_10min, clean_streak = excluded.clean_streak, \
            ts_side_clean_streak = excluded.ts_side_clean_streak, calls = excluded.calls, \
            safety_violations = excluded.safety_violations, updated_at = excluded.updated_at",
        params![
            domain.as_str(),
            generation_id,
            mode.as_str(),
            counters.window,
            to_i64(counters.consecutive_failures)?,
            serde_json::to_string(&counters.timeouts_10min)?,
            to_i64(counters.clean_streak)?,
            to_i64(counters.ts_side_clean_streak)?,
            to_i64(counters.calls)?,
            to_i64(counters.safety_violations)?,
            ts,
        ],
    )?;
    Ok(())
}

pub(super) fn read_halted_tx(conn: &Connection) -> Result<Option<String>, StoreError> {
    let row: Option<(i64, Option<String>)> = conn
        .query_row(
            "SELECT halted, halt_reason FROM script_guard_state \
             WHERE domain = ?1 AND generation_id = ?2",
            params![HALT_DOMAIN, HALT_GENERATION],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    Ok(row
        .filter(|(halted, _)| *halted != 0)
        .map(|(_, reason)| reason.unwrap_or_default()))
}

fn write_halt_tx(
    tx: &Connection,
    halted: bool,
    reason: Option<&str>,
    ts: &str,
) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO script_guard_state (domain, generation_id, mode, halted, halt_reason, updated_at) \
         VALUES (?1, ?2, 'rust', ?3, ?4, ?5) \
         ON CONFLICT (domain, generation_id) DO UPDATE SET \
            halted = excluded.halted, halt_reason = excluded.halt_reason, updated_at = excluded.updated_at",
        params![HALT_DOMAIN, HALT_GENERATION, i64::from(halted), reason, ts],
    )?;
    Ok(())
}

fn append_halt_event_tx(
    tx: &Connection,
    kind: ScriptGenerationEventKind,
    actor: &str,
    reason: &str,
    ts: &str,
) -> Result<(), StoreError> {
    append_generation_event_tx(
        tx,
        &ScriptGenerationEvent {
            kind,
            generation_id: HALT_EVENT_GENERATION.to_owned(),
            bundle_sha256: None,
            from_state: None,
            to_state: None,
            actor: actor.to_owned(),
            reason: reason.to_owned(),
            cancelled_in_flight: None,
        },
        ts,
    )
}

fn to_i64(v: u64) -> Result<i64, StoreError> {
    i64::try_from(v)
        .map_err(|_| StoreError::Invalid(format!("script guard count {v} out of range")))
}

fn to_u64(v: i64) -> Result<u64, StoreError> {
    u64::try_from(v).map_err(|_| StoreError::Invalid(format!("script guard count {v} is negative")))
}
