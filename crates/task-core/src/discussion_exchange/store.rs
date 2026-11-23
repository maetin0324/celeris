//! The store side of [`super`]: the person's share and its revocation (`chat_events`, D2.1), the snapshot read (D1.4),
//! and receiving an import or a propose as a `discuss` message (D3・D4.2・D4.3). Reads and writes go through the
//! existing chat functions and tables; nothing new is stored but two `chat_events` types and
//! `chat_messages.metadata_json.exchange`.

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use time::{Duration, OffsetDateTime};

use super::{
    ActiveShare, DiscussionSnapshot, ExchangeDocument, ExchangeRefusal, ExchangeShareRequest,
    ExchangeShareRevoked, ExchangeShared, FORBIDDEN, IDEMPOTENCY_CONFLICT, INVALID, ImportRequest,
    Inbound, InboundContext, METADATA_KEY, MessageFacts, ProposeRequest, SECRET, SHARE_DAYS_MAX,
    SHARE_MESSAGES_MAX, SNAPSHOT_ITEMS_DEFAULT, ShareEvent, SharedRevision, SnapshotInput,
    THREAD_KIND_NOT_PROJECT, THREAD_PROJECT_MISMATCH, active_shares, build_snapshot, check_import,
    check_propose, parse_ts,
};
use crate::chat::store::{ChatError, chat_ts, immediate, read_tx, thread_require, writer};
use crate::chat::{
    CHAT_EVENT_EXCHANGE_SHARE_REVOKED, CHAT_EVENT_EXCHANGE_SHARED, ChatDiscussionMode, ChatMessage,
    ChatPostMessageRequest, ChatSendMode, ChatThreadKind,
};
use crate::chat_citation::{ChatCitationState, citation_state, cited_message_conn};
use crate::project_proposal::sha256_hex;
use crate::store::SqliteStore;
use crate::strategy::SourceRevision;

/// D1.2: the thread of the three tools — by `thread_id`, by `project_id` (the project's fixed CoS thread), or both
/// (they must agree). Only a `project` thread.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExchangeTarget {
    pub project_id: Option<String>,
    pub thread_id: Option<String>,
}

/// D1.4 / D4.1: what to read. `share_id` limits the snapshot to one share (the export).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnapshotQuery {
    pub target: ExchangeTarget,
    pub after_seq: u64,
    pub limit: Option<usize>,
    pub share_id: Option<i64>,
}

/// A received import / propose: the message (new or the one already received with the key).
#[derive(Debug, Clone, PartialEq)]
pub struct Received {
    pub message: ChatMessage,
    pub project_id: String,
    pub content_hash: String,
    /// The same key and body were received before; nothing new was written.
    pub duplicate: bool,
}

/// Errors of the store side: a refusal (403 / 409 / 422) or a chat store error.
#[derive(Debug, thiserror::Error)]
pub enum ExchangeError {
    #[error(transparent)]
    Refused(#[from] ExchangeRefusal),
    #[error(transparent)]
    Chat(#[from] ChatError),
}

impl From<rusqlite::Error> for ExchangeError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Chat(e.into())
    }
}

impl From<serde_json::Error> for ExchangeError {
    fn from(e: serde_json::Error) -> Self {
        Self::Chat(e.into())
    }
}

impl ExchangeError {
    pub fn http_status(&self) -> u16 {
        match self {
            Self::Refused(r) => r.http_status(),
            Self::Chat(e) => e.http_status(),
        }
    }
}

fn refuse(code: &'static str, detail: impl Into<String>) -> ExchangeError {
    ExchangeError::Refused(ExchangeRefusal::new(code, detail))
}

