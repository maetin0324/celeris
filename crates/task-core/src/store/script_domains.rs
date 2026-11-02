//! ADR 2026-11-02-ts-control-layer-boundary 付記 S2-4・S2-5: 表 `script_generations`・`script_domain_modes`・
//! `script_domain_events`（migration 0103）と event `script_compare_recorded` の追記、
//! `cos_digest` の compare の記録 `script_compare_log`（migration 0104、付記 S4-5）。
//!
//! - mode の更新は `version` の CAS だけ。更新と `script_domain_mode_changed` の追記は 1 つの IMMEDIATE transaction。
//! - S2 では `canary`・`ts` への変更を store でも拒む（表を変えない）。
//! - `script_domain_events` は追記専用（UPDATE・DELETE は表の trigger が拒む。この module にも経路を作らない）。

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use time::OffsetDateTime;

use super::{SqliteStore, StoreError, format_rfc3339};
use crate::model::{Event, TaskId};
use crate::script_compare::{
    CosDigestCompareRecord, NewScriptGeneration, ScriptCompareRecord, ScriptDomain,
    ScriptDomainError, ScriptDomainModeRow, ScriptGeneration, ScriptGenerationState, ScriptMode,
    ScriptModeChange, ScriptModeChangeOutcome, actor_may_enable_ts,
};

/// `script_compare_log` の 1 行（付記 S4-5）。
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptCompareLogRow {
    pub id: u64,
    pub ts: String,
    pub domain: ScriptDomain,
    pub thread_id: String,
    pub run_id: String,
    pub record: CosDigestCompareRecord,
}

/// `script_domain_events` の 1 行。`event` は `Event::ScriptDomainModeChanged`。
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptDomainEventRow {
    pub id: u64,
    pub ts: String,
    pub domain: ScriptDomain,
    pub event: Event,
}

const MODE_COLUMNS: &str = "domain, mode, generation_id, version, updated_at, updated_by";
const GENERATION_COLUMNS: &str =
    "id, bundle_sha256, host_api, schema_sha256, manifest_json, state, created_at, updated_at";

