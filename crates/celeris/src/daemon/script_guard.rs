//! ADR 2026-11-02-ts-control-layer-boundary 付記 S5-2・S5-6・S5-7: script の自動の戻し・無効化・停止を受信箱に出す。
//!
//! `script_domain_events`・`script_generation_events` を永続の cursor（`script_inbox_cursors`）から読み、actor が
//! `system:script-startup`・`system:script-guard` の記録 1 件につき受信箱 `script_control` の項目を 1 件作る
//! （冪等 key は記録の表と event の id。再起動しても同じ event で 2 件作らない）。人の actor の記録では項目を作らず、
//! 後の人の操作で開いている項目を閉じる。判定は決定的（DB を読むだけ）で、LLM を呼ばない。
//!
//! generation の記録で項目にするのは、それ自体が知らせになる `script_generation_invalidated`（安全違反・起動時の
//! 無効化）と `script_guard_halted`（全体の停止）だけ。mode の CAS が同じ actor で書く `draining`・`retired` 等は
//! 同じ出来事の mode の変更の項目と重なるので作らない。戻しの判定はここに無い（起動時の復元・canary の適用・
//! 停止基準の監視が記録を書く）。

use std::sync::Arc;
use std::time::Duration;

use task_core::model::Event;
use task_core::script_compare::{
    ScriptGenerationEvent, ScriptGenerationEventKind, actor_may_enable_ts,
};
use task_core::script_inbox::{
    SCRIPT_GUARD_HALTED, SCRIPT_INBOX_ACTORS, ScriptInboxApplied, ScriptInboxBatch,
    ScriptInboxCursors, ScriptInboxItem, ScriptInboxOp, ScriptInboxResolver, ScriptInboxSource,
    ScriptInboxStore,
};
use task_core::store::{ScriptDomainEventRow, ScriptGenerationEventRow};
use task_core::{InstanceRole, SharedRole, StoreError, TaskStore};
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

/// active の instance で `INBOX_INTERVAL` ごとに取り込む（起動直後に 1 回。起動時の戻しの記録もここで拾う）。
pub(crate) fn spawn(store: Arc<dyn TaskStore>, role: SharedRole) -> tokio::task::JoinHandle<()> {
    let flight = Arc::new(SingleFlight::default());
    tokio::spawn(periodic(INBOX_INTERVAL, move || {
        if role.get() != InstanceRole::Active {
            return;
        }
        let Ok(guard) = flight.try_begin() else {
            return;
        };
        let store = Arc::clone(&store);
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
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
