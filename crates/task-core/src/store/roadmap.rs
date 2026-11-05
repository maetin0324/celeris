//! ADR 2026-11-04-project-controller 付記 A5: 案件の Roadmap の版（`project_roadmap_versions`）と Outcome の記録
//! （`project_roadmap_outcome_events`）。どちらも追記専用（migration 0113 の trigger）。
//!
//! 書き込みは呼び出し側の transaction（`*_tx`）で行い、監査（`/cos/operations`）や `human_epoch` と同じ
//! transaction に束ねる。`milestones` 表・`tasks.milestone_id` は読まない。

use rusqlite::{Connection, OptionalExtension, params};
use time::OffsetDateTime;

use crate::roadmap::{
    OutcomeEvent, OutcomeStatus, RoadmapInvalid, RoadmapProposalInput, RoadmapStatus,
    RoadmapVersion, validate_proposal,
};

use super::{SqliteStore, StoreError, format_rfc3339};

const VERSION_COLUMNS: &str = "project_id, version, status, base_version, request_sha256, \
     change_reason, outcomes_json, proposed_by, proposed_at, decided_by, decided_at, decision_note, \
     superseded_at";

const EVENT_COLUMNS: &str = "id, outcome_id, kind, status, root_task_id, roadmap_version, \
     evidence_json, reason, actor, created_at";

/// Roadmap の書き込みの失敗。
#[derive(Debug, thiserror::Error)]
pub enum RoadmapWriteError {
    #[error("{}: {}", .0.code, .0.detail)]
    Invalid(RoadmapInvalid),
    /// 提案の `base_version` か決定の土台が今の approved の版と違う。
    #[error("the approved roadmap version is now {current:?}")]
    Stale { current: Option<i64> },
    #[error("roadmap version {0} not found")]
    NotFound(i64),
    /// 決定の対象が `proposed` でない。
    #[error("roadmap version is {0}, not proposed")]
    NotProposed(&'static str),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for RoadmapWriteError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Store(StoreError::from(e))
    }
}

fn version_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<RoadmapVersion, StoreError>> {
    let status: String = row.get(2)?;
    let outcomes: String = row.get(6)?;
    let mut v = RoadmapVersion {
        project_id: row.get(0)?,
        version: row.get(1)?,
        status: RoadmapStatus::Proposed,
        base_version: row.get(3)?,
        request_sha256: row.get(4)?,
        change_reason: row.get(5)?,
        outcomes: Vec::new(),
        proposed_by: row.get(7)?,
        proposed_at: row.get(8)?,
        decided_by: row.get(9)?,
        decided_at: row.get(10)?,
        decision_note: row.get(11)?,
        superseded_at: row.get(12)?,
    };
    Ok((|| {
        v.status = RoadmapStatus::parse(&status)
            .ok_or_else(|| StoreError::Invalid(format!("roadmap status {status:?}")))?;
        v.outcomes = serde_json::from_str(&outcomes)?;
        Ok(v)
    })())
}

fn event_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<OutcomeEvent, StoreError>> {
    let status: Option<String> = row.get(3)?;
    let evidence: String = row.get(6)?;
    let mut e = OutcomeEvent {
        id: row.get(0)?,
        outcome_id: row.get(1)?,
        kind: row.get(2)?,
        status: None,
        root_task_id: row.get(4)?,
        roadmap_version: row.get(5)?,
        evidence: Vec::new(),
        reason: row.get(7)?,
        actor: row.get(8)?,
        created_at: row.get(9)?,
    };
    Ok((|| {
        e.status = match status {
            None => None,
            Some(s) => Some(
                OutcomeStatus::parse(&s)
                    .ok_or_else(|| StoreError::Invalid(format!("outcome status {s:?}")))?,
            ),
        };
        e.evidence = serde_json::from_str(&evidence)?;
        Ok(e)
    })())
}

pub(super) fn approved_tx(
    conn: &Connection,
    project: &str,
) -> Result<Option<RoadmapVersion>, StoreError> {
    conn.query_row(
        &format!(
            "SELECT {VERSION_COLUMNS} FROM project_roadmap_versions \
             WHERE project_id = ?1 AND status = 'approved'"
        ),
        params![project],
        version_row,
    )
    .optional()?
    .transpose()
}

fn version_tx(
    conn: &Connection,
    project: &str,
    version: i64,
) -> Result<Option<RoadmapVersion>, StoreError> {
    conn.query_row(
        &format!(
            "SELECT {VERSION_COLUMNS} FROM project_roadmap_versions \
             WHERE project_id = ?1 AND version = ?2"
        ),
        params![project, version],
        version_row,
    )
    .optional()?
    .transpose()
}

