//! ADR 2026-11-03-ts-intended-behavior-change D4: 方針改善の仕様の承認・取り消し（migration 0108）。
//!
//! 承認は人の主体（`human:*`）だけ。取り消しは誰でも（説明を減らす向き）。記録の表は追記専用で、UPDATE・DELETE の経路を持たない。

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use time::OffsetDateTime;

use super::{SqliteStore, StoreError, format_rfc3339};
use crate::script_behavior_change::{
    ApprovedBehaviorChange, BehaviorChangeError, BehaviorChangeEvent, BehaviorChangeRow,
    BehaviorChangeSpec, BehaviorChangeState, domain_supported,
};
use crate::script_compare::{ScriptDomain, actor_may_enable_ts};

/// 承認の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BehaviorChangeApproval {
    /// 表を変えたか（同じ hash がすでに `approved` なら `false`）。
    pub changed: bool,
    /// 同じ transaction で `superseded` にした前の版の hash。
    pub superseded: Option<String>,
    pub row: BehaviorChangeRow,
}

const ROW_COLUMNS: &str = "spec_sha256, change_id, version, domain, spec_json, state, approved_by, \
                           approved_at, updated_by, updated_at";

fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, String)> {
    Ok((row.get(0)?, row.get(5)?))
}

fn parse_row(conn: &Connection, sha: &str) -> Result<Option<BehaviorChangeRow>, StoreError> {
    let raw = conn
        .query_row(
            &format!("SELECT {ROW_COLUMNS} FROM script_behavior_changes WHERE spec_sha256 = ?1"),
            params![sha],
            |r| {
                Ok((
                    read_row(r)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, String>(8)?,
                    r.get::<_, String>(9)?,
                ))
            },
        )
        .optional()?;
    let Some(((sha, state), spec_json, approved_by, approved_at, updated_by, updated_at)) = raw
    else {
        return Ok(None);
    };
    let spec: BehaviorChangeSpec = serde_json::from_str(&spec_json)?;
    let state = BehaviorChangeState::parse(&state)
        .ok_or_else(|| StoreError::Invalid(format!("behavior change state `{state}`")))?;
    Ok(Some(BehaviorChangeRow {
        spec_sha256: sha,
        id: spec.id.clone(),
        version: spec.version,
        domain: spec.domain,
        state,
        spec,
        approved_by,
        approved_at,
        updated_by,
        updated_at,
    }))
}

#[allow(clippy::too_many_arguments)]
fn append_event_tx(
    tx: &Connection,
    ts: &str,
    sha: &str,
    id: &str,
    version: u32,
    action: BehaviorChangeState,
    actor: &str,
    reason: &str,
) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO script_behavior_change_events \
             (ts, spec_sha256, change_id, version, action, actor, reason) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![ts, sha, id, version, action.as_str(), actor, reason],
    )?;
    Ok(())
}

