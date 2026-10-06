//! CoS run credentials and summary checkpoints (ADR 2026-10-05 D2/D3).
//! Bearer values never enter events or chat rows. The store retains only a digest.

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::io::Read;
use time::{Duration, OffsetDateTime};

use super::store::{ChatError, chat_ts, immediate, read_tx, writer};
use crate::store::SqliteStore;

pub const CHAT_CHECKPOINT_MAX_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CosRunIdentity {
    pub thread_id: String,
    pub run_id: String,
}

#[derive(Debug, thiserror::Error)]
pub enum CosRunCredentialError {
    #[error(transparent)]
    Chat(#[from] ChatError),
    #[error("unknown CoS run credential")]
    Unknown,
    #[error("expired CoS run credential")]
    Expired,
    #[error("revoked CoS run credential")]
    Revoked,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
pub struct ChatCheckpointSaved {
    pub thread_id: String,
    pub summary_through_seq: u64,
}

fn token_hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

pub(crate) fn revoke_conn(conn: &Connection, run_id: &str, at: &str) -> Result<(), ChatError> {
    conn.execute(
        "UPDATE cos_run_credentials SET revoked_at=?2 WHERE run_id=?1 AND revoked_at IS NULL",
        params![run_id, at],
    )?;
    Ok(())
}

impl SqliteStore {
    /// Issue a fresh 256-bit bearer credential for a live run. The caller owns the plaintext;
    /// this method never writes it to a log, event, or database row.
    pub fn cos_run_credential_issue(
        &self,
        thread_id: &str,
        run_id: &str,
        ttl: Duration,
    ) -> Result<String, ChatError> {
        self.cos_run_credential_issue_at(thread_id, run_id, ttl, OffsetDateTime::now_utc())
    }

    /// Clock-injected issuance for deterministic tests and controlled daemon scheduling.
    pub fn cos_run_credential_issue_at(
        &self,
        thread_id: &str,
        run_id: &str,
        ttl: Duration,
        now: OffsetDateTime,
    ) -> Result<String, ChatError> {
        if ttl < Duration::milliseconds(1) {
            return Err(ChatError::Invalid(
                "credential ttl must be at least one millisecond".into(),
            ));
        }
        let expiry = now
            .checked_add(ttl)
            .ok_or_else(|| ChatError::Invalid("credential ttl overflows time".into()))?;
        let mut random = [0_u8; 32];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut source| source.read_exact(&mut random))
            .map_err(|_| {
                ChatError::Store(crate::store::StoreError::Invalid(
                    "OS credential entropy unavailable".into(),
                ))
            })?;
        let mut token = String::with_capacity(64);
        for byte in random {
            use std::fmt::Write;
            write!(&mut token, "{byte:02x}").map_err(|_| {
                ChatError::Store(crate::store::StoreError::Invalid(
                    "credential encoding failed".into(),
                ))
            })?;
        }
        let hash = token_hash(&token);
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let live: bool = tx
            .query_row(
                "SELECT state IN ('running','stopping') FROM chat_runs WHERE run_id=?1 AND thread_id=?2",
                params![run_id, thread_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| ChatError::not_found("live run", run_id))?;
        if !live {
            return Err(ChatError::Conflict(format!("run {run_id} is finished")));
        }
        let existing: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM cos_run_credentials WHERE run_id=?1)",
            params![run_id],
            |row| row.get(0),
        )?;
        if existing {
            return Err(ChatError::Conflict(format!(
                "run {run_id} already has a credential"
            )));
        }
        tx.execute(
            "INSERT INTO cos_run_credentials(token_hash,thread_id,run_id,issued_at,expires_at) \
             VALUES(?1,?2,?3,?4,?5)",
            params![hash, thread_id, run_id, chat_ts(now), chat_ts(expiry)],
        )?;
        tx.commit()?;
        Ok(token)
    }