impl SqliteStore {
    /// 全領域の mode（dispatcher が tick ごとに 1 回読む）。`domain` の順。
    pub fn script_domain_modes(&self) -> Result<Vec<ScriptDomainModeRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {MODE_COLUMNS} FROM script_domain_modes ORDER BY domain"
            ))?;
            let rows = stmt.query_map([], raw_mode_row)?;
            rows.map(|r| mode_row(r?)).collect()
        })
    }

    /// 1 領域の mode。未知の領域名は `UnknownDomain`。
    pub fn script_domain_mode(
        &self,
        domain: &str,
    ) -> Result<ScriptDomainModeRow, ScriptDomainError> {
        let domain = parse_domain(domain)?;
        let row = self.with_read_conn(|conn| read_mode_tx(conn, domain))?;
        row.ok_or_else(|| ScriptDomainError::UnknownDomain(domain.as_str().to_owned()))
    }

    /// 領域の active generation（mode が `rust` なら `None`）。
    pub fn script_domain_active_generation(
        &self,
        domain: &str,
    ) -> Result<Option<ScriptGeneration>, ScriptDomainError> {
        let row = self.script_domain_mode(domain)?;
        match row.generation_id {
            None => Ok(None),
            Some(id) => Ok(self.script_generation_get(&id)?),
        }
    }

    /// generation の行を作る。同じ `bundle_sha256` の行が在ればそれを返す（行を足さない、S2-4）。
    pub fn script_generation_record(
        &self,
        new: &NewScriptGeneration,
        now: OffsetDateTime,
    ) -> Result<ScriptGeneration, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = read_generation_by_sha_tx(&tx, &new.bundle_sha256)? {
            tx.commit()?;
            return Ok(existing);
        }
        let ts = format_rfc3339(now)?;
        let id = ulid::Ulid::new().to_string();
        tx.execute(
            &format!(
                "INSERT INTO script_generations ({GENERATION_COLUMNS}) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)"
            ),
            params![
                id,
                new.bundle_sha256,
                new.host_api,
                new.schema_sha256,
                new.manifest_json,
                new.state.as_str(),
                ts,
            ],
        )?;
        let row = read_generation_tx(&tx, &id)?
            .ok_or_else(|| StoreError::Invalid(format!("script generation {id} vanished")))?;
        tx.commit()?;
        Ok(row)
    }

    pub fn script_generation_get(&self, id: &str) -> Result<Option<ScriptGeneration>, StoreError> {
        self.with_read_conn(|conn| read_generation_tx(conn, id))
    }

    pub fn script_generation_by_sha256(
        &self,
        bundle_sha256: &str,
    ) -> Result<Option<ScriptGeneration>, StoreError> {
        self.with_read_conn(|conn| read_generation_by_sha_tx(conn, bundle_sha256))
    }

    /// mode を変える（S2-4 の CAS・S2-5）。表の更新と `script_domain_events` への追記は同じ transaction。
    ///
    /// - 未知の領域 → `UnknownDomain`。`canary`、`cos_digest` 以外の `ts` → `ModeNotEnabled`（表を変えない）。
    /// - `ts` は actor が `human:*` でなければ `TsRequiresHuman`、元が同じ generation の `compare` でなければ
    ///   `TsRequiresCompare`（S4-14）。戻し（`ts→rust`・`ts→compare`）は誰の actor でも受ける。
    /// - 同じ mode・同じ generation → `Unchanged`（表も記録も変えない）。
    /// - `version` が `expected_version` と違う（他の更新が先に入った）→ `VersionConflict`。
    /// - `compare`・`ts` は `loaded` で manifest の `domains` にその領域がある generation が要る。
    pub fn script_domain_set_mode(
        &self,
        change: &ScriptModeChange,
        now: OffsetDateTime,
    ) -> Result<ScriptModeChangeOutcome, ScriptDomainError> {
        let domain = parse_domain(&change.domain)?;
        if !change.to_mode.accepted_for(domain) {
            return Err(ScriptDomainError::ModeNotEnabled {
                domain,
                mode: change.to_mode,
            });
        }
        match (change.to_mode.needs_generation(), &change.to_generation) {
            (true, None) => return Err(ScriptDomainError::GenerationRequired),
            (false, Some(_)) => return Err(ScriptDomainError::GenerationNotAllowed),
            _ => {}
        }
        if change.to_mode == ScriptMode::Ts && !actor_may_enable_ts(&change.actor) {
            return Err(ScriptDomainError::TsRequiresHuman(change.actor.clone()));
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = read_mode_tx(&tx, domain)?
            .ok_or_else(|| ScriptDomainError::UnknownDomain(domain.as_str().to_owned()))?;
        if current.mode == change.to_mode && current.generation_id == change.to_generation {
            tx.commit()?;
            return Ok(ScriptModeChangeOutcome::Unchanged { row: current });
        }
        if current.version != change.expected_version {
            return Err(ScriptDomainError::VersionConflict {
                domain: domain.as_str().to_owned(),
                expected: change.expected_version,
                found: current.version,
            });
        }
        // S4-14: `→ts` の元は同じ generation の `compare` だけ（`rust` からの直行・generation の同時変更は拒む）。
        if change.to_mode == ScriptMode::Ts
            && (current.mode != ScriptMode::Compare
                || current.generation_id != change.to_generation)
        {
            return Err(ScriptDomainError::TsRequiresCompare {
                current: current.mode,
            });
        }
        if let Some(id) = &change.to_generation {
            let generation = read_generation_tx(&tx, id)?
                .ok_or_else(|| ScriptDomainError::GenerationNotFound(id.clone()))?;
            if generation.state != ScriptGenerationState::Loaded {
                return Err(ScriptDomainError::GenerationNotLoaded(id.clone()));
            }
            if !generation.declares_domain(domain) {
                return Err(ScriptDomainError::DomainNotDeclared {
                    domain: domain.as_str().to_owned(),
                    generation: id.clone(),
                });
            }
        }
        let ts = format_rfc3339(now)?;
        let new_version = change.expected_version + 1;
        let updated = tx.execute(
            "UPDATE script_domain_modes \
             SET mode = ?1, generation_id = ?2, version = version + 1, updated_at = ?3, updated_by = ?4 \
             WHERE domain = ?5 AND version = ?6",
            params![
                change.to_mode.as_str(),
                change.to_generation,
                ts,
                change.actor,
                domain.as_str(),
                to_i64(change.expected_version)?,
            ],
        )?;
        if updated != 1 {
            let found = read_mode_tx(&tx, domain)?.map_or(0, |r| r.version);
            return Err(ScriptDomainError::VersionConflict {
                domain: domain.as_str().to_owned(),
                expected: change.expected_version,
                found,
            });
        }
        let event = Event::ScriptDomainModeChanged {
            domain,
            from_mode: current.mode,
            to_mode: change.to_mode,
            from_generation: current.generation_id.clone(),
            to_generation: change.to_generation.clone(),
            version: new_version,
            actor: change.actor.clone(),
            reason: change.reason.clone(),
        };
        tx.execute(
            "INSERT INTO script_domain_events (ts, domain, json) VALUES (?1, ?2, ?3)",
            params![ts, domain.as_str(), serde_json::to_string(&event)?],
        )?;
        let event_id = u64::try_from(tx.last_insert_rowid())
            .map_err(|_| StoreError::Invalid("negative script_domain_events id".into()))?;
        let row = read_mode_tx(&tx, domain)?
            .ok_or_else(|| ScriptDomainError::UnknownDomain(domain.as_str().to_owned()))?;
        tx.commit()?;
        Ok(ScriptModeChangeOutcome::Changed { row, event_id })
    }

    /// `script_domain_events` を新しい順に `limit` 行（`domain` を渡せばその領域だけ）。
    pub fn script_domain_events(
        &self,
        domain: Option<ScriptDomain>,
        limit: u32,
    ) -> Result<Vec<ScriptDomainEventRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, ts, domain, json FROM script_domain_events \
                 WHERE ?1 IS NULL OR domain = ?1 ORDER BY id DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![domain.map(|d| d.as_str()), limit], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            rows.map(|r| {
                let (id, ts, domain, json) = r?;
                Ok(ScriptDomainEventRow {
                    id: u64::try_from(id).map_err(|_| {
                        StoreError::Invalid(format!("script_domain_events id {id}"))
                    })?,
                    ts,
                    domain: ScriptDomain::parse(&domain).ok_or_else(|| {
                        StoreError::Invalid(format!("unknown script domain `{domain}`"))
                    })?,
                    event: serde_json::from_str(&json)?,
                })
            })
            .collect()
        })
    }

    /// `script_compare_recorded` を task の events に追記する（S2-4）。同じ task・同じ `domain` の直前の
    /// 記録と `generation`・`input_digest`・`outcome` が同じなら追記せず `Ok(false)`。
    pub fn script_compare_append(
        &self,
        task_id: TaskId,
        record: &ScriptCompareRecord,
        now: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let last: Option<String> = tx
            .query_row(
                "SELECT json FROM events WHERE task_id = ?1 \
                 AND json_extract(json, '$.type') = 'script_compare_recorded' \
                 AND json_extract(json, '$.domain') = ?2 \
                 ORDER BY seq DESC LIMIT 1",
                params![task_id.to_string(), record.domain.as_str()],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(json) = last
            && let Event::ScriptCompareRecorded { record: prev } = serde_json::from_str(&json)?
            && prev.generation == record.generation
            && prev.input_digest == record.input_digest
            && prev.outcome == record.outcome
        {
            tx.commit()?;
            return Ok(false);
        }
        let event = Event::ScriptCompareRecorded {
            record: Box::new(record.clone()),
        };
        Self::append_event_tx_at(&tx, task_id, &event, now)?;
        tx.commit()?;
        Ok(true)
    }
}

