//! Phase 3 browser state persistence. All writes are scoped to the full task/run/session key.

use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use crate::browser_control::{BrowserControl, ControlError, ControlOutcome, ControlRequest};
use crate::browser_identity::{
    self, BrowserIdentity, IdentityDenied, IdentityRegistration, IdentityState, SealedEnvelope,
};
use crate::browser_live::{PersistedLiveEvent, ScrubbedLiveEvent};
use crate::store::{SqliteStore, StoreError};

#[derive(Debug, Clone, Copy)]
pub struct BrowserSessionKey<'a> {
    pub task_id: &'a str,
    pub run_id: &'a str,
    pub session_id: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredLiveEvent {
    pub seq: u64,
    pub body: PersistedLiveEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveEventPage {
    pub oldest_seq: Option<u64>,
    pub latest_seq: Option<u64>,
    pub events: Vec<StoredLiveEvent>,
}

#[derive(Debug, thiserror::Error)]
pub enum BrowserStoreError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Control(#[from] ControlError),
    #[error("identity denied: {0:?}")]
    Identity(IdentityDenied),
}

impl From<IdentityDenied> for BrowserStoreError {
    fn from(value: IdentityDenied) -> Self {
        Self::Identity(value)
    }
}

impl From<rusqlite::Error> for BrowserStoreError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Store(StoreError::Sqlite(value))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredIdentity {
    pub identity: BrowserIdentity,
    pub key_label: String,
    /// Broker-sealed bytes only; never plaintext browser state.
    pub sealed_blob: Option<Vec<u8>>,
}

fn invalid(message: impl Into<String>) -> StoreError {
    StoreError::Invalid(message.into())
}

fn nonempty_key(key: BrowserSessionKey<'_>) -> Result<(), StoreError> {
    if key.task_id.is_empty() || key.run_id.is_empty() || key.session_id.is_empty() {
        Err(invalid("browser task/run/session binding is required"))
    } else {
        Ok(())
    }
}

fn kind(event: &PersistedLiveEvent) -> &'static str {
    match event {
        PersistedLiveEvent::Status { .. } => "status",
        PersistedLiveEvent::Tabs { .. } => "tabs",
        PersistedLiveEvent::Url { .. } => "url",
        PersistedLiveEvent::Console { .. } => "console",
    }
}

fn identity_state(state: IdentityState) -> &'static str {
    match state {
        IdentityState::Active => "active",
        IdentityState::Revoked => "revoked",
        IdentityState::Deleted => "deleted",
    }
}