    /// Resolve bearer identity without accepting actor or scope fields from a request.
    pub fn cos_run_credential_verify(
        &self,
        token: &str,
        now: OffsetDateTime,
    ) -> Result<CosRunIdentity, CosRunCredentialError> {
        let hash = token_hash(token);
        let row: Option<(String, String, String, Option<String>, String)> =
            read_tx(self, |conn| {
                conn.query_row(
                    "SELECT c.thread_id,c.run_id,c.expires_at,c.revoked_at,r.state \
                 FROM cos_run_credentials c JOIN chat_runs r ON r.run_id=c.run_id \
                 AND r.thread_id=c.thread_id \
                 WHERE c.token_hash=?1",
                    params![hash],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )
                .optional()
                .map_err(ChatError::from)
            })?;
        let Some((thread_id, run_id, expires_at, revoked_at, state)) = row else {
            return Err(CosRunCredentialError::Unknown);
        };
        if revoked_at.is_some() || !matches!(state.as_str(), "running" | "stopping") {
            return Err(CosRunCredentialError::Revoked);
        }
        if chat_ts(now) >= expires_at {
            return Err(CosRunCredentialError::Expired);
        }
        Ok(CosRunIdentity { thread_id, run_id })
    }

    /// Revoke a credential explicitly, for cancellation or a failed worker launch.
    pub fn cos_run_credential_revoke(
        &self,
        run_id: &str,
        now: OffsetDateTime,
    ) -> Result<(), ChatError> {
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        revoke_conn(&tx, run_id, &chat_ts(now))?;
        tx.commit()?;
        Ok(())
    }

    /// Save a CoS summary only for the current live run. The run's input message is the
    /// highest sequence proven delivered to it by the durable run record.
    pub fn chat_thread_checkpoint(
        &self,
        thread_id: &str,
        run_id: &str,
        summary: &str,
        through_seq: u64,
        expected_summary_through_seq: u64,
    ) -> Result<ChatCheckpointSaved, ChatError> {
        self.chat_thread_checkpoint_at(
            thread_id,
            run_id,
            summary,
            through_seq,
            expected_summary_through_seq,
            OffsetDateTime::now_utc(),
        )
    }

    /// Clock-injected checkpoint variant for deterministic tests.
    pub fn chat_thread_checkpoint_at(
        &self,
        thread_id: &str,
        run_id: &str,
        summary: &str,
        through_seq: u64,
        expected_summary_through_seq: u64,
        now: OffsetDateTime,
    ) -> Result<ChatCheckpointSaved, ChatError> {
        if summary.len() > CHAT_CHECKPOINT_MAX_BYTES {
            return Err(ChatError::TooLarge("summary exceeds 32 KiB".into()));
        }
        let through = i64::try_from(through_seq)
            .map_err(|_| ChatError::Invalid("through_seq is out of range".into()))?;
        let expected = i64::try_from(expected_summary_through_seq).map_err(|_| {
            ChatError::Invalid("expected_summary_through_seq is out of range".into())
        })?;
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let delivered: Option<(i64, i64)> = tx
            .query_row(
                "SELECT m.seq,t.summary_through_seq FROM chat_runs r \
             JOIN chat_messages m ON m.id=r.input_message_id \
             JOIN chat_threads t ON t.id=r.thread_id \
             WHERE r.run_id=?1 AND r.thread_id=?2 AND r.state IN ('running','stopping') \
             AND m.thread_id=r.thread_id AND NOT EXISTS \
             (SELECT 1 FROM chat_runs current WHERE current.thread_id=r.thread_id \
              AND current.run_id<>r.run_id AND current.state IN ('running','stopping'))",
                params![run_id, thread_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (delivered, current_summary_seq) = delivered.ok_or_else(|| {
            ChatError::Conflict(format!(
                "run {run_id} is not the current run for thread {thread_id}"
            ))
        })?;
        if current_summary_seq != expected {
            return Err(ChatError::Conflict("summary cursor changed".into()));
        }
        if through > delivered {
            return Err(ChatError::Invalid(format!(
                "through_seq {through_seq} exceeds delivered seq {delivered}"
            )));
        }
        let skipped_queued: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_messages WHERE thread_id=?1 AND role='user' \
             AND state='queued' AND seq<=?2)",
            params![thread_id, through],
            |row| row.get(0),
        )?;
        if skipped_queued {
            return Err(ChatError::Invalid(
                "through_seq includes a user message not delivered to a run".into(),
            ));
        }
        let changed = tx.execute(
            "UPDATE chat_threads SET summary=?3,summary_through_seq=?4,updated_at=?5 \
             WHERE id=?1 AND summary_through_seq=?2 AND summary_through_seq<=?4",
            params![thread_id, expected, summary, through, chat_ts(now)],
        )?;
        if changed == 0 {
            return Err(ChatError::Conflict(
                "summary cursor changed or regressed".into(),
            ));
        }
        tx.commit()?;
        Ok(ChatCheckpointSaved {
            thread_id: thread_id.into(),
            summary_through_seq: through_seq,
        })
    }
}
