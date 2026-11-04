//! ADR 2026-11-02-ts-control-layer-boundary 付記 S2-4・S2-5: 表 `script_generations`・`script_domain_modes`・
//! `script_domain_events`（migration 0103）と event `script_compare_recorded` の追記、
//! `cos_digest` の compare の記録 `script_compare_log`（migration 0104、付記 S4-5）。
//!
//! 付記 S5（migration 0105）: generation の状態遷移と `script_generation_events`、`canary_bps`。
//!
//! - mode の更新は `version` の CAS だけ。更新と `script_domain_mode_changed` の追記、generation の状態の移動は
//!   1 つの IMMEDIATE transaction。
//! - generation の状態を書くのは `transition_tx` の 1 か所だけで、毎回 `script_generation_events` に追記する（S5-1）。
//! - `script_domain_events`・`script_generation_events` は追記専用（UPDATE・DELETE は表の trigger が拒む。
//!   この module にも経路を作らない）。

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use time::OffsetDateTime;

use super::{SqliteStore, StoreError, format_rfc3339};
use crate::model::{Event, TaskId};
use crate::script_compare::{
    CosDigestCompareRecord, NewScriptGeneration, SCRIPT_GENERATIONS_LIVE_MAX, ScriptCompareRecord,
    ScriptDomain, ScriptDomainError, ScriptDomainModeRow, ScriptGeneration, ScriptGenerationEvent,
    ScriptGenerationEventKind, ScriptGenerationState, ScriptGenerationTransition, ScriptMode,
    ScriptModeChange, ScriptModeChangeOutcome, actor_may_enable_ts, checkpoint_versions_cover,
};
use crate::script_guard::readiness;

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
    /// 変更後の `canary_bps`（`json` の `canary_bps`。`canary` 以外と migration 0105 より前の行は `None`）。
    pub canary_bps: Option<u32>,
}

/// `script_generation_events` の 1 行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptGenerationEventRow {
    pub id: u64,
    pub ts: String,
    pub generation_id: String,
    pub event: ScriptGenerationEvent,
}

/// 登録の actor（daemon が bundle を検証して書く）。
pub const SCRIPT_RUNTIME_ACTOR: &str = "system:script-runtime";