/// D1.2: resolves the target to `(thread_id, project_id)`.
fn resolve(conn: &Connection, target: &ExchangeTarget) -> Result<(String, String), ExchangeError> {
    let thread_id = match (&target.thread_id, &target.project_id) {
        (Some(t), _) => t.clone(),
        (None, Some(p)) => conn
            .query_row(
                "SELECT id FROM chat_threads WHERE project_id=?1 AND kind='project'",
                params![p],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| ChatError::not_found("project thread", p))?,
        (None, None) => return Err(refuse(INVALID, "give project_id or thread_id")),
    };
    let thread = thread_require(conn, &thread_id)?;
    let project = match (thread.kind, thread.project_id) {
        (ChatThreadKind::Project, Some(p)) => p,
        _ => {
            return Err(refuse(
                THREAD_KIND_NOT_PROJECT,
                format!("thread {thread_id} is not a project thread"),
            ));
        }
    };
    if target.project_id.as_ref().is_some_and(|p| *p != project) {
        return Err(refuse(
            THREAD_PROJECT_MISMATCH,
            format!("thread {thread_id} is not of the given project"),
        ));
    }
    Ok((thread_id, project))
}

/// The project's `Project.request` sha256 and approved Roadmap version now.
fn source_revision(conn: &Connection, project: &str) -> Result<SourceRevision, ExchangeError> {
    let request: String = conn
        .query_row(
            "SELECT request FROM projects WHERE id=?1",
            params![project],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or_default();
    let roadmap_version = conn
        .query_row(
            "SELECT version FROM project_roadmap_versions WHERE project_id=?1 AND status='approved'",
            params![project],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    Ok(SourceRevision {
        request_sha256: sha256_hex(&request),
        roadmap_version,
    })
}

fn share_events(conn: &Connection, thread_id: &str) -> Result<Vec<ShareEvent>, ExchangeError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id,type,payload_json,created_at FROM chat_events \
         WHERE thread_id=?1 AND type IN (?2,?3) ORDER BY id",
    )?;
    let rows = stmt.query_map(
        params![
            thread_id,
            CHAT_EVENT_EXCHANGE_SHARED,
            CHAT_EVENT_EXCHANGE_SHARE_REVOKED
        ],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        },
    )?;
    let mut out = Vec::new();
    for row in rows {
        let (id, ty, payload, at) = row?;
        out.push(if ty == CHAT_EVENT_EXCHANGE_SHARED {
            ShareEvent::Shared {
                id,
                at,
                record: serde_json::from_str(&payload)?,
            }
        } else {
            ShareEvent::Revoked(serde_json::from_str(&payload)?)
        });
    }
    Ok(out)
}

