//! ADR 2026-11-02-ts-control-layer-boundary 付記 S5-2・S5-6・S5-7: script の自動の戻し・無効化・停止を受信箱に出す。
//!
//! `script_domain_events`・`script_generation_events` を永続の cursor（`script_inbox_cursors`）から読み、actor が
//! `system:script-startup`・`system:script-guard` の記録 1 件につき受信箱 `script_control` の項目を 1 件作る
//! （冪等 key は記録の表と event の id。再起動しても同じ event で 2 件作らない）。人の actor の記録では項目を作らず、
//! 後の人の操作で開いている項目を閉じる。判定は決定的（DB を読むだけ）で、LLM を呼ばない。
//!
//! generation の記録で項目にするのは、それ自体が知らせになる `script_generation_invalidated`（安全違反・起動時の
//! 無効化）と `script_guard_halted`（全体の停止）だけ。mode の CAS が同じ actor で書く `draining`・`retired` 等は
//! 同じ出来事の mode の変更の項目と重なるので作らない。
//!
//! 停止基準の監視（付記 S5-7、(f) の数値のまま）:
//! - 呼び出しごとの計数と (f) の 4 行の判定・`canary`・`ts` の `rust` への戻し・安全違反の `invalid` は、dispatcher が
//!   記録を追記する transaction の中で store（`script_compare_append_observed`・`script_compare_log_append_observed`
//!   → `task_core::script_guard::evaluate`）が行う。計数は表 `script_guard_state` にあり、再起動の後も残る。
//! - この module の見回り（[`sweep`]）は、それに加えて 2 つを行う。
//!   1. Rust の検証を通らずに副作用が実行された記録（[`unvalidated_effect`]）を `script_compare_log` に見つけたら、
//!      全領域を `rust` に戻して全体を止める（`script_guard_halt`。解くのは人だけ）。
//!   2. `compare`→`canary`・`canary`→`ts` へ進む条件（純粋判定 1000 件・CoS の経路 200 件等）を計数から判定して
//!      返し、log に出す。mode は書かない（自動で昇格する経路は無い。昇格は人の API だけ）。

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use task_core::model::Event;
use task_core::script_compare::{
    CompareOutcome, CosDigestAppliedBy, CosDigestCompareRecord, ScriptDomain,
    ScriptGenerationEvent, ScriptGenerationEventKind, ScriptMode, actor_may_enable_ts,
};
use task_core::script_guard::{ScriptGuardReadiness, readiness};
use task_core::script_inbox::{
    SCRIPT_GUARD_HALTED, SCRIPT_INBOX_ACTORS, ScriptInboxApplied, ScriptInboxBatch,
    ScriptInboxCursors, ScriptInboxItem, ScriptInboxOp, ScriptInboxResolver, ScriptInboxSource,
    ScriptInboxStore,
};
use task_core::store::{ScriptDomainEventRow, ScriptGenerationEventRow};
use task_core::{InstanceRole, SharedRole, SqliteStore, StoreError, TaskStore};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::stranded_sweep::{SingleFlight, periodic};

/// 取り込みの間隔（人に知らせる遅れの上限の目安）。
pub(crate) const INBOX_INTERVAL: Duration = Duration::from_secs(30);

/// 1 回の読みの行数（読み切るまで繰り返す）。
const PAGE: u32 = 500;

/// 記録の `reason` の先頭の code（`guard_mismatch: …` の `guard_mismatch`）。読めなければ `unknown`。
pub(crate) fn reason_code(reason: &str) -> String {
    let code: String = reason
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .collect();
    if code.is_empty() {
        "unknown".to_owned()
    } else {
        code
    }
}

fn is_system_inbox_actor(actor: &str) -> bool {
    SCRIPT_INBOX_ACTORS.contains(&actor)
}

fn snake<T: serde::Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(serde_json::Value::String(s)) => s,
        _ => "?".to_owned(),
    }
}