fn read_identity(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredIdentity> {
    let state: String = row.get(8)?;
    let state = match state.as_str() {
        "active" => IdentityState::Active,
        "revoked" => IdentityState::Revoked,
        "deleted" => IdentityState::Deleted,
        _ => {
            return Err(rusqlite::Error::InvalidColumnType(
                8,
                "state".into(),
                rusqlite::types::Type::Text,
            ));
        }
    };
    Ok(StoredIdentity {
        identity: BrowserIdentity {
            identity_id: row.get(0)?,
            project_id: row.get(1)?,
            origin: row.get(2)?,
            generation: row.get(4)?,
            created_at: row.get(5)?,
            expires_at: row.get(6)?,
            demand_confirmed_by: row.get(7)?,
            state,
        },
        key_label: row.get(3)?,
        sealed_blob: row.get(9)?,
    })
}

const IDENTITY_SELECT: &str = "SELECT identity_id, project_id, origin, key_label, generation, created_at, expires_at, demand_confirmed_by, state, sealed_blob FROM browser_identities";

impl SqliteStore {
    /// Append a scrubbed event; sequence is monotonic within one browser session.
    pub fn browser_live_append(
        &self,
        key: BrowserSessionKey<'_>,
        event: &ScrubbedLiveEvent,
    ) -> Result<u64, StoreError> {
        nonempty_key(key)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let next: i64 = tx.query_row(
            "SELECT COALESCE(MAX(event_seq), 0) + 1 FROM browser_live_events WHERE task_id=?1 AND run_id=?2 AND session_id=?3",
            params![key.task_id, key.run_id, key.session_id],
            |r| r.get(0),
        )?;
        let body = event.as_persisted();
        tx.execute(
            "INSERT INTO browser_live_events(task_id, run_id, session_id, event_seq, kind, body_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![key.task_id, key.run_id, key.session_id, next, kind(body), serde_json::to_string(body)?],
        )?;
        tx.commit()?;
        u64::try_from(next).map_err(|_| invalid("negative browser event sequence"))
    }

    /// Return the retained range as well as events, so callers can choose Replay or Reset.
    pub fn browser_live_after(
        &self,
        key: BrowserSessionKey<'_>,
        after: u64,
        limit: usize,
    ) -> Result<LiveEventPage, StoreError> {
        nonempty_key(key)?;
        let conn = self.lock()?;
        let (oldest, latest): (Option<i64>, Option<i64>) = conn.query_row(
            "SELECT MIN(event_seq), MAX(event_seq) FROM browser_live_events WHERE task_id=?1 AND run_id=?2 AND session_id=?3",
            params![key.task_id, key.run_id, key.session_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let mut stmt = conn.prepare(
            "SELECT event_seq, body_json FROM browser_live_events WHERE task_id=?1 AND run_id=?2 AND session_id=?3 AND event_seq>?4 ORDER BY event_seq LIMIT ?5",
        )?;
        let after = match i64::try_from(after) {
            Ok(value) => value,
            Err(_) => i64::MAX,
        };
        let rows = stmt.query_map(
            params![
                key.task_id,
                key.run_id,
                key.session_id,
                after,
                limit.min(1000)
            ],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
        )?;
        let mut events = Vec::new();
        for row in rows {
            let (seq, json) = row?;
            events.push(StoredLiveEvent {
                seq: u64::try_from(seq).map_err(|_| invalid("negative browser event sequence"))?,
                body: serde_json::from_str(&json)?,
            });
        }
        Ok(LiveEventPage {
            oldest_seq: oldest.and_then(|n| u64::try_from(n).ok()),
            latest_seq: latest.and_then(|n| u64::try_from(n).ok()),
            events,
        })
    }

    /// Retention may remove old rows; the next append still allocates a higher sequence.
    pub fn browser_live_prune_before(
        &self,
        key: BrowserSessionKey<'_>,
        before: u64,
    ) -> Result<usize, StoreError> {
        nonempty_key(key)?;
        let conn = self.lock()?;
        let before = match i64::try_from(before) {
            Ok(value) => value,
            Err(_) => i64::MAX,
        };
        Ok(conn.execute(
            "DELETE FROM browser_live_events WHERE task_id=?1 AND run_id=?2 AND session_id=?3 AND event_seq<?4 AND event_seq < (SELECT MAX(event_seq) FROM browser_live_events WHERE task_id=?1 AND run_id=?2 AND session_id=?3)",
            params![key.task_id, key.run_id, key.session_id, before],
        )?)
    }

    pub fn browser_control_get(
        &self,
        key: BrowserSessionKey<'_>,
    ) -> Result<BrowserControl, StoreError> {
        nonempty_key(key)?;
        let conn = self.lock()?;
        let json: Option<String> = conn.query_row(
            "SELECT state_json FROM browser_control_state WHERE task_id=?1 AND run_id=?2 AND session_id=?3",
            params![key.task_id, key.run_id, key.session_id],
            |r| r.get(0),
        ).optional()?;
        json.map(|s| serde_json::from_str(&s).map_err(StoreError::from))
            .unwrap_or_else(|| Ok(BrowserControl::new()))
    }

    /// CAS and idempotency check run under one IMMEDIATE transaction.
    pub fn browser_control_apply(
        &self,
        key: BrowserSessionKey<'_>,
        req: &ControlRequest,
        now: u64,
    ) -> Result<ControlOutcome, BrowserStoreError> {
        nonempty_key(key)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let json: Option<String> = tx.query_row(
            "SELECT state_json FROM browser_control_state WHERE task_id=?1 AND run_id=?2 AND session_id=?3",
            params![key.task_id, key.run_id, key.session_id], |r| r.get(0),
        ).optional()?;
        let mut state: BrowserControl = match json {
            Some(json) => serde_json::from_str(&json).map_err(StoreError::from)?,
            None => BrowserControl::new(),
        };
        let outcome = state.apply(req, now)?;
        if !outcome.replayed {
            let phase = serde_json::to_string(&state.phase()).map_err(StoreError::from)?;
            tx.execute(
                "INSERT INTO browser_control_state(task_id,run_id,session_id,phase,version,lease_holder,lease_expires_at,state_json) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
                 ON CONFLICT(task_id,run_id,session_id) DO UPDATE SET phase=excluded.phase,version=excluded.version,lease_holder=excluded.lease_holder,lease_expires_at=excluded.lease_expires_at,state_json=excluded.state_json",
                params![key.task_id, key.run_id, key.session_id, phase, state.version(), state.lease().map(|l| l.holder.as_str()), state.lease().map(|l| l.expires_at), serde_json::to_string(&state).map_err(StoreError::from)?],
            )?;
            tx.execute(
                "INSERT INTO browser_control_actions(task_id,run_id,session_id,idempotency_key,command_json,outcome_json) VALUES (?1,?2,?3,?4,?5,?6)",
                params![key.task_id, key.run_id, key.session_id, req.idempotency_key, serde_json::to_string(&req.command).map_err(StoreError::from)?, serde_json::to_string(&outcome).map_err(StoreError::from)?],
            )?;
        }
        tx.commit()?;
        Ok(outcome)
    }

    pub fn browser_identity_get(
        &self,
        identity_id: &str,
    ) -> Result<Option<StoredIdentity>, StoreError> {
        let conn = self.lock()?;
        let sql = format!("{IDENTITY_SELECT} WHERE identity_id=?1");
        conn.query_row(&sql, params![identity_id], read_identity)
            .optional()
            .map_err(StoreError::from)
    }

    pub fn browser_identity_list(
        &self,
        project_id: &str,
    ) -> Result<Vec<StoredIdentity>, StoreError> {
        let conn = self.lock()?;
        let sql = format!("{IDENTITY_SELECT} WHERE project_id=?1 ORDER BY origin, identity_id");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![project_id], read_identity)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(StoreError::from)
    }

    /// The blob must already be sealed by the broker. Its envelope must bind to this identity.
    pub fn browser_identity_register(
        &self,
        req: &IdentityRegistration,
        envelope: &SealedEnvelope,
        sealed_blob: &[u8],
        now: u64,
    ) -> Result<StoredIdentity, BrowserStoreError> {
        let identity = browser_identity::register(req, now)?;
        browser_identity::check_envelope(&identity, envelope, now)?;
        if sealed_blob.is_empty() {
            return Err(invalid("sealed identity blob is empty").into());
        }
        let record = StoredIdentity {
            key_label: envelope.key_label.clone(),
            sealed_blob: Some(sealed_blob.to_vec()),
            identity,
        };
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO browser_identities(identity_id,project_id,origin,key_label,generation,created_at,expires_at,demand_confirmed_by,state,tombstone,sealed_blob) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'active',0,?9)",
            params![record.identity.identity_id, record.identity.project_id, record.identity.origin, record.key_label, record.identity.generation, record.identity.created_at, record.identity.expires_at, record.identity.demand_confirmed_by, sealed_blob],
        )?;
        Ok(record)
    }

    pub fn browser_identity_revoke(
        &self,
        identity_id: &str,
    ) -> Result<Option<StoredIdentity>, BrowserStoreError> {
        self.browser_identity_change(identity_id, false)
    }

    pub fn browser_identity_delete(
        &self,
        identity_id: &str,
    ) -> Result<Option<StoredIdentity>, BrowserStoreError> {
        self.browser_identity_change(identity_id, true)
    }

    fn browser_identity_change(
        &self,
        identity_id: &str,
        delete: bool,
    ) -> Result<Option<StoredIdentity>, BrowserStoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let sql = format!("{IDENTITY_SELECT} WHERE identity_id=?1");
        let Some(mut record) = tx
            .query_row(&sql, params![identity_id], read_identity)
            .optional()?
        else {
            return Ok(None);
        };
        let changed = if delete {
            browser_identity::delete(&mut record.identity)
        } else {
            browser_identity::revoke(&mut record.identity)
        };
        if changed {
            // Revocation and deletion destroy the stored envelope; only metadata survives.
            record.sealed_blob = None;
            if delete {
                record.identity.origin.clear();
                record.identity.demand_confirmed_by.clear();
                record.key_label.clear();
            }
            tx.execute(
                "UPDATE browser_identities SET generation=?2,state=?3,tombstone=?4,sealed_blob=NULL,origin=?5,key_label=?6,demand_confirmed_by=?7 WHERE identity_id=?1",
                params![identity_id, record.identity.generation, identity_state(record.identity.state), delete, record.identity.origin, record.key_label, record.identity.demand_confirmed_by],
            )?;
        }
        tx.commit()?;
        Ok(Some(record))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser_control::{ControlCommand, ControlPhase};
    use crate::browser_live::{LiveEvent, LiveTab};

    fn key() -> BrowserSessionKey<'static> {
        BrowserSessionKey {
            task_id: "task-a",
            run_id: "run-a",
            session_id: "session-a",
        }
    }

    #[test]
    fn live_events_are_scrubbed_and_retention_exposes_oldest()
    -> Result<(), Box<dyn std::error::Error>> {
        let store = SqliteStore::open_in_memory()?;
        let inputs = [
            LiveEvent::Console {
                level: "error".into(),
                text: "Set-Cookie: sid=SENTINEL".into(),
            },
            LiveEvent::Url {
                url: "https://u:p@example.com/a?token=SENTINEL".into(),
            },
            LiveEvent::Tabs {
                tabs: vec![LiveTab {
                    id: "1".into(),
                    url: "https://u:p@example.com/?token=SENTINEL".into(),
                    title: "SENTINEL".into(),
                }],
            },
        ];
        for (index, input) in inputs.iter().enumerate() {
            let scrubbed = ScrubbedLiveEvent::from_event(input).ok_or("event omitted")?;
            assert_eq!(
                store.browser_live_append(key(), &scrubbed)?,
                (index + 1) as u64
            );
        }
        assert!(
            ScrubbedLiveEvent::from_event(&LiveEvent::Frame {
                bytes: vec![1, 2, 3]
            })
            .is_none()
        );
        let page = store.browser_live_after(key(), 0, 20)?;
        assert_eq!(page.events.len(), 3);
        let json = serde_json::to_string(&page.events)?;
        assert!(!json.contains("SENTINEL"));
        assert!(!json.contains("u:p"));
        store.browser_live_prune_before(key(), 3)?;
        let page = store.browser_live_after(key(), 1, 20)?;
        assert_eq!(page.oldest_seq, Some(3));
        assert_eq!(page.latest_seq, Some(3));
        assert_eq!(page.events.len(), 1);
        // Even an aggressive prune leaves the latest sequence anchor.
        store.browser_live_prune_before(key(), 100)?;
        let status = ScrubbedLiveEvent::from_event(&LiveEvent::Status {
            state: "paused".into(),
        })
        .ok_or("status omitted")?;
        assert_eq!(store.browser_live_append(key(), &status)?, 4);
        assert!(
            store
                .browser_live_after(
                    BrowserSessionKey {
                        task_id: "other",
                        ..key()
                    },
                    0,
                    20
                )?
                .events
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn control_cas_and_idempotency_survive_store_reads() -> Result<(), Box<dyn std::error::Error>> {
        let store = SqliteStore::open_in_memory()?;
        let pause = ControlRequest {
            command: ControlCommand::Pause,
            expected_version: 0,
            idempotency_key: "pause-1".into(),
        };
        let first = store.browser_control_apply(key(), &pause, 100)?;
        assert_eq!(first.phase, ControlPhase::Paused);
        assert_eq!(first.version, 1);
        let replay = store.browser_control_apply(key(), &pause, 500)?;
        assert!(replay.replayed);
        assert_eq!(replay.version, first.version);
        let conflict = ControlRequest {
            command: ControlCommand::Stop,
            expected_version: 1,
            idempotency_key: "pause-1".into(),
        };
        assert!(matches!(
            store.browser_control_apply(key(), &conflict, 500),
            Err(BrowserStoreError::Control(
                ControlError::IdempotencyConflict
            ))
        ));
        let stale = ControlRequest {
            command: ControlCommand::Stop,
            expected_version: 0,
            idempotency_key: "stop-1".into(),
        };
        assert!(matches!(
            store.browser_control_apply(key(), &stale, 500),
            Err(BrowserStoreError::Control(
                ControlError::VersionConflict { .. }
            ))
        ));
        let takeover = ControlRequest {
            command: ControlCommand::Takeover {
                holder: "owner".into(),
                ttl_secs: Some(60),
            },
            expected_version: 1,
            idempotency_key: "takeover-1".into(),
        };
        let held = store.browser_control_apply(key(), &takeover, 100)?;
        assert_eq!(held.lease_expires_at, Some(160));
        let replay = store.browser_control_apply(key(), &takeover, 120)?;
        assert_eq!(replay.lease_expires_at, Some(160));
        assert_eq!(store.browser_control_get(key())?.version(), held.version);
        Ok(())
    }

    #[test]
    fn identity_revocation_and_delete_leave_tombstone() -> Result<(), Box<dyn std::error::Error>> {
        let store = SqliteStore::open_in_memory()?;
        let req = IdentityRegistration {
            identity_id: "id-a".into(),
            project_id: "proj-a".into(),
            origin: "https://example.com".into(),
            demand_confirmed_by: Some("human-h5".into()),
            ttl_secs: 3600,
        };
        let identity = browser_identity::register(&req, 100)
            .map_err(|e| std::io::Error::other(format!("{e:?}")))?;
        let envelope = browser_identity::envelope_for(&identity);
        let registered = store.browser_identity_register(&req, &envelope, b"sealed-only", 100)?;
        assert_eq!(registered.identity.generation, 1);
        assert_eq!(store.browser_identity_list("proj-a")?.len(), 1);
        let revoked = store
            .browser_identity_revoke("id-a")?
            .ok_or("missing identity")?;
        assert_eq!(revoked.identity.generation, 2);
        assert_eq!(revoked.identity.state, IdentityState::Revoked);
        assert!(revoked.sealed_blob.is_none());
        let deleted = store
            .browser_identity_delete("id-a")?
            .ok_or("missing identity")?;
        assert_eq!(deleted.identity.generation, 3);
        assert_eq!(deleted.identity.state, IdentityState::Deleted);
        assert!(deleted.sealed_blob.is_none());
        assert!(deleted.key_label.is_empty());
        assert!(deleted.identity.origin.is_empty());
        let persisted = store
            .browser_identity_get("id-a")?
            .ok_or("missing tombstone")?;
        assert_eq!(persisted, deleted);
        Ok(())
    }
}
