//! ADR 2026-10-27-relayed-instructions 付記 A2: an escalated item gets a second CoS judgment when
//! a relayed instruction arrives after the triage run that escalated it started (that run's prompt
//! could not hold it). The state is `cos_inbox_items.message_id` and `chat_messages.seq` only: the
//! re-judging claim moves the item's input message past the relay, so one relay hands an item over
//! once.
use rusqlite::params;
use serde::Serialize;
use time::OffsetDateTime;

use super::ChatMessage;
use super::store::{
    ChatError, MESSAGE_SELECT, chat_ts, immediate, message_from_raw, message_raw, relay_expired,
    writer,
};
use super::triage::{COS_TRIAGE_CLAIM_PREFIX, CosTriageClaim, bind_claim_tx};
use crate::store::SqliteStore;

/// The second line of a re-judging run's input message.
pub const COS_RELAY_REJUDGE_PREFIX: &str = "取り次ぎによる再判断: ";

/// An escalated item and the unexpired relays of its thread that its triage run did not see.
#[derive(Debug, Clone)]
pub struct RelayRejudgeCandidate {
    pub item_id: String,
    pub thread_id: String,
    pub source_kind: String,
    /// The human inbox item id (the caller maps it to tasks for the relay scope).
    pub source_key: String,
    /// Relays posted after the input message of the run that judged the item, oldest first.
    pub relays: Vec<ChatMessage>,
}

/// One item handed to CoS again, with the relays whose scope covers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RelayRejudge {
    pub item_id: String,
    pub source_key: String,
    pub relay_message_ids: Vec<String>,
}

impl SqliteStore {
    /// A2 R1: `escalated` items judged by a triage run (not direct escalations) whose thread has an
    /// uncancelled, unexpired relay newer than that run's input message.
    pub fn cos_triage_relay_candidates(
        &self,
        now: OffsetDateTime,
    ) -> Result<Vec<RelayRejudgeCandidate>, ChatError> {
        let conn = writer(self)?;
        let rows: Vec<(String, String, String, String, i64)> = {
            let mut stmt = conn.prepare(
                "SELECT i.id,i.thread_id,i.source_kind,i.source_key,j.seq FROM cos_inbox_items i \
                 JOIN chat_messages j ON j.id=i.message_id AND j.thread_id=i.thread_id \
                 WHERE i.state='escalated' AND i.message_id!='' AND j.role='system' \
                 AND EXISTS(SELECT 1 FROM chat_messages r WHERE r.thread_id=i.thread_id \
                   AND r.role='user' AND r.state!='cancelled' \
                   AND json_type(r.metadata_json,'$.relay')='object' AND r.seq>j.seq) \
                 ORDER BY i.created_at,i.id",
            )?;
            stmt.query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?
            .collect::<Result<_, _>>()?
        };
        let mut out = Vec::new();
        for (item_id, thread_id, source_kind, source_key, judged_seq) in rows {
            let raws = {
                let mut stmt = conn.prepare(&format!(
                    "{MESSAGE_SELECT} WHERE thread_id=?1 AND role='user' AND state!='cancelled' \
                     AND json_type(metadata_json,'$.relay')='object' AND seq>?2 ORDER BY seq"
                ))?;
                stmt.query_map(params![thread_id, judged_seq], message_raw)?
                    .collect::<Result<Vec<_>, _>>()?
            };
            let mut relays = Vec::new();
            for raw in raws {
                let message = message_from_raw(&conn, raw)?;
                if message
                    .relay
                    .as_ref()
                    .is_some_and(|relay| !relay_expired(relay, now))
                {
                    relays.push(message);
                }
            }
            if !relays.is_empty() {
                out.push(RelayRejudgeCandidate {
                    item_id,
                    thread_id,
                    source_kind,
                    source_key,
                    relays,
                });
            }
        }
        Ok(out)
    }

    /// A2 R1/R2: bind the still-escalated items of `rejudge` in `thread_id` to one system message and
    /// one chat run, as a triage claim does. `None` while a run of the thread is live (the running
    /// triage run finishes first) or when no item is still escalated.
    pub fn cos_triage_relay_claim(
        &self,
        thread_id: &str,
        run_id: &str,
        rejudge: &[RelayRejudge],
        now: OffsetDateTime,
    ) -> Result<Option<CosTriageClaim>, ChatError> {
        let at = chat_ts(now);
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let active: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_runs WHERE thread_id=?1 AND state IN ('running','stopping'))",
            [thread_id],
            |r| r.get(0),
        )?;
        if active {
            return Ok(None);
        }
        let mut items = Vec::new();
        for r in rejudge {
            let escalated: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM cos_inbox_items WHERE id=?1 AND thread_id=?2 AND state='escalated')",
                params![r.item_id, thread_id],
                |row| row.get(0),
            )?;
            if escalated && !r.relay_message_ids.is_empty() {
                items.push(r.clone());
            }
        }
        if items.is_empty() {
            return Ok(None);
        }
        let ids: Vec<String> = items.iter().map(|r| r.item_id.clone()).collect();
        let body = format!(
            "{COS_TRIAGE_CLAIM_PREFIX}{}\n{COS_RELAY_REJUDGE_PREFIX}{}",
            serde_json::to_string(&ids)?,
            serde_json::to_string(&items)?
        );
        let message_id = bind_claim_tx(&tx, thread_id, run_id, &ids, &body, &at)?;
        tx.commit()?;
        Ok(Some(CosTriageClaim {
            thread_id: thread_id.to_owned(),
            message_id,
            run_id: run_id.to_owned(),
            item_ids: ids,
        }))
    }
}