fn parse_ts(ts: &str) -> OffsetDateTime {
    OffsetDateTime::parse(ts, &Rfc3339).unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

/// 1 件の記録から当てる操作（無ければ `None`）。
fn domain_op(row: &ScriptDomainEventRow) -> Option<ScriptInboxOp> {
    let Event::ScriptDomainModeChanged {
        domain,
        from_mode,
        to_mode,
        from_generation,
        to_generation,
        actor,
        reason,
        ..
    } = &row.event
    else {
        return None;
    };
    if is_system_inbox_actor(actor) {
        return Some(ScriptInboxOp::Open(ScriptInboxItem {
            id: ScriptInboxSource::Domain.item_id(row.id),
            source: ScriptInboxSource::Domain,
            event_id: row.id,
            event_type: "script_domain_mode_changed".to_owned(),
            domain: Some(domain.as_str().to_owned()),
            generation_id: from_generation.clone().or_else(|| to_generation.clone()),
            change: Some(format!("{} → {}", from_mode.as_str(), to_mode.as_str())),
            actor: actor.clone(),
            reason: reason.clone(),
            code: reason_code(reason),
            created_at: parse_ts(&row.ts),
        }));
    }
    actor_may_enable_ts(actor).then(|| ScriptInboxOp::ResolveDomain {
        domain: domain.as_str().to_owned(),
        by: ScriptInboxResolver {
            source: ScriptInboxSource::Domain,
            event_id: row.id,
            at: parse_ts(&row.ts),
        },
    })
}

fn generation_op(row: &ScriptGenerationEventRow) -> Option<ScriptInboxOp> {
    let ScriptGenerationEvent {
        kind,
        generation_id,
        from_state,
        to_state,
        actor,
        reason,
        ..
    } = &row.event;
    if is_system_inbox_actor(actor) {
        if !matches!(
            kind,
            ScriptGenerationEventKind::ScriptGenerationInvalidated
                | ScriptGenerationEventKind::ScriptGuardHalted
        ) {
            return None;
        }
        let event_type = snake(kind);
        debug_assert!(
            *kind != ScriptGenerationEventKind::ScriptGuardHalted
                || event_type == SCRIPT_GUARD_HALTED
        );
        let change = match (from_state, to_state) {
            (Some(f), Some(t)) => Some(format!("{} → {}", f.as_str(), t.as_str())),
            (None, Some(t)) => Some(format!("→ {}", t.as_str())),
            _ => None,
        };
        return Some(ScriptInboxOp::Open(ScriptInboxItem {
            id: ScriptInboxSource::Generation.item_id(row.id),
            source: ScriptInboxSource::Generation,
            event_id: row.id,
            event_type,
            domain: None,
            generation_id: Some(generation_id.clone()),
            change,
            actor: actor.clone(),
            reason: reason.clone(),
            code: reason_code(reason),
            created_at: parse_ts(&row.ts),
        }));
    }
    // `activated`・`draining` は人の mode の変更（CAS）が同じ transaction で書く。閉じるのは mode の変更の記録に任せる。
    if !actor_may_enable_ts(actor)
        || matches!(
            kind,
            ScriptGenerationEventKind::ScriptGenerationActivated
                | ScriptGenerationEventKind::ScriptGenerationDraining
        )
    {
        return None;
    }
    let by = ScriptInboxResolver {
        source: ScriptInboxSource::Generation,
        event_id: row.id,
        at: parse_ts(&row.ts),
    };
    Some(match kind {
        ScriptGenerationEventKind::ScriptGuardCleared => ScriptInboxOp::ResolveHalt { by },
        _ => ScriptInboxOp::ResolveGeneration {
            generation_id: generation_id.clone(),
            by,
        },
    })
}

/// 読んだ記録から取り込みを組み立てる（純関数）。2 つの表の記録は時刻順（同時刻は mode の変更が先、id 順）に
/// 並べて当てる。cursor は読んだ最大の id（読んでいなければ前の値）。
pub(crate) fn plan(
    cursors: ScriptInboxCursors,
    domain_rows: &[ScriptDomainEventRow],
    generation_rows: &[ScriptGenerationEventRow],
) -> ScriptInboxBatch {
    let mut ordered: Vec<(OffsetDateTime, u8, u64, ScriptInboxOp)> = Vec::new();
    for row in domain_rows {
        if let Some(op) = domain_op(row) {
            ordered.push((parse_ts(&row.ts), 0, row.id, op));
        }
    }
    for row in generation_rows {
        if let Some(op) = generation_op(row) {
            ordered.push((parse_ts(&row.ts), 1, row.id, op));
        }
    }
    ordered.sort_by_key(|a| (a.0, a.1, a.2));
    ScriptInboxBatch {
        domain_cursor: domain_rows
            .iter()
            .map(|r| r.id)
            .max()
            .unwrap_or(cursors.domain)
            .max(cursors.domain),
        generation_cursor: generation_rows
            .iter()
            .map(|r| r.id)
            .max()
            .unwrap_or(cursors.generation)
            .max(cursors.generation),
        ops: ordered.into_iter().map(|(_, _, _, op)| op).collect(),
    }
}

/// cursor から読み切るまで取り込む（1 page ごとに 1 transaction）。
pub(crate) fn ingest<S: ScriptInboxStore + ?Sized>(
    store: &S,
) -> Result<ScriptInboxApplied, StoreError> {
    let mut total = ScriptInboxApplied::default();
    loop {
        let cursors = store.script_inbox_cursors()?;
        let domain_rows = store.script_inbox_domain_events_after(cursors.domain, PAGE)?;
        let generation_rows =
            store.script_inbox_generation_events_after(cursors.generation, PAGE)?;
        if domain_rows.is_empty() && generation_rows.is_empty() {
            return Ok(total);
        }
        let full = domain_rows.len() == PAGE as usize || generation_rows.len() == PAGE as usize;
        let applied = store.script_inbox_apply(&plan(cursors, &domain_rows, &generation_rows))?;
        total.opened += applied.opened;
        total.resolved += applied.resolved;
        if !full {
            return Ok(total);
        }
    }
}

/// Rust の検証を通らない副作用を表す reason の code（付記 S5-7。全体の停止の記録の先頭）。
pub(crate) const GUARD_UNVALIDATED_EFFECT: &str = "guard_unvalidated_effect";

/// `cos_digest` の記録 1 件が「Rust の検証を通らずに副作用が実行された」ことを示すなら、その理由の code。
///
/// 副作用を起こすのは Rust の apply だけで、TS の command を apply してよいのは、mode が `canary`・`ts` で、TS の
/// 出力が Rust の検証を通った（安全違反が無く、`ts_error`・`invalid_output`・`unavailable` でなく、fallback が無い）
/// ときだけである（付記 S4-14・S5-6）。`applied_by` が `ts` でそれに反する記録は検証の外の副作用とみなす。
/// 純粋判定の領域（`lane_rules`・`route_priority`）は副作用を持たないので、この検出の対象にならない。
pub(crate) fn unvalidated_effect(record: &CosDigestCompareRecord) -> Option<&'static str> {
    if record.applied_by != CosDigestAppliedBy::Ts {
        return None;
    }
    if !matches!(record.mode, ScriptMode::Canary | ScriptMode::Ts) {
        return Some("ts_applied_outside_ts_mode");
    }
    if !record.safety_violations.is_empty() {
        return Some("ts_applied_with_safety_violation");
    }
    if record.ts.is_none()
        || matches!(
            record.outcome,
            CompareOutcome::TsError | CompareOutcome::InvalidOutput | CompareOutcome::Unavailable
        )
    {
        return Some("ts_applied_without_valid_output");
    }
    if record.fallback.is_some() {
        return Some("ts_applied_with_fallback");
    }
    None
}