impl SqliteStore {
    /// 仕様を承認する（D4）。`given_sha256` は人が見た仕様の hash で、計算と違えば拒否する。
    pub fn script_behavior_change_approve(
        &self,
        spec: &BehaviorChangeSpec,
        given_sha256: &str,
        actor: &str,
        reason: &str,
        now: OffsetDateTime,
    ) -> Result<BehaviorChangeApproval, BehaviorChangeError> {
        if !actor_may_enable_ts(actor) {
            return Err(BehaviorChangeError::RequiresHuman(actor.to_owned()));
        }
        spec.validate()?;
        if !domain_supported(spec.domain) {
            return Err(BehaviorChangeError::DomainUnsupported(
                spec.domain.as_str().to_owned(),
            ));
        }
        let sha = spec.sha256();
        if sha != given_sha256 {
            return Err(BehaviorChangeError::HashMismatch {
                computed: sha,
                given: given_sha256.to_owned(),
            });
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(row) = parse_row(&tx, &sha)? {
            if row.state == BehaviorChangeState::Approved {
                tx.commit()?;
                return Ok(BehaviorChangeApproval {
                    changed: false,
                    superseded: None,
                    row,
                });
            }
            return Err(BehaviorChangeError::Retired {
                sha,
                state: row.state,
            });
        }
        let taken: Option<String> = tx
            .query_row(
                "SELECT spec_sha256 FROM script_behavior_changes WHERE change_id = ?1 AND version = ?2",
                params![spec.id, spec.version],
                |r| r.get(0),
            )
            .optional()?;
        if taken.is_some() {
            return Err(BehaviorChangeError::VersionTaken {
                id: spec.id.clone(),
                version: spec.version,
            });
        }
        let latest: Option<u32> = tx.query_row(
            "SELECT MAX(version) FROM script_behavior_changes WHERE change_id = ?1",
            params![spec.id],
            |r| r.get(0),
        )?;
        if let Some(latest) = latest.filter(|l| *l >= spec.version) {
            return Err(BehaviorChangeError::VersionNotNewer {
                id: spec.id.clone(),
                version: spec.version,
                latest,
            });
        }
        let current: Option<(String, u32)> = tx
            .query_row(
                "SELECT spec_sha256, version FROM script_behavior_changes \
                 WHERE change_id = ?1 AND state = 'approved'",
                params![spec.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((current_sha, _)) = &current
            && spec.supersedes.as_deref() != Some(current_sha.as_str())
        {
            return Err(BehaviorChangeError::Invalid(
                crate::script_behavior_change::BehaviorChangeInvalid(format!(
                    "supersedes must name the approved version {current_sha}"
                )),
            ));
        }
        let ts = format_rfc3339(now)?;
        if let Some((current_sha, current_version)) = &current {
            tx.execute(
                "UPDATE script_behavior_changes SET state = 'superseded', updated_by = ?2, updated_at = ?3 \
                 WHERE spec_sha256 = ?1",
                params![current_sha, actor, ts],
            )?;
            append_event_tx(
                &tx,
                &ts,
                current_sha,
                &spec.id,
                *current_version,
                BehaviorChangeState::Superseded,
                actor,
                &format!("superseded by version {} ({sha})", spec.version),
            )?;
        }
        tx.execute(
            &format!(
                "INSERT INTO script_behavior_changes ({ROW_COLUMNS}) \
                 VALUES (?1, ?2, ?3, ?4, ?5, 'approved', ?6, ?7, ?6, ?7)"
            ),
            params![
                sha,
                spec.id,
                spec.version,
                spec.domain.as_str(),
                serde_json::to_string(spec)?,
                actor,
                ts
            ],
        )?;
        append_event_tx(
            &tx,
            &ts,
            &sha,
            &spec.id,
            spec.version,
            BehaviorChangeState::Approved,
            actor,
            reason,
        )?;
        let row = parse_row(&tx, &sha)?
            .ok_or_else(|| BehaviorChangeError::NotFound(sha.clone()))?;
        tx.commit()?;
        Ok(BehaviorChangeApproval {
            changed: true,
            superseded: current.map(|(s, _)| s),
            row,
        })
    }

    /// 承認を取り消す（誰でも。D4）。`approved` でなければ何も変えず `false`。
    pub fn script_behavior_change_revoke(
        &self,
        sha: &str,
        actor: &str,
        reason: &str,
        now: OffsetDateTime,
    ) -> Result<bool, BehaviorChangeError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row = parse_row(&tx, sha)?.ok_or_else(|| BehaviorChangeError::NotFound(sha.into()))?;
        if row.state != BehaviorChangeState::Approved {
            tx.commit()?;
            return Ok(false);
        }
        let ts = format_rfc3339(now)?;
        tx.execute(
            "UPDATE script_behavior_changes SET state = 'revoked', updated_by = ?2, updated_at = ?3 \
             WHERE spec_sha256 = ?1",
            params![sha, actor, ts],
        )?;
        append_event_tx(
            &tx,
            &ts,
            sha,
            &row.id,
            row.version,
            BehaviorChangeState::Revoked,
            actor,
            reason,
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// 全行（`change_id`・`version` の順）。
    pub fn script_behavior_changes(&self) -> Result<Vec<BehaviorChangeRow>, StoreError> {
        let conn = self.lock()?;
        let shas: Vec<String> = conn
            .prepare(
                "SELECT spec_sha256 FROM script_behavior_changes ORDER BY change_id, version",
            )?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        shas.iter()
            .filter_map(|s| parse_row(&conn, s).transpose())
            .collect()
    }

    /// 記録（id の順）。
    pub fn script_behavior_change_events(&self) -> Result<Vec<BehaviorChangeEvent>, StoreError> {
        let conn = self.lock()?;
        let rows = conn
            .prepare(
                "SELECT id, ts, spec_sha256, change_id, version, action, actor, reason \
                 FROM script_behavior_change_events ORDER BY id",
            )?
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, u32>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(id, ts, sha, change_id, version, action, actor, reason)| {
                Ok(BehaviorChangeEvent {
                    id: u64::try_from(id).unwrap_or_default(),
                    ts,
                    spec_sha256: sha,
                    change_id,
                    version,
                    action: BehaviorChangeState::parse(&action).ok_or_else(|| {
                        StoreError::Invalid(format!("behavior change action `{action}`"))
                    })?,
                    actor,
                    reason,
                })
            })
            .collect()
    }

    /// 領域の承認済みの仕様（`change_id` の順）。dispatcher が pass ごとに読む（D4）。
    pub fn script_behavior_changes_approved(
        &self,
        domain: ScriptDomain,
    ) -> Result<Vec<ApprovedBehaviorChange>, StoreError> {
        let conn = self.lock()?;
        let rows: Vec<(String, String)> = conn
            .prepare(
                "SELECT spec_sha256, spec_json FROM script_behavior_changes \
                 WHERE domain = ?1 AND state = 'approved' ORDER BY change_id",
            )?
            .query_map(params![domain.as_str()], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        rows.into_iter()
            .map(|(sha, json)| {
                let spec: BehaviorChangeSpec = serde_json::from_str(&json)?;
                // 保存した本文と hash が食い違う行は使わない（fail-closed）。
                if spec.sha256() != sha {
                    return Err(StoreError::Invalid(format!(
                        "behavior change `{sha}`: stored spec hashes to {}",
                        spec.sha256()
                    )));
                }
                Ok(ApprovedBehaviorChange {
                    spec,
                    spec_sha256: sha,
                })
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "script_behavior_changes_tests.rs"]
mod tests;