impl SqliteStore {
    /// 付記 S4-5: `cos_digest` の compare の記録を `script_compare_log` に追記する。同じ `thread_id`・`run_id`・
    /// `domain` に `generation`・`input_digest`・`outcome` が同じ行が既にあれば追記せず `Ok(false)`。
    pub fn script_compare_log_append(
        &self,
        record: &CosDigestCompareRecord,
        now: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let outcome = serde_json::to_value(record.outcome)?;
        let outcome = outcome.as_str().unwrap_or_default().to_owned();
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists = tx
            .query_row(
                "SELECT 1 FROM script_compare_log \
                 WHERE thread_id = ?1 AND run_id = ?2 AND domain = ?3 \
                 AND json_extract(json, '$.generation') = ?4 \
                 AND json_extract(json, '$.input_digest') = ?5 \
                 AND json_extract(json, '$.outcome') = ?6 LIMIT 1",
                params![
                    record.thread_id,
                    record.run_id,
                    record.domain.as_str(),
                    record.generation,
                    record.input_digest,
                    outcome,
                ],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if exists {
            tx.commit()?;
            return Ok(false);
        }
        tx.execute(
            "INSERT INTO script_compare_log (ts, domain, thread_id, run_id, json) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                format_rfc3339(now)?,
                record.domain.as_str(),
                record.thread_id,
                record.run_id,
                serde_json::to_string(record)?,
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// `script_compare_log` を古い順に読む（`run_id` を渡せばその run だけ）。
    pub fn script_compare_log(
        &self,
        run_id: Option<&str>,
    ) -> Result<Vec<ScriptCompareLogRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, ts, domain, thread_id, run_id, json FROM script_compare_log \
                 WHERE ?1 IS NULL OR run_id = ?1 ORDER BY id",
            )?;
            let rows = stmt.query_map(params![run_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })?;
            rows.map(|r| {
                let (id, ts, domain, thread_id, run_id, json) = r?;
                Ok(ScriptCompareLogRow {
                    id: u64::try_from(id)
                        .map_err(|_| StoreError::Invalid(format!("script_compare_log id {id}")))?,
                    ts,
                    domain: parse_domain(&domain).map_err(|_| {
                        StoreError::Invalid(format!("unknown script domain `{domain}`"))
                    })?,
                    thread_id,
                    run_id,
                    record: serde_json::from_str(&json)?,
                })
            })
            .collect()
        })
    }
}