/// 進む条件の判定 1 件（領域・generation・今の mode ごと）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DomainReadiness {
    pub domain: ScriptDomain,
    pub generation_id: String,
    /// 今の mode（`compare` なら `canary` へ、`canary` なら `ts` へ進む条件）。
    pub mode: ScriptMode,
    pub readiness: ScriptGuardReadiness,
}

/// 見回り 1 回の結果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GuardSweep {
    /// この見回りで全体を止めたときの reason（止めなければ `None`）。
    pub halted: Option<String>,
    /// 止めたときに `rust` へ戻した領域。
    pub rolled_back: Vec<ScriptDomain>,
    /// `compare`・`canary` の領域の進む条件の判定（自動で昇格しない。結果を返すだけ）。
    pub readiness: Vec<DomainReadiness>,
}

/// 見回りの process の中の状態。`script_compare_log` の読み終えた id と、ready を log に出した組。
/// 再起動で失っても正しさは変わらない（停止は最後に人が解いた時刻より後の記録だけで判定し、判定は冪等）。
#[derive(Debug, Default)]
pub(crate) struct GuardState {
    log_cursor: u64,
    announced_ready: BTreeSet<(ScriptDomain, String, &'static str)>,
}

/// 最後に人が全体の停止を解いた時刻（無ければ `None`）。
fn last_guard_clear(store: &SqliteStore) -> Result<Option<OffsetDateTime>, StoreError> {
    let mut after = 0;
    let mut last = None;
    loop {
        let rows = store.script_generation_events_after(after, PAGE)?;
        let Some(tail) = rows.last() else {
            return Ok(last);
        };
        after = tail.id;
        for row in &rows {
            if row.event.kind == ScriptGenerationEventKind::ScriptGuardCleared {
                last = Some(parse_ts(&row.ts));
            }
        }
        if rows.len() < PAGE as usize {
            return Ok(last);
        }
    }
}

/// 見回り 1 回（付記 S5-7）。`now` は判定と記録の時計（試験では差し替える）。
pub(crate) fn sweep(
    store: &SqliteStore,
    state: &mut GuardState,
    now: OffsetDateTime,
) -> Result<GuardSweep, StoreError> {
    let mut out = GuardSweep::default();

    // 1. 検証の外の副作用 → 全領域を `rust` に戻して止める。
    let rows: Vec<_> = store
        .script_compare_log(None)?
        .into_iter()
        .filter(|row| row.id > state.log_cursor)
        .collect();
    let offending: Vec<_> = rows
        .iter()
        .filter_map(|row| unvalidated_effect(&row.record).map(|code| (row, code)))
        .collect();
    if !offending.is_empty() && store.script_guard_halted()?.is_none() {
        let cleared = last_guard_clear(store)?;
        let first = offending
            .iter()
            .find(|(row, _)| cleared.is_none_or(|at| parse_ts(&row.ts) > at));
        if let Some((row, code)) = first {
            let reason = format!(
                "{GUARD_UNVALIDATED_EFFECT}: {code}: domain {}, generation {}, run {}, pass {}, script_compare_log {}",
                row.domain.as_str(),
                row.record.generation,
                row.run_id,
                row.record.pass_id,
                row.id
            );
            let rolled_back = store
                .script_guard_halt(&reason, now)
                .map_err(|e| StoreError::Invalid(e.to_string()))?;
            out.halted = Some(reason);
            out.rolled_back = rolled_back;
        }
    }
    if let Some(last) = rows.last() {
        state.log_cursor = last.id;
    }

    // 2. 進む条件の判定（書かない）。
    for row in store.script_domain_modes()? {
        let Some(generation_id) = row.generation_id.clone() else {
            continue;
        };
        if !matches!(row.mode, ScriptMode::Compare | ScriptMode::Canary) {
            continue;
        }
        let counters = store
            .script_guard_counters(row.domain, &generation_id)?
            .filter(|(counted, _)| *counted == row.mode)
            .map(|(_, counters)| counters)
            .unwrap_or_default();
        out.readiness.push(DomainReadiness {
            domain: row.domain,
            generation_id,
            mode: row.mode,
            readiness: readiness(row.domain, row.mode, &counters),
        });
    }
    let ready: BTreeSet<_> = out
        .readiness
        .iter()
        .filter(|r| r.readiness.ready)
        .map(|r| (r.domain, r.generation_id.clone(), r.mode.as_str()))
        .collect();
    for r in out.readiness.iter().filter(|r| {
        r.readiness.ready
            && !state.announced_ready.contains(&(
                r.domain,
                r.generation_id.clone(),
                r.mode.as_str(),
            ))
    }) {
        tracing::info!(
            domain = r.domain.as_str(),
            generation = %r.generation_id,
            mode = r.mode.as_str(),
            readiness = %r.readiness,
            "script guard: promotion criteria met (a human promotes through the API; not promoted automatically)"
        );
    }
    state.announced_ready = ready;
    Ok(out)
}

/// active の instance で `INBOX_INTERVAL` ごとに、停止基準の見回り（`guard_store` があれば）と受信箱の取り込みを
/// この順で行う（起動直後に 1 回。起動時の戻しの記録もここで拾う。見回りが止めた記録も同じ回で項目になる）。
pub(crate) fn spawn(
    store: Arc<dyn TaskStore>,
    guard_store: Option<Arc<SqliteStore>>,
    role: SharedRole,
) -> tokio::task::JoinHandle<()> {
    let flight = Arc::new(SingleFlight::default());
    let state = Arc::new(Mutex::new(GuardState::default()));
    tokio::spawn(periodic(INBOX_INTERVAL, move || {
        if role.get() != InstanceRole::Active {
            return;
        }
        let Ok(guard) = flight.try_begin() else {
            return;
        };
        let store = Arc::clone(&store);
        let guard_store = guard_store.clone();
        let state = Arc::clone(&state);
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            if let Some(db) = guard_store {
                let mut state = match state.lock() {
                    Ok(state) => state,
                    Err(poisoned) => poisoned.into_inner(),
                };
                match sweep(&db, &mut state, OffsetDateTime::now_utc()) {
                    Ok(done) => {
                        if let Some(reason) = &done.halted {
                            tracing::error!(
                                %reason,
                                rolled_back = done.rolled_back.len(),
                                "script guard: effect without Rust validation; all domains rolled back to rust and halted"
                            );
                        }
                    }
                    Err(error) => tracing::warn!(%error, "script guard: sweep failed"),
                }
            }
            match ingest(store.as_ref()) {
                Ok(applied) if applied.opened + applied.resolved > 0 => tracing::info!(
                    opened = applied.opened,
                    resolved = applied.resolved,
                    "script guard: inbox items updated"
                ),
                Ok(_) => {}
                Err(error) => tracing::warn!(%error, "script guard: inbox ingest failed"),
            }
        });
    }))
}

#[cfg(test)]
#[path = "script_guard_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "script_guard_stop_tests.rs"]
mod stop_tests;