const MODE_COLUMNS: &str =
    "domain, mode, generation_id, version, canary_bps, updated_at, updated_by";
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

    /// 全領域の mode と、その行が指す generation（起動時の復元が読む。S5-2）。`domain` の順。
    pub fn script_domain_active_generations(
        &self,
    ) -> Result<Vec<(ScriptDomainModeRow, Option<ScriptGeneration>)>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {MODE_COLUMNS} FROM script_domain_modes ORDER BY domain"
            ))?;
            let rows = stmt
                .query_map([], raw_mode_row)?
                .collect::<Result<Vec<_>, _>>()?;
            rows.into_iter()
                .map(|raw| {
                    let row = mode_row(raw)?;
                    let generation = match row.generation_id.as_deref() {
                        Some(id) => read_generation_tx(conn, id)?,
                        None => None,
                    };
                    Ok((row, generation))
                })
                .collect()
        })
    }

    /// 検証に通った generation を登録する（S5-1 の 4・6）。
    ///
    /// - `new.state` は `loaded` だけを受ける。
    /// - 同じ hash が `loaded`・`active`・`draining` → 何も変えず今の行を返す。
    /// - 同じ hash が `retired` → `loaded` に戻す。`invalid` → `GenerationInvalidated`。
    /// - 生きた generation が上限（2 つ）に達していれば `GenerationLimit`（表も記録も変えない）。
    pub fn script_generation_record(
        &self,
        new: &NewScriptGeneration,
        now: OffsetDateTime,
    ) -> Result<ScriptGeneration, ScriptDomainError> {
        if new.state != ScriptGenerationState::Loaded {
            return Err(StoreError::Invalid(format!(
                "a new script generation must be loaded, got `{}`",
                new.state.as_str()
            ))
            .into());
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ts = format_rfc3339(now)?;
        let row = match read_generation_by_sha_tx(&tx, &new.bundle_sha256)? {
            Some(existing) if existing.state.is_live() => existing,
            Some(existing) if existing.state == ScriptGenerationState::Invalid => {
                return Err(ScriptDomainError::GenerationInvalidated(existing.id));
            }
            Some(existing) => {
                check_live_limit_tx(&tx)?;
                transition_tx(
                    &tx,
                    &existing,
                    ScriptGenerationState::Loaded,
                    SCRIPT_RUNTIME_ACTOR,
                    "registered again",
                    None,
                    &ts,
                )?
            }
            None => {
                check_live_limit_tx(&tx)?;
                insert_generation_tx(&tx, new, ScriptGenerationState::Loaded, "", &ts)?
            }
        };
        tx.commit()?;
        Ok(row)
    }

    /// 検証に落ちた bundle を `invalid` で書く（S5-1 の 4）。同じ hash が `invalid` なら今の行を返し、`retired` なら
    /// `invalid` に移す。生きた行は検証し直さない（S5-1 の 6）ので `IllegalTransition`。上限には数えない。
    pub fn script_generation_record_invalid(
        &self,
        new: &NewScriptGeneration,
        reason: &str,
        now: OffsetDateTime,
    ) -> Result<ScriptGeneration, ScriptDomainError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ts = format_rfc3339(now)?;
        let row = match read_generation_by_sha_tx(&tx, &new.bundle_sha256)? {
            Some(existing) if existing.state == ScriptGenerationState::Invalid => existing,
            Some(existing) => transition_tx(
                &tx,
                &existing,
                ScriptGenerationState::Invalid,
                SCRIPT_RUNTIME_ACTOR,
                reason,
                None,
                &ts,
            )?,
            None => insert_generation_tx(&tx, new, ScriptGenerationState::Invalid, reason, &ts)?,
        };
        tx.commit()?;
        Ok(row)
    }

    /// generation の状態を変える（S5-1）。受けるのは次だけで、どの拒否でも表も記録も変えない。
    ///
    /// - `retired`: `loaded`（退役）・`draining`（drain の終わり）から。`active` は `GenerationActive`。
    /// - `invalid`: 指している全領域を同じ transaction で `rust` に戻す（S5-7 の安全違反）。
    /// - `loaded` は登録、`active`・`draining` は mode の CAS だけが書くので `IllegalTransition`。
    pub fn script_generation_transition(
        &self,
        change: &ScriptGenerationTransition,
        now: OffsetDateTime,
    ) -> Result<ScriptGeneration, ScriptDomainError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = read_generation_tx(&tx, &change.id)?
            .ok_or_else(|| ScriptDomainError::GenerationNotFound(change.id.clone()))?;
        let ts = format_rfc3339(now)?;
        let row = match change.to {
            ScriptGenerationState::Retired if current.state == ScriptGenerationState::Active => {
                return Err(ScriptDomainError::GenerationActive(change.id.clone()));
            }
            ScriptGenerationState::Retired => transition_tx(
                &tx,
                &current,
                change.to,
                &change.actor,
                &change.reason,
                change.cancelled_in_flight,
                &ts,
            )?,
            ScriptGenerationState::Invalid => {
                invalidate_tx(&tx, &current, &change.actor, &change.reason, &ts)?.0
            }
            to => {
                return Err(ScriptDomainError::IllegalTransition {
                    id: change.id.clone(),
                    from: current.state,
                    to,
                });
            }
        };
        tx.commit()?;
        Ok(row)
    }

    /// 全 generation（古い順。`GET /script/generations` が読む）。
    pub fn script_generations(&self) -> Result<Vec<ScriptGeneration>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {GENERATION_COLUMNS} FROM script_generations ORDER BY created_at, id"
            ))?;
            let rows = stmt.query_map([], raw_generation_row)?;
            rows.map(|r| generation_row(r?)).collect()
        })
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

    /// S5-5: 領域に保存済みの checkpoint の版の集合（dispatcher が pass の前に manifest と照合する）。
    pub fn script_checkpoint_versions_stored(&self, _domain: ScriptDomain) -> Vec<u32> {
        stored_checkpoint_versions()
    }

    /// mode を変える（S2-4 の CAS・S5-4）。`canary` は比率の要る [`Self::script_domain_set_canary`] で変える。
    pub fn script_domain_set_mode(
        &self,
        change: &ScriptModeChange,
        now: OffsetDateTime,
    ) -> Result<ScriptModeChangeOutcome, ScriptDomainError> {
        self.set_mode_inner(change, None, &stored_checkpoint_versions(), now)
    }

    /// mode を `canary` にする、または `canary` の `canary_bps` を変える（S5-3・S5-4）。
    pub fn script_domain_set_canary(
        &self,
        change: &ScriptModeChange,
        canary_bps: u32,
        now: OffsetDateTime,
    ) -> Result<ScriptModeChangeOutcome, ScriptDomainError> {
        self.set_mode_inner(change, Some(canary_bps), &stored_checkpoint_versions(), now)
    }

    /// CAS の本体。どの拒否でも表も記録も変えない（transaction を commit しない）。
    ///
    /// - 未知の領域 → `UnknownDomain`。`canary` の `canary_bps` は 1〜9999（`canary` 以外は渡せない）。
    /// - `ts`・`canary` は actor が `human:*` でなければ `TsRequiresHuman`・`CanaryRequiresHuman`。
    ///   戻し（`rust`・`compare`）は誰の actor でも受ける。
    /// - 同じ mode・同じ generation・同じ `canary_bps` → `Unchanged`（表も記録も変えない）。
    /// - 全体の停止の間は `rust` 以外を `GuardHalted` で拒む。
    /// - `version` が `expected_version` と違う（他の更新が先に入った）→ `VersionConflict`。
    /// - `canary` の元は同じ generation の `compare`・`canary`、`ts` の元は同じ generation の `canary`
    ///   （S4 の `cos_digest` の `compare→ts` は S5-4 で消した）。違えば `RequiresPreviousStage`。
    /// - 指す generation は `loaded`・`active` で manifest の `domains` にその領域があり、別の generation へ移るときは
    ///   `checkpoint_versions` が保存済みの版を全部含む（S5-5）。
    /// - `compare→canary`・`canary→ts` は計数が (f) の条件を満たす（`CanaryNotReady`）。
    pub(super) fn set_mode_inner(
        &self,
        change: &ScriptModeChange,
        canary_bps: Option<u32>,
        stored_checkpoints: &[u32],
        now: OffsetDateTime,
    ) -> Result<ScriptModeChangeOutcome, ScriptDomainError> {
        let domain = parse_domain(&change.domain)?;
        let bps_ok = match (change.to_mode, canary_bps) {
            (ScriptMode::Canary, Some(bps)) => (1..=9999).contains(&bps),
            (ScriptMode::Canary, None) => false,
            (_, bps) => bps.is_none(),
        };
        if !bps_ok {
            return Err(ScriptDomainError::CanaryBpsInvalid(canary_bps));
        }
        match (change.to_mode.needs_generation(), &change.to_generation) {
            (true, None) => return Err(ScriptDomainError::GenerationRequired),
            (false, Some(_)) => return Err(ScriptDomainError::GenerationNotAllowed),
            _ => {}
        }
        if !actor_may_enable_ts(&change.actor) {
            match change.to_mode {
                ScriptMode::Ts => {
                    return Err(ScriptDomainError::TsRequiresHuman(change.actor.clone()));
                }
                ScriptMode::Canary => {
                    return Err(ScriptDomainError::CanaryRequiresHuman(change.actor.clone()));
                }
                ScriptMode::Rust | ScriptMode::Compare => {}
            }
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = read_mode_tx(&tx, domain)?
            .ok_or_else(|| ScriptDomainError::UnknownDomain(domain.as_str().to_owned()))?;
        if current.mode == change.to_mode
            && current.generation_id == change.to_generation
            && current.canary_bps == canary_bps
        {
            tx.commit()?;
            return Ok(ScriptModeChangeOutcome::Unchanged { row: current });
        }
        if change.to_mode != ScriptMode::Rust
            && let Some(reason) = super::script_guard::read_halted_tx(&tx)?
        {
            return Err(ScriptDomainError::GuardHalted(reason));
        }
        if current.version != change.expected_version {
            return Err(ScriptDomainError::VersionConflict {
                domain: domain.as_str().to_owned(),
                expected: change.expected_version,
                found: current.version,
            });
        }
        let same_generation = current.generation_id == change.to_generation;
        let stage_ok = match change.to_mode {
            ScriptMode::Rust | ScriptMode::Compare => true,
            ScriptMode::Canary => {
                same_generation && matches!(current.mode, ScriptMode::Compare | ScriptMode::Canary)
            }
            ScriptMode::Ts => same_generation && current.mode == ScriptMode::Canary,
        };
        if !stage_ok {
            return Err(ScriptDomainError::RequiresPreviousStage {
                to: change.to_mode,
                current: current.mode,
            });
        }
        if let Some(id) = &change.to_generation {
            let generation = read_generation_tx(&tx, id)?
                .ok_or_else(|| ScriptDomainError::GenerationNotFound(id.clone()))?;
            if !matches!(
                generation.state,
                ScriptGenerationState::Loaded | ScriptGenerationState::Active
            ) {
                return Err(ScriptDomainError::GenerationNotLoaded(id.clone()));
            }
            if !generation.declares_domain(domain) {
                return Err(ScriptDomainError::DomainNotDeclared {
                    domain: domain.as_str().to_owned(),
                    generation: id.clone(),
                });
            }
            if !same_generation
                && !checkpoint_versions_cover(&generation.checkpoint_versions(), stored_checkpoints)
            {
                return Err(ScriptDomainError::CheckpointIncompatible {
                    generation: id.clone(),
                    stored: stored_checkpoints.to_vec(),
                });
            }
            let promotes = matches!(
                (current.mode, change.to_mode),
                (ScriptMode::Compare, ScriptMode::Canary) | (ScriptMode::Canary, ScriptMode::Ts)
            );
            if promotes {
                let counters =
                    super::script_guard::counters_for_mode_tx(&tx, domain, id, current.mode)?;
                let detail = readiness(
                    domain,
                    current.mode,
                    &counters,
                    super::script_exhaustive::standing_tx(
                        &tx,
                        domain,
                        id,
                        super::script_exhaustive::current_reference_sha256()?,
                    )?,
                );
                if !detail.ready {
                    return Err(ScriptDomainError::CanaryNotReady {
                        domain: domain.as_str().to_owned(),
                        to: change.to_mode,
                        detail,
                    });
                }
            }
        }
        let ts = format_rfc3339(now)?;
        let event_id = write_mode_tx(
            &tx,
            &current,
            change.to_mode,
            change.to_generation.as_deref(),
            canary_bps,
            &change.actor,
            &change.reason,
            &ts,
        )?;
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
            let rows =
                stmt.query_map(params![domain.map(|d| d.as_str()), limit], raw_domain_event)?;
            rows.map(|r| domain_event_row(r?)).collect()
        })
    }

    /// `id` が `after` より大きい `script_domain_events` を古い順に `limit` 行（`script-guard` の cursor。S5-7）。
    pub fn script_domain_events_after(
        &self,
        after: u64,
        limit: u32,
    ) -> Result<Vec<ScriptDomainEventRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, ts, domain, json FROM script_domain_events \
                 WHERE id > ?1 ORDER BY id LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![to_i64(after)?, limit], raw_domain_event)?;
            rows.map(|r| domain_event_row(r?)).collect()
        })
    }

    /// `script_generation_events` を古い順に `limit` 行（`id` が `after` より大きいもの。`script-guard` の cursor）。
    pub fn script_generation_events_after(
        &self,
        after: u64,
        limit: u32,
    ) -> Result<Vec<ScriptGenerationEventRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, ts, generation_id, json FROM script_generation_events \
                 WHERE id > ?1 ORDER BY id LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![to_i64(after)?, limit], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            rows.map(|r| {
                let (id, ts, generation_id, json) = r?;
                Ok(ScriptGenerationEventRow {
                    id: u64::try_from(id).map_err(|_| {
                        StoreError::Invalid(format!("script_generation_events id {id}"))
                    })?,
                    ts,
                    generation_id,
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
        let appended = Self::script_compare_append_tx(&tx, task_id, record, now)?;
        tx.commit()?;
        Ok(appended)
    }

    pub(super) fn script_compare_append_tx(
        tx: &rusqlite::Transaction<'_>,
        task_id: TaskId,
        record: &ScriptCompareRecord,
        now: OffsetDateTime,
    ) -> Result<bool, StoreError> {
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
            return Ok(false);
        }
        let event = Event::ScriptCompareRecorded {
            record: Box::new(record.clone()),
        };
        Self::append_event_tx_at(tx, task_id, &event, now)?;
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
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let appended = Self::script_compare_log_append_tx(&tx, record, now)?;
        tx.commit()?;
        Ok(appended)
    }

    pub(super) fn script_compare_log_append_tx(
        tx: &Connection,
        record: &CosDigestCompareRecord,
        now: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let outcome = serde_json::to_value(record.outcome)?;
        let outcome = outcome.as_str().unwrap_or_default().to_owned();
        let applied_by = record.applied_by.as_str();
        let exists = tx
            .query_row(
                "SELECT 1 FROM script_compare_log \
                 WHERE thread_id = ?1 AND run_id = ?2 AND domain = ?3 \
                 AND json_extract(json, '$.generation') = ?4 \
                 AND json_extract(json, '$.input_digest') = ?5 \
                 AND json_extract(json, '$.outcome') = ?6 \
                 AND json_extract(json, '$.applied_by') = ?7 LIMIT 1",
                params![
                    record.thread_id,
                    record.run_id,
                    record.domain.as_str(),
                    record.generation,
                    record.input_digest,
                    outcome,
                    applied_by,
                ],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if exists {
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
        Ok(true)
    }

    /// `script_compare_log` を古い順に読む（`run_id` を渡せばその run だけ）。
    pub fn script_compare_log(
        &self,
        run_id: Option<&str>,
    ) -> Result<Vec<ScriptCompareLogRow>, StoreError> {
        self.script_compare_log_query(
            "SELECT id, ts, domain, thread_id, run_id, json FROM script_compare_log \
             WHERE ?1 IS NULL OR run_id = ?1 ORDER BY id",
            params![run_id],
        )
    }

    /// P1-C（S5 未解決 6）: `script_compare_log` の `id > after` の行を古い順に最大 `limit` 行読む（主キーの範囲で読む。
    /// script-guard の見回りが読み終えた id から差分だけを読む）。
    pub fn script_compare_log_after(
        &self,
        after: u64,
        limit: u32,
    ) -> Result<Vec<ScriptCompareLogRow>, StoreError> {
        self.script_compare_log_query(
            "SELECT id, ts, domain, thread_id, run_id, json FROM script_compare_log \
             WHERE id > ?1 ORDER BY id LIMIT ?2",
            params![to_i64(after)?, i64::from(limit)],
        )
    }

    fn script_compare_log_query(
        &self,
        sql: &str,
        args: impl rusqlite::Params,
    ) -> Result<Vec<ScriptCompareLogRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map(args, |row| {
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

type RawDomainEvent = (i64, String, String, String);

fn raw_domain_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawDomainEvent> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
}

fn domain_event_row(raw: RawDomainEvent) -> Result<ScriptDomainEventRow, StoreError> {
    let (id, ts, domain, json) = raw;
    let value: serde_json::Value = serde_json::from_str(&json)?;
    Ok(ScriptDomainEventRow {
        id: u64::try_from(id)
            .map_err(|_| StoreError::Invalid(format!("script_domain_events id {id}")))?,
        ts,
        domain: ScriptDomain::parse(&domain)
            .ok_or_else(|| StoreError::Invalid(format!("unknown script domain `{domain}`")))?,
        canary_bps: value
            .get("canary_bps")
            .and_then(serde_json::Value::as_u64)
            .and_then(|bps| u32::try_from(bps).ok()),
        event: serde_json::from_value(value)?,
    })
}

/// S5-5: 領域に保存済みの checkpoint の版の集合。S5 の 3 領域に長期 workflow は無く、表も無いので常に空。
fn stored_checkpoint_versions() -> Vec<u32> {
    Vec::new()
}

fn check_live_limit_tx(conn: &Connection) -> Result<(), ScriptDomainError> {
    let live: i64 = conn.query_row(
        "SELECT COUNT(*) FROM script_generations WHERE state IN ('loaded', 'active', 'draining')",
        [],
        |row| row.get(0),
    )?;
    let live = usize::try_from(live).unwrap_or(usize::MAX);
    if live >= SCRIPT_GENERATIONS_LIVE_MAX {
        return Err(ScriptDomainError::GenerationLimit {
            live,
            max: SCRIPT_GENERATIONS_LIVE_MAX,
        });
    }
    Ok(())
}

fn insert_generation_tx(
    tx: &Connection,
    new: &NewScriptGeneration,
    state: ScriptGenerationState,
    reason: &str,
    ts: &str,
) -> Result<ScriptGeneration, ScriptDomainError> {
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
            state.as_str(),
            ts,
        ],
    )?;
    let row = read_generation_tx(tx, &id)?
        .ok_or_else(|| StoreError::Invalid(format!("script generation {id} vanished")))?;
    append_generation_event_tx(
        tx,
        &ScriptGenerationEvent {
            kind: ScriptGenerationEventKind::entering(state),
            generation_id: id,
            bundle_sha256: Some(new.bundle_sha256.clone()),
            from_state: None,
            to_state: Some(state),
            actor: SCRIPT_RUNTIME_ACTOR.to_owned(),
            reason: reason.to_owned(),
            cancelled_in_flight: None,
        },
        ts,
    )?;
    Ok(row)
}

/// generation の状態を書く唯一の場所（S5-1）。遷移の表にない遷移は `IllegalTransition`。
pub(super) fn transition_tx(
    tx: &Connection,
    current: &ScriptGeneration,
    to: ScriptGenerationState,
    actor: &str,
    reason: &str,
    cancelled_in_flight: Option<u64>,
    ts: &str,
) -> Result<ScriptGeneration, ScriptDomainError> {
    if !current.state.may_become(to) {
        return Err(ScriptDomainError::IllegalTransition {
            id: current.id.clone(),
            from: current.state,
            to,
        });
    }
    tx.execute(
        "UPDATE script_generations SET state = ?1, updated_at = ?2 WHERE id = ?3",
        params![to.as_str(), ts, current.id],
    )?;
    append_generation_event_tx(
        tx,
        &ScriptGenerationEvent {
            kind: ScriptGenerationEventKind::entering(to),
            generation_id: current.id.clone(),
            bundle_sha256: Some(current.bundle_sha256.clone()),
            from_state: Some(current.state),
            to_state: Some(to),
            actor: actor.to_owned(),
            reason: reason.to_owned(),
            cancelled_in_flight,
        },
        ts,
    )?;
    read_generation_tx(tx, &current.id)?.ok_or_else(|| {
        ScriptDomainError::Store(StoreError::Invalid(format!(
            "script generation {} vanished",
            current.id
        )))
    })
}

pub(super) fn append_generation_event_tx(
    tx: &Connection,
    event: &ScriptGenerationEvent,
    ts: &str,
) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO script_generation_events (ts, generation_id, json) VALUES (?1, ?2, ?3)",
        params![ts, event.generation_id, serde_json::to_string(event)?],
    )?;
    Ok(())
}

/// generation を `invalid` にし、指している全領域を `rust` に戻す（S5-7）。戻した領域を返す。
pub(super) fn invalidate_tx(
    tx: &Connection,
    current: &ScriptGeneration,
    actor: &str,
    reason: &str,
    ts: &str,
) -> Result<(ScriptGeneration, Vec<ScriptDomain>), ScriptDomainError> {
    let row = transition_tx(
        tx,
        current,
        ScriptGenerationState::Invalid,
        actor,
        reason,
        None,
        ts,
    )?;
    let mut rolled_back = Vec::new();
    for domain in ScriptDomain::ALL {
        let Some(mode) = read_mode_tx(tx, domain)? else {
            continue;
        };
        if mode.generation_id.as_deref() == Some(current.id.as_str()) {
            write_mode_tx(tx, &mode, ScriptMode::Rust, None, None, actor, reason, ts)?;
            rolled_back.push(domain);
        }
    }
    Ok((row, rolled_back))
}

/// mode の行を CAS で書き、`script_domain_mode_changed` を追記し、generation の状態を動かす（S5-2 の切替）。
/// 新しい generation が `loaded` なら `active` に、離れた generation を指す行が残らず `active` なら `draining` にする。
#[allow(clippy::too_many_arguments)]
pub(super) fn write_mode_tx(
    tx: &Connection,
    current: &ScriptDomainModeRow,
    to_mode: ScriptMode,
    to_generation: Option<&str>,
    canary_bps: Option<u32>,
    actor: &str,
    reason: &str,
    ts: &str,
) -> Result<u64, ScriptDomainError> {
    let domain = current.domain;
    let updated = tx.execute(
        "UPDATE script_domain_modes \
         SET mode = ?1, generation_id = ?2, canary_bps = ?3, version = version + 1, \
             updated_at = ?4, updated_by = ?5 \
         WHERE domain = ?6 AND version = ?7",
        params![
            to_mode.as_str(),
            to_generation,
            canary_bps,
            ts,
            actor,
            domain.as_str(),
            to_i64(current.version)?,
        ],
    )?;
    if updated != 1 {
        let found = read_mode_tx(tx, domain)?.map_or(0, |r| r.version);
        return Err(ScriptDomainError::VersionConflict {
            domain: domain.as_str().to_owned(),
            expected: current.version,
            found,
        });
    }
    let event = Event::ScriptDomainModeChanged {
        domain,
        from_mode: current.mode,
        to_mode,
        from_generation: current.generation_id.clone(),
        to_generation: to_generation.map(str::to_owned),
        version: current.version + 1,
        actor: actor.to_owned(),
        reason: reason.to_owned(),
    };
    let mut json = serde_json::to_value(&event)?;
    if let (Some(bps), Some(object)) = (canary_bps, json.as_object_mut()) {
        object.insert("canary_bps".to_owned(), bps.into());
    }
    tx.execute(
        "INSERT INTO script_domain_events (ts, domain, json) VALUES (?1, ?2, ?3)",
        params![ts, domain.as_str(), serde_json::to_string(&json)?],
    )?;
    let event_id = u64::try_from(tx.last_insert_rowid())
        .map_err(|_| StoreError::Invalid("negative script_domain_events id".into()))?;
    let moved = current.generation_id.as_deref() != to_generation;
    if moved && let Some(id) = to_generation {
        let generation = read_generation_tx(tx, id)?
            .ok_or_else(|| ScriptDomainError::GenerationNotFound(id.to_owned()))?;
        if generation.state == ScriptGenerationState::Loaded {
            transition_tx(
                tx,
                &generation,
                ScriptGenerationState::Active,
                actor,
                reason,
                None,
                ts,
            )?;
        }
    }
    if moved && let Some(id) = current.generation_id.as_deref() {
        let pointed: i64 = tx.query_row(
            "SELECT COUNT(*) FROM script_domain_modes WHERE generation_id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        if pointed == 0
            && let Some(generation) = read_generation_tx(tx, id)?
            && generation.state == ScriptGenerationState::Active
        {
            transition_tx(
                tx,
                &generation,
                ScriptGenerationState::Draining,
                actor,
                reason,
                None,
                ts,
            )?;
        }
    }
    Ok(event_id)
}

type RawModeRow = (
    String,
    String,
    Option<String>,
    i64,
    Option<i64>,
    String,
    String,
);

fn raw_mode_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawModeRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
    ))
}

fn mode_row(raw: RawModeRow) -> Result<ScriptDomainModeRow, StoreError> {
    let (domain, mode, generation_id, version, canary_bps, updated_at, updated_by) = raw;
    Ok(ScriptDomainModeRow {
        domain: ScriptDomain::parse(&domain)
            .ok_or_else(|| StoreError::Invalid(format!("unknown script domain `{domain}`")))?,
        mode: ScriptMode::parse(&mode)
            .ok_or_else(|| StoreError::Invalid(format!("unknown script mode `{mode}`")))?,
        generation_id,
        version: u64::try_from(version).map_err(|_| {
            StoreError::Invalid(format!("negative script domain version {version}"))
        })?,
        canary_bps: canary_bps
            .map(|bps| {
                u32::try_from(bps)
                    .map_err(|_| StoreError::Invalid(format!("script canary_bps {bps}")))
            })
            .transpose()?,
        updated_at,
        updated_by,
    })
}

pub(super) fn read_mode_tx(
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

pub(super) fn read_generation_tx(
    conn: &Connection,
    id: &str,
) -> Result<Option<ScriptGeneration>, StoreError> {
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