/// Outcome の記録 1 件の入力。
#[derive(Debug, Clone)]
pub struct NewOutcomeEvent<'a> {
    pub project_id: &'a str,
    pub outcome_id: &'a str,
    /// `Some` なら状態の記録、`None` なら root との対応の記録（`root_task_id` 必須）。
    pub status: Option<OutcomeStatus>,
    pub root_task_id: Option<&'a str>,
    pub evidence: &'a [String],
    pub reason: &'a str,
    pub actor: &'a str,
}

impl SqliteStore {
    /// 今の approved の版（無ければ `None`）。
    pub fn roadmap_approved(&self, project: &str) -> Result<Option<RoadmapVersion>, StoreError> {
        self.with_read_conn(|conn| approved_tx(conn, project))
    }

    /// 版の行 1 つ。
    pub fn roadmap_version(
        &self,
        project: &str,
        version: i64,
    ) -> Result<Option<RoadmapVersion>, StoreError> {
        self.with_read_conn(|conn| version_tx(conn, project, version))
    }

    /// 案件の版の行（版の番号の順）。
    pub fn roadmap_versions(&self, project: &str) -> Result<Vec<RoadmapVersion>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {VERSION_COLUMNS} FROM project_roadmap_versions \
                 WHERE project_id = ?1 ORDER BY version"
            ))?;
            let rows = stmt.query_map(params![project], version_row)?;
            rows.map(|r| r?).collect()
        })
    }

    /// 案件の Outcome の記録（古い順）。
    pub fn roadmap_outcome_events(&self, project: &str) -> Result<Vec<OutcomeEvent>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {EVENT_COLUMNS} FROM project_roadmap_outcome_events \
                 WHERE project_id = ?1 ORDER BY id"
            ))?;
            let rows = stmt.query_map(params![project], event_row)?;
            rows.map(|r| r?).collect()
        })
    }

    /// 第 3 フェーズ I2・I11（付記 P0-B）: controller の `mark_milestone` の効果を書く transaction の中で、Outcome の
    /// 最後の状態の記録（状態と `reason`）を読む。記録が無ければ `None`。
    pub fn roadmap_last_status_tx(
        conn: &Connection,
        project: &str,
        outcome_id: &str,
    ) -> Result<Option<(OutcomeStatus, String)>, StoreError> {
        let row: Option<(Option<String>, String)> = conn
            .query_row(
                "SELECT status, reason FROM project_roadmap_outcome_events WHERE project_id = ?1 \
                 AND outcome_id = ?2 AND kind = 'status' ORDER BY id DESC LIMIT 1",
                params![project, outcome_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(row
            .and_then(|(status, reason)| Some((OutcomeStatus::parse(status.as_deref()?)?, reason))))
    }

    /// 同じ transaction で効果の門（mode・phase・案件の状態・判断の `human_epoch`）が開いているか（A1 の付記 11 と
    /// 同じ門）。閉じていれば controller は Outcome の状態を書かない。
    pub fn roadmap_effect_gate_open_tx(
        conn: &Connection,
        project: &str,
        decision_epoch: i64,
    ) -> Result<bool, StoreError> {
        super::project_wake_effect::effect_gate_open_tx(conn, project, decision_epoch)
    }

    /// 提案を積む（今の approved の版は変えない）。`base_version` が今の approved と違えば `Stale`。
    /// 版の番号は案件の最大の次。
    pub fn roadmap_propose_tx(
        conn: &Connection,
        project: &str,
        input: &RoadmapProposalInput,
        request_sha256: &str,
        by: &str,
        now: OffsetDateTime,
    ) -> Result<RoadmapVersion, RoadmapWriteError> {
        let approved = approved_tx(conn, project)?;
        let current = approved.as_ref().map(|a| a.version);
        if input.base_version != current {
            return Err(RoadmapWriteError::Stale { current });
        }
        validate_proposal(approved.as_ref().map(|a| a.outcomes.as_slice()), input)
            .map_err(RoadmapWriteError::Invalid)?;
        let next: i64 = conn.query_row(
            "SELECT COALESCE(MAX(version), 0) + 1 FROM project_roadmap_versions WHERE project_id = ?1",
            params![project],
            |r| r.get(0),
        )?;
        let at = format_rfc3339(now)?;
        conn.execute(
            "INSERT INTO project_roadmap_versions (project_id, version, status, base_version, \
             request_sha256, change_reason, outcomes_json, proposed_by, proposed_at) \
             VALUES (?1, ?2, 'proposed', ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                project,
                next,
                input.base_version,
                request_sha256,
                input.change_reason,
                serde_json::to_string(&input.outcomes).map_err(StoreError::from)?,
                by,
                at,
            ],
        )?;
        version_tx(conn, project, next)?.ok_or(RoadmapWriteError::NotFound(next))
    }

    /// 人の決定。`approve` なら今の approved を `superseded` にしてこの版を `approved` にする（土台が今の
    /// approved と違えば `Stale`）。却下なら `rejected`。呼び出し側が人であることを確かめる。
    pub fn roadmap_decide_tx(
        conn: &Connection,
        project: &str,
        version: i64,
        approve: bool,
        note: Option<&str>,
        by: &str,
        now: OffsetDateTime,
    ) -> Result<RoadmapVersion, RoadmapWriteError> {
        let row =
            version_tx(conn, project, version)?.ok_or(RoadmapWriteError::NotFound(version))?;
        if row.status != RoadmapStatus::Proposed {
            return Err(RoadmapWriteError::NotProposed(row.status.as_str()));
        }
        let at = format_rfc3339(now)?;
        if approve {
            let current = approved_tx(conn, project)?.map(|a| a.version);
            if row.base_version != current {
                return Err(RoadmapWriteError::Stale { current });
            }
            conn.execute(
                "UPDATE project_roadmap_versions SET status = 'superseded', superseded_at = ?2 \
                 WHERE project_id = ?1 AND status = 'approved'",
                params![project, at],
            )?;
        }
        conn.execute(
            "UPDATE project_roadmap_versions SET status = ?3, decided_by = ?4, decided_at = ?5, \
             decision_note = ?6 WHERE project_id = ?1 AND version = ?2",
            params![
                project,
                version,
                if approve { "approved" } else { "rejected" },
                by,
                at,
                note
            ],
        )?;
        version_tx(conn, project, version)?.ok_or(RoadmapWriteError::NotFound(version))
    }

    /// Outcome の記録を 1 件足す。approved の版が要り、Outcome はその版にあって取り下げていないこと。
    /// `dropped` は状態の記録で付けられない（版で付ける）。root との対応は同じ root を 2 回記録しない。
    /// 返り値は足した記録の id（対応が既にあれば `None`）。
    pub fn roadmap_outcome_event_tx(
        conn: &Connection,
        new: &NewOutcomeEvent<'_>,
        now: OffsetDateTime,
    ) -> Result<Option<i64>, RoadmapWriteError> {
        let approved = approved_tx(conn, new.project_id)?.ok_or_else(|| {
            RoadmapWriteError::Invalid(RoadmapInvalid::new(
                "roadmap_not_approved",
                "the project has no approved roadmap version",
            ))
        })?;
        let Some(spec) = approved.outcomes.iter().find(|o| o.id == new.outcome_id) else {
            return Err(RoadmapWriteError::Invalid(RoadmapInvalid::new(
                "roadmap_outcome_unknown",
                format!(
                    "outcome {:?} is not in the approved version",
                    new.outcome_id
                ),
            )));
        };
        if spec.dropped {
            return Err(RoadmapWriteError::Invalid(RoadmapInvalid::new(
                "roadmap_outcome_dropped",
                format!(
                    "outcome {:?} is dropped in the approved version",
                    new.outcome_id
                ),
            )));
        }
        if new.status == Some(OutcomeStatus::Dropped) {
            return Err(RoadmapWriteError::Invalid(RoadmapInvalid::new(
                "roadmap_drop_needs_version",
                "dropping an outcome is a roadmap change; propose a version with dropped: true",
            )));
        }
        let kind = if new.status.is_some() {
            "status"
        } else {
            let Some(root) = new.root_task_id else {
                return Err(RoadmapWriteError::Invalid(RoadmapInvalid::new(
                    "roadmap_invalid",
                    "a root link needs root_task_id",
                )));
            };
            let linked: Option<i64> = conn
                .query_row(
                    "SELECT id FROM project_roadmap_outcome_events WHERE project_id = ?1 \
                     AND outcome_id = ?2 AND kind = 'root_linked' AND root_task_id = ?3",
                    params![new.project_id, new.outcome_id, root],
                    |r| r.get(0),
                )
                .optional()?;
            if linked.is_some() {
                return Ok(None);
            }
            "root_linked"
        };
        conn.execute(
            "INSERT INTO project_roadmap_outcome_events (project_id, outcome_id, kind, status, \
             root_task_id, roadmap_version, evidence_json, reason, actor, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                new.project_id,
                new.outcome_id,
                kind,
                new.status.map(OutcomeStatus::as_str),
                new.root_task_id,
                approved.version,
                serde_json::to_string(new.evidence).map_err(StoreError::from)?,
                new.reason,
                new.actor,
                format_rfc3339(now)?,
            ],
        )?;
        Ok(Some(conn.last_insert_rowid()))
    }
}

#[cfg(test)]
#[path = "roadmap_tests.rs"]
mod tests;