/// The message as it is now (`cited_message_conn` for revision / sha256 / retracted, plus its text and record).
fn message_facts(
    conn: &Connection,
    thread_id: &str,
    message_id: &str,
) -> Result<Option<MessageFacts>, ExchangeError> {
    let Some(cited) = cited_message_conn(conn, thread_id, message_id)? else {
        return Ok(None);
    };
    let (role, text, meta, created_at): (String, String, String, String) = conn.query_row(
        "SELECT role,text,metadata_json,created_at FROM chat_messages WHERE id=?1 AND thread_id=?2",
        params![message_id, thread_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    let meta: Value = serde_json::from_str(&meta)?;
    Ok(Some(MessageFacts {
        message_id: message_id.to_owned(),
        seq: cited.seq,
        revision: cited.revision,
        sha256: cited.sha256,
        retracted: cited.retracted,
        role: serde_json::from_value(Value::String(role))?,
        author: meta
            .get("author")
            .and_then(Value::as_str)
            .map(str::to_owned),
        text,
        created_at,
        exchange: meta.get(METADATA_KEY).filter(|v| !v.is_null()).cloned(),
    }))
}

/// Appends one share record to `chat_events` (an audit record read by [`share_events`]; not an SSE event, so
/// `chat_events_page` skips its type) and returns its id.
fn append_record(
    tx: &Connection,
    thread_id: &str,
    ty: &str,
    payload: &impl serde::Serialize,
    at: &str,
) -> Result<i64, ExchangeError> {
    tx.execute(
        "INSERT INTO chat_events(thread_id,run_id,message_id,type,payload_json,created_at) \
         VALUES(?1,NULL,NULL,?2,?3,?4)",
        params![thread_id, ty, serde_json::to_string(payload)?, at],
    )?;
    Ok(tx.last_insert_rowid())
}

impl SqliteStore {
    /// D2.1: `by` (a person, `human:<name>`) shares messages of a project thread with external LLMs, fixed at their
    /// current revision and body. A retracted message or one whose body looks like a secret is refused (D2.4).
    /// Returns the share id.
    pub fn chat_exchange_share(
        &self,
        thread_id: &str,
        by: &str,
        req: &ExchangeShareRequest,
        now: OffsetDateTime,
    ) -> Result<i64, ExchangeError> {
        if !crate::is_human_principal(by) {
            return Err(refuse(FORBIDDEN, "only a person shares messages"));
        }
        let mut ids = req.message_ids.clone();
        ids.sort();
        ids.dedup();
        if ids.is_empty() || ids.len() != req.message_ids.len() || ids.len() > SHARE_MESSAGES_MAX {
            return Err(refuse(
                INVALID,
                format!("message_ids are 1-{SHARE_MESSAGES_MAX} distinct ids"),
            ));
        }
        let expires = parse_ts(&req.expires_at)
            .filter(|x| *x > now && *x <= now + Duration::days(SHARE_DAYS_MAX))
            .ok_or_else(|| {
                refuse(
                    INVALID,
                    format!("expires_at is RFC 3339, after now and within {SHARE_DAYS_MAX} days"),
                )
            })?;
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        resolve(
            &tx,
            &ExchangeTarget {
                project_id: None,
                thread_id: Some(thread_id.to_owned()),
            },
        )?;
        let mut revisions = Vec::new();
        for id in &req.message_ids {
            let m = message_facts(&tx, thread_id, id)?
                .ok_or_else(|| ChatError::not_found("message", id))?;
            if m.retracted {
                return Err(refuse(INVALID, format!("message {id} is retracted")));
            }
            if crate::strategy::envelope::contains_secret(&m.text) {
                return Err(refuse(
                    SECRET,
                    format!("message {id} looks like it holds a secret; edit it before sharing"),
                ));
            }
            revisions.push(SharedRevision {
                message_id: id.clone(),
                revision: m.revision,
                sha256: m.sha256,
            });
        }
        let record = ExchangeShared {
            by: by.to_owned(),
            message_ids: req.message_ids.clone(),
            revisions,
            scope: req.scope,
            include_confidential: req.include_confidential,
            expires_at: chat_ts(expires),
        };
        let id = append_record(
            &tx,
            thread_id,
            CHAT_EVENT_EXCHANGE_SHARED,
            &record,
            &chat_ts(now),
        )?;
        tx.commit().map_err(ChatError::from)?;
        Ok(id)
    }

    /// D2.1: `by` (a person) revokes share `share_id` of the thread.
    pub fn chat_exchange_share_revoke(
        &self,
        thread_id: &str,
        share_id: i64,
        by: &str,
        now: OffsetDateTime,
    ) -> Result<(), ExchangeError> {
        if !crate::is_human_principal(by) {
            return Err(refuse(FORBIDDEN, "only a person revokes a share"));
        }
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let events = share_events(&tx, thread_id)?;
        let shared = events
            .iter()
            .any(|e| matches!(e, ShareEvent::Shared { id, .. } if *id == share_id));
        if !shared {
            return Err(ChatError::not_found("share", &share_id.to_string()).into());
        }
        if events
            .iter()
            .any(|e| matches!(e, ShareEvent::Revoked(r) if r.share_event_id == share_id))
        {
            return Err(ChatError::Conflict(format!("share {share_id} is already revoked")).into());
        }
        let record = ExchangeShareRevoked {
            by: by.to_owned(),
            share_event_id: share_id,
        };
        append_record(
            &tx,
            thread_id,
            CHAT_EVENT_EXCHANGE_SHARE_REVOKED,
            &record,
            &chat_ts(now),
        )?;
        tx.commit().map_err(ChatError::from)?;
        Ok(())
    }

    /// D1.4・D2: the snapshot `viewer` (`mcp:<client_id>`, or the exporting `human:<name>`) may read now. `documents`
    /// are the summary / strategy files of the thread's CoS replies, read by the caller from the workspace.
    pub fn chat_exchange_snapshot(
        &self,
        query: &SnapshotQuery,
        viewer: &str,
        documents: &[ExchangeDocument],
        now: OffsetDateTime,
    ) -> Result<DiscussionSnapshot, ExchangeError> {
        let input = read_tx(self, |conn| {
            Ok(snapshot_input(conn, query, viewer, documents, now))
        })??;
        Ok(build_snapshot(&input))
    }

    /// D4.2・D4.3: a person (`caller`, `human:<name>`) brings in one summary or quote they picked. It becomes a
    /// `role=user`, `discuss` message by the caller with `metadata_json.exchange`; the same key and body return the
    /// message already received, a different body is `R7:idempotency_conflict`.
    pub fn chat_exchange_import(
        &self,
        thread_id: &str,
        caller: &str,
        req: &ImportRequest,
        now: OffsetDateTime,
    ) -> Result<Received, ExchangeError> {
        if !crate::is_human_principal(caller) {
            return Err(refuse(FORBIDDEN, "only a person imports"));
        }
        let target = ExchangeTarget {
            project_id: None,
            thread_id: Some(thread_id.to_owned()),
        };
        let ctx = self.inbound_context(&target, caller, &[], &req.client_message_id, now)?;
        let inbound = check_import(req, &ctx)?;
        self.receive(&ctx, &inbound, now)
    }

    /// D1.6・D3: an MCP client (`caller`, `mcp:<client_id>`, with its scopes) records one draft. The key is
    /// `mcp:<client_id>:<idempotency_key>`.
    pub fn chat_exchange_propose(
        &self,
        caller: &str,
        caller_scopes: &[String],
        req: &ProposeRequest,
        now: OffsetDateTime,
    ) -> Result<Received, ExchangeError> {
        let target = ExchangeTarget {
            project_id: req.project_id.clone(),
            thread_id: req.thread_id.clone(),
        };
        let key = format!("{caller}:{}", req.idempotency_key);
        let ctx = self.inbound_context(&target, caller, caller_scopes, &key, now)?;
        let inbound = check_propose(req, &ctx)?;
        self.receive(&ctx, &inbound, now)
    }

    /// The thread, project, source revision and the `content_hash` already received with `key` (D4.3: from the
    /// message's `client_message_id` and `metadata_json.exchange.content_hash`; a message with the key but no
    /// exchange record never matches).
    fn inbound_context(
        &self,
        target: &ExchangeTarget,
        caller: &str,
        caller_scopes: &[String],
        key: &str,
        now: OffsetDateTime,
    ) -> Result<InboundContext, ExchangeError> {
        read_tx(self, |conn| {
            Ok((|| {
                let (thread_id, project_id) = resolve(conn, target)?;
                let source_revision = source_revision(conn, &project_id)?;
                let seen: Option<String> = conn
                    .query_row(
                        "SELECT metadata_json FROM chat_messages WHERE thread_id=?1 AND client_message_id=?2",
                        params![thread_id, key],
                        |row| row.get(0),
                    )
                    .optional()?;
                let mut idempotency = BTreeMap::new();
                if let Some(meta) = seen {
                    let meta: Value = serde_json::from_str(&meta)?;
                    let hash = meta
                        .pointer("/exchange/content_hash")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    idempotency.insert(key.to_owned(), hash.to_owned());
                }
                Ok::<_, ExchangeError>(InboundContext {
                    project_id,
                    thread_id,
                    caller: caller.to_owned(),
                    caller_scopes: caller_scopes.to_vec(),
                    idempotency,
                    source_revision,
                    now: chat_ts(now),
                })
            })())
        })?
    }

    /// Posts the checked text as a `discuss` message by its author (`chat_message_post_as_also`) and records the
    /// envelope in `metadata_json.exchange` in the same transaction.
    fn receive(
        &self,
        ctx: &InboundContext,
        inbound: &Inbound,
        now: OffsetDateTime,
    ) -> Result<Received, ExchangeError> {
        let req = ChatPostMessageRequest {
            client_message_id: inbound.client_message_id.clone(),
            text: inbound.body.clone(),
            attachment_ids: Vec::new(),
            reply_to_id: None,
            mode: ChatSendMode::Queue,
            resume_queue: false,
            discussion_mode: Some(ChatDiscussionMode::Discuss),
        };
        let metadata = serde_json::to_string(&inbound.metadata)?;
        let posted = self
            .chat_message_post_as_also(&ctx.thread_id, &req, &inbound.author, now, |tx, posted| {
                if posted.created {
                    tx.execute(
                        "UPDATE chat_messages SET metadata_json=json_set(metadata_json,'$.exchange',json(?2)) \
                         WHERE id=?1",
                        params![posted.response.message.id, metadata],
                    )?;
                }
                Ok(())
            })
            .map_err(|e| match e {
                ChatError::Conflict(detail) => {
                    ExchangeError::Refused(ExchangeRefusal {
                        code: IDEMPOTENCY_CONFLICT,
                        rules: vec![IDEMPOTENCY_CONFLICT],
                        detail,
                    })
                }
                other => other.into(),
            })?;
        Ok(Received {
            message: posted.response.message,
            project_id: ctx.project_id.clone(),
            content_hash: inbound.content_hash.clone(),
            duplicate: !posted.created,
        })
    }
}

/// Gathers [`SnapshotInput`] in one read transaction.
fn snapshot_input(
    conn: &Connection,
    query: &SnapshotQuery,
    viewer: &str,
    documents: &[ExchangeDocument],
    now: OffsetDateTime,
) -> Result<SnapshotInput, ExchangeError> {
    let (thread_id, project_id) = resolve(conn, &query.target)?;
    let mut shares: Vec<ActiveShare> = active_shares(&share_events(conn, &thread_id)?, now);
    if let Some(id) = query.share_id {
        shares.retain(|s| s.share_id == id);
    }
    let mut ids: Vec<String> = shares
        .iter()
        .flat_map(|s| s.record.message_ids.iter().cloned())
        .collect();
    ids.extend(documents.iter().map(|d| d.message_id.clone()));
    if viewer.starts_with("mcp:") {
        let mut stmt = conn.prepare_cached(
            "SELECT id FROM chat_messages WHERE thread_id=?1 AND json_extract(metadata_json,'$.author')=?2 \
             AND json_extract(metadata_json,'$.exchange') IS NOT NULL",
        )?;
        let own = stmt
            .query_map(params![thread_id, viewer], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        ids.extend(own);
    }
    let mut messages = BTreeMap::new();
    for id in ids {
        if !messages.contains_key(&id)
            && let Some(m) = message_facts(conn, &thread_id, &id)?
        {
            messages.insert(id, m);
        }
    }
    let mut docs = Vec::new();
    for d in documents {
        let mut current = true;
        for c in &d.citations {
            let now_msg = cited_message_conn(conn, &c.thread_id, &c.message_id)?;
            current &= citation_state(c, now_msg.as_ref()) == ChatCitationState::Current;
        }
        docs.push((d.clone(), current));
    }
    Ok(SnapshotInput {
        source_revision: source_revision(conn, &project_id)?,
        project_id,
        thread_id,
        generated_at: chat_ts(now),
        shares,
        messages,
        documents: docs,
        viewer: viewer.to_owned(),
        after_seq: query.after_seq,
        limit: query.limit.unwrap_or(SNAPSHOT_ITEMS_DEFAULT),
    })
}