fn parse_domain(name: &str) -> Result<ScriptDomain, ScriptDomainError> {
    ScriptDomain::parse(name).ok_or_else(|| ScriptDomainError::UnknownDomain(name.to_owned()))
}

fn to_i64(v: u64) -> Result<i64, StoreError> {
    i64::try_from(v).map_err(|_| StoreError::Invalid(format!("version {v} out of range")))
}

type RawModeRow = (String, String, Option<String>, i64, String, String);

fn raw_mode_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawModeRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
    ))
}

fn mode_row(raw: RawModeRow) -> Result<ScriptDomainModeRow, StoreError> {
    let (domain, mode, generation_id, version, updated_at, updated_by) = raw;
    Ok(ScriptDomainModeRow {
        domain: ScriptDomain::parse(&domain)
            .ok_or_else(|| StoreError::Invalid(format!("unknown script domain `{domain}`")))?,
        mode: ScriptMode::parse(&mode)
            .ok_or_else(|| StoreError::Invalid(format!("unknown script mode `{mode}`")))?,
        generation_id,
        version: u64::try_from(version).map_err(|_| {
            StoreError::Invalid(format!("negative script domain version {version}"))
        })?,
        updated_at,
        updated_by,
    })
}

fn read_mode_tx(
    conn: &Connection,
    domain: ScriptDomain,
) -> Result<Option<ScriptDomainModeRow>, StoreError> {
    let raw = conn
        .query_row(
            &format!("SELECT {MODE_COLUMNS} FROM script_domain_modes WHERE domain = ?1"),
            params![domain.as_str()],
            raw_mode_row,
        )
        .optional()?;
    raw.map(mode_row).transpose()
}

type RawGenerationRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
);

fn raw_generation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawGenerationRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
    ))
}

fn generation_row(raw: RawGenerationRow) -> Result<ScriptGeneration, StoreError> {
    let (id, bundle_sha256, host_api, schema_sha256, manifest_json, state, created_at, updated_at) =
        raw;
    Ok(ScriptGeneration {
        id,
        bundle_sha256,
        host_api,
        schema_sha256,
        manifest_json,
        state: ScriptGenerationState::parse(&state).ok_or_else(|| {
            StoreError::Invalid(format!("unknown script generation state `{state}`"))
        })?,
        created_at,
        updated_at,
    })
}

fn read_generation_tx(conn: &Connection, id: &str) -> Result<Option<ScriptGeneration>, StoreError> {
    let raw = conn
        .query_row(
            &format!("SELECT {GENERATION_COLUMNS} FROM script_generations WHERE id = ?1"),
            params![id],
            raw_generation_row,
        )
        .optional()?;
    raw.map(generation_row).transpose()
}

fn read_generation_by_sha_tx(
    conn: &Connection,
    bundle_sha256: &str,
) -> Result<Option<ScriptGeneration>, StoreError> {
    let raw = conn
        .query_row(
            &format!(
                "SELECT {GENERATION_COLUMNS} FROM script_generations WHERE bundle_sha256 = ?1"
            ),
            params![bundle_sha256],
            raw_generation_row,
        )
        .optional()?;
    raw.map(generation_row).transpose()
}

#[cfg(test)]
#[path = "script_domains_tests.rs"]
mod tests;
