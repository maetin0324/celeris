//! Atomic CoS domain operations and audit records (ADR 2026-10-05 D3).
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::{Value, json};
use time::OffsetDateTime;
use ulid::Ulid;

use super::store::{append_event, chat_ts};
use super::{
    ChatActor, ChatCard, ChatCardData, ChatCardKind, ChatError, ChatEventData, ChatEventType,
};
use crate::model::{Event, TaskId};
use crate::store::SqliteStore;

/// Set by the verified run credential, never by a request body.
#[derive(Debug, Clone)]
pub struct AuditContext {
    pub actor: ChatActor,
    pub thread_id: String,
    pub run_id: String,
    pub operation_id: String,
    pub reason: String,
    pub policy_version: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, schemars::JsonSchema)]
pub struct CosOperation {
    pub id: String,
    pub thread_id: String,
    pub run_id: String,
    pub idempotency_key: String,
    pub request_hash: String,
    pub target_kind: String,
    pub target_id: String,
    pub expected_revision: Option<String>,
    pub action: String,
    pub payload: Value,
    pub reason: String,
    pub policy_version: String,
    pub state: String,
    pub result: Option<Value>,
    pub event_id: Option<i64>,
}

fn get_conn(conn: &rusqlite::Connection, id: &str) -> Result<Option<CosOperation>, ChatError> {
    let raw = conn
        .query_row(
            "SELECT id,thread_id,run_id,idempotency_key,request_hash,target_kind,target_id,\
         expected_revision,action,payload_json,reason,policy_version,state,result_json,event_id \
         FROM cos_operations WHERE id=?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    r.get::<_, String>(8)?,
                    r.get::<_, String>(9)?,
                    r.get::<_, String>(10)?,
                    r.get::<_, String>(11)?,
                    r.get::<_, String>(12)?,
                    r.get::<_, Option<String>>(13)?,
                    r.get::<_, Option<i64>>(14)?,
                ))
            },
        )
        .optional()?;
    raw.map(
        |(
            id,
            thread_id,
            run_id,
            idempotency_key,
            request_hash,
            target_kind,
            target_id,
            expected_revision,
            action,
            payload,
            reason,
            policy_version,
            state,
            result,
            event_id,
        )| {
            Ok(CosOperation {
                id,
                thread_id,
                run_id,
                idempotency_key,
                request_hash,
                target_kind,
                target_id,
                expected_revision,
                action,
                payload: serde_json::from_str(&payload)?,
                reason,
                policy_version,
                state,
                result: result.map(|v| serde_json::from_str(&v)).transpose()?,
                event_id,
            })
        },
    )
    .transpose()
}

fn audit_event(
    tx: &Transaction<'_>,
    ctx: &AuditContext,
    target_kind: &str,
    target_id: &str,
    state: &str,
) -> Result<(), ChatError> {
    // Non-task operations use their operation ID as an audit stream key. Task operations
    // join the task's existing append-only event stream.
    let operation_stream = ctx
        .operation_id
        .parse::<TaskId>()
        .map_err(|_| ChatError::Invalid("operation_id must be a ULID".into()))?;
    let task_id = match (target_kind, target_id.parse::<TaskId>()) {
        ("task", Ok(task_id)) => task_id,
        _ => operation_stream,
    };
    SqliteStore::append_event_tx(
        tx,
        task_id,
        &Event::CosOperation {
            actor: "cos".into(),
            thread_id: ctx.thread_id.clone(),
            run_id: ctx.run_id.clone(),
            operation_id: ctx.operation_id.clone(),
            reason: ctx.reason.clone(),
            policy_version: ctx.policy_version.clone(),
            state: state.into(),
            target_kind: target_kind.into(),
            target_id: target_id.into(),
        },
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn insert_record(
    tx: &Transaction<'_>,
    ctx: &AuditContext,
    key: &str,
    hash: &str,
    target_kind: &str,
    target_id: &str,
    revision: Option<&str>,
    action: &str,
    payload: &Value,
    state: &str,
    result: &Value,
    now: &str,
) -> Result<(), ChatError> {
    tx.execute("INSERT INTO cos_operations(id,thread_id,run_id,idempotency_key,request_hash,\
        target_kind,target_id,expected_revision,action,payload_json,reason,policy_version,\
        state,result_json,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?15)",
        params![ctx.operation_id,ctx.thread_id,ctx.run_id,key,hash,target_kind,target_id,
            revision,action,payload.to_string(),ctx.reason,ctx.policy_version,state,result.to_string(),now])?;
    Ok(())
}

impl SqliteStore {
    /// Persist a C-class operation before invoking an external side effect. Duplicate requests
    /// return the existing receipt and must never be interpreted as permission to replay it.
    #[allow(clippy::too_many_arguments)]
    pub fn cos_operation_begin_external(
        &self,
        ctx: &AuditContext,
        key: &str,
        hash: &str,
        target_kind: &str,
        target_id: &str,
        revision: Option<&str>,
        action: &str,
        payload: &Value,
        now: OffsetDateTime,
    ) -> Result<(CosOperation, bool), ChatError> {
        if ctx.actor != ChatActor::Cos
            || ctx.thread_id.trim().is_empty()
            || ctx.run_id.trim().is_empty()
            || ctx.operation_id.parse::<Ulid>().is_err()
            || ctx.reason.trim().is_empty()
            || ctx.policy_version.trim().is_empty()
            || key.trim().is_empty()
            || hash.trim().is_empty()
            || target_kind.trim().is_empty()
            || target_id.trim().is_empty()
            || action.trim().is_empty()
        {
            return Err(ChatError::Invalid(
                "invalid external operation audit context".into(),
            ));
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some((id, prior_hash)) = tx.query_row(
            "SELECT id,request_hash FROM cos_operations WHERE thread_id=?1 AND idempotency_key=?2",
            params![ctx.thread_id, key], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)),
        ).optional()? {
            if prior_hash != hash { return Err(ChatError::Conflict("idempotency key has a different request hash".into())); }
            let op = get_conn(&tx, &id)?.ok_or_else(|| ChatError::not_found("operation", &id))?;
            tx.commit()?;
            return Ok((op, false));
        }
        let stamp = chat_ts(now);
        insert_record(
            &tx,
            ctx,
            key,
            hash,
            target_kind,
            target_id,
            revision,
            action,
            payload,
            "pending",
            &Value::Null,
            &stamp,
        )?;
        audit_event(&tx, ctx, target_kind, target_id, "pending")?;
        let card = ChatCard {
            kind: ChatCardKind::Operation,
            id: ctx.operation_id.clone(),
            title: action.into(),
            state: "pending".into(),
            href: format!("/cos/operations/{}", ctx.operation_id),
            actor: ChatActor::Cos,
            reason: Some(ctx.reason.clone()),
            operation_id: Some(ctx.operation_id.clone()),
        };
        let event_id = append_event(
            &tx,
            &ctx.thread_id,
            Some(&ctx.run_id),
            None,
            ChatEventType::Card,
            &ChatEventData::Card(ChatCardData { card }),
            &stamp,
        )?;
        tx.execute(
            "UPDATE cos_operations SET event_id=?2 WHERE id=?1",
            params![ctx.operation_id, event_id],
        )?;
        let op = get_conn(&tx, &ctx.operation_id)?
            .ok_or_else(|| ChatError::not_found("operation", &ctx.operation_id))?;
        tx.commit()?;
        Ok((op, true))
    }

    /// Finish only a pending external operation. A repeated finish returns the settled receipt.
    pub fn cos_operation_finish_external(
        &self,
        ctx: &AuditContext,
        result: &Value,
        now: OffsetDateTime,
    ) -> Result<CosOperation, ChatError> {
        self.settle_external(ctx, "applied", result, now)
    }

    /// Settle a pending external operation as `rejected` when the effect is known not to have
    /// happened (the external system refused it before changing anything: an etag mismatch, a
    /// busy branch, a missing page). An unknown outcome must stay pending for remediation instead.
    pub fn cos_operation_fail_external(
        &self,
        ctx: &AuditContext,
        result: &Value,
        now: OffsetDateTime,
    ) -> Result<CosOperation, ChatError> {
        self.settle_external(ctx, "rejected", result, now)
    }

    fn settle_external(
        &self,
        ctx: &AuditContext,
        state: &str,
        result: &Value,
        now: OffsetDateTime,
    ) -> Result<CosOperation, ChatError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut op = get_conn(&tx, &ctx.operation_id)?
            .ok_or_else(|| ChatError::not_found("operation", &ctx.operation_id))?;
        if op.state != "pending" {
            tx.commit()?;
            return Ok(op);
        }
        let stamp = chat_ts(now);
        tx.execute("UPDATE cos_operations SET state=?4,result_json=?2,updated_at=?3 WHERE id=?1 AND state='pending'",
            params![ctx.operation_id,result.to_string(),stamp,state])?;
        audit_event(&tx, ctx, &op.target_kind, &op.target_id, state)?;
        let card = ChatCard {
            kind: ChatCardKind::Operation,
            id: ctx.operation_id.clone(),
            title: op.action.clone(),
            state: state.into(),
            href: format!("/cos/operations/{}", ctx.operation_id),
            actor: ChatActor::Cos,
            reason: Some(ctx.reason.clone()),
            operation_id: Some(ctx.operation_id.clone()),
        };
        let event_id = append_event(
            &tx,
            &ctx.thread_id,
            Some(&ctx.run_id),
            None,
            ChatEventType::Card,
            &ChatEventData::Card(ChatCardData { card }),
            &stamp,
        )?;
        tx.execute(
            "UPDATE cos_operations SET event_id=?2 WHERE id=?1",
            params![ctx.operation_id, event_id],
        )?;
        op = get_conn(&tx, &ctx.operation_id)?
            .ok_or_else(|| ChatError::not_found("operation", &ctx.operation_id))?;
        tx.commit()?;
        Ok(op)
    }

    /// Mark pending external operations older than the supplied cutoff for human remediation.
    /// Caller injects the clock and invokes this once during daemon startup.
    pub fn cos_operation_remediate_stale(
        &self,
        cutoff: OffsetDateTime,
    ) -> Result<usize, ChatError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let cutoff = chat_ts(cutoff);
        let mut stmt = tx.prepare("SELECT id,thread_id,run_id,target_kind,target_id,reason,policy_version FROM cos_operations WHERE state='pending' AND updated_at < ?1")?;
        let rows = stmt
            .query_map([&cutoff], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        for (id, thread, run, kind, target, reason, policy) in &rows {
            tx.execute("UPDATE cos_operations SET state='needs_remediation',updated_at=?2 WHERE id=?1 AND state='pending'", params![id,cutoff])?;
            let ctx = AuditContext {
                actor: ChatActor::Cos,
                thread_id: thread.clone(),
                run_id: run.clone(),
                operation_id: id.clone(),
                reason: reason.clone(),
                policy_version: policy.clone(),
            };
            audit_event(&tx, &ctx, kind, target, "needs_remediation")?;
        }
        let n = rows.len();
        tx.commit()?;
        Ok(n)
    }

    /// A pending operation may have reached an external system before the daemon disappeared.
    /// Recovery must wait for a human to establish its outcome instead of replaying it.
    pub fn cos_operation_pending_for_run(&self, run_id: &str) -> Result<bool, ChatError> {
        let conn = self.lock()?;
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM cos_operations WHERE run_id=?1 AND state='pending')",
            [run_id],
            |row| row.get(0),
        )
        .map_err(Into::into)
    }

    fn cos_operations_for_run_state(
        &self,
        run_id: &str,
        state: &str,
    ) -> Result<Vec<CosOperation>, ChatError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id FROM cos_operations WHERE run_id=?1 AND state=?2 ORDER BY created_at,id",
        )?;
        let ids = stmt
            .query_map(params![run_id, state], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids.into_iter()
            .map(|id| get_conn(&conn, &id)?.ok_or_else(|| ChatError::not_found("operation", &id)))
            .collect()
    }

    /// Applied operation receipts from a disappeared run, ordered for a
    /// continuation worker. The idempotency key remains unique per thread.
    pub fn cos_operation_applied_for_run(
        &self,
        run_id: &str,
    ) -> Result<Vec<CosOperation>, ChatError> {
        self.cos_operations_for_run_state(run_id, "applied")
    }

    /// Pending operation records requiring human review after a worker disappears.
    pub fn cos_operation_pending_details_for_run(
        &self,
        run_id: &str,
    ) -> Result<Vec<CosOperation>, ChatError> {
        self.cos_operations_for_run_state(run_id, "pending")
    }

    pub fn cos_operation_get(&self, id: &str) -> Result<Option<CosOperation>, ChatError> {
        let conn = self.lock()?;
        get_conn(&conn, id)
    }

    /// The operation recorded for `(thread_id, idempotency_key)`, if any.
    pub fn cos_operation_find(
        &self,
        thread_id: &str,
        idempotency_key: &str,
    ) -> Result<Option<CosOperation>, ChatError> {
        let conn = self.lock()?;
        let id = conn
            .query_row(
                "SELECT id FROM cos_operations WHERE thread_id=?1 AND idempotency_key=?2",
                params![thread_id, idempotency_key],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        match id {
            Some(id) => get_conn(&conn, &id),
            None => Ok(None),
        }
    }

    /// The closure must perform all domain writes through `tx`; any error rolls them back.
    /// A failed application is recorded in a new transaction after the rollback.
    #[allow(clippy::too_many_arguments)]
    pub fn cos_operation_apply<F>(
        &self,
        ctx: &AuditContext,
        idempotency_key: &str,
        request_hash: &str,
        target_kind: &str,
        target_id: &str,
        expected_revision: Option<&str>,
        action: &str,
        payload: &Value,
        apply: F,
    ) -> Result<CosOperation, ChatError>
    where
        F: FnOnce(&Transaction<'_>, &AuditContext) -> Result<Value, ChatError>,
    {
        let mut conn = self.lock()?;
        let now = chat_ts(OffsetDateTime::now_utc());
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some((id, hash)) = tx.query_row(
            "SELECT id,request_hash FROM cos_operations WHERE thread_id=?1 AND idempotency_key=?2",
            params![ctx.thread_id,idempotency_key], |r| Ok((r.get::<_, String>(0)?,r.get::<_, String>(1)?)),
        ).optional()? {
            if ctx.actor != ChatActor::Cos {
                return Err(ChatError::Invalid("actor must be cos".into()));
            }
            if hash != request_hash { return Err(ChatError::Conflict("idempotency key has a different request hash".into())); }
            return get_conn(&tx, &id)?.ok_or_else(|| ChatError::not_found("operation", &id));
        }
        let validation = if ctx.actor != ChatActor::Cos {
            Some("actor must be cos")
        } else if ctx.thread_id.trim().is_empty() || ctx.run_id.trim().is_empty() {
            Some("thread_id and run_id are required")
        } else if ctx.operation_id.parse::<Ulid>().is_err() {
            Some("operation_id must be a ULID")
        } else if ctx.reason.trim().is_empty() {
            Some("reason is required")
        } else if ctx.policy_version.trim().is_empty() {
            Some("policy_version is required")
        } else if idempotency_key.trim().is_empty() || request_hash.trim().is_empty() {
            Some("idempotency_key and request_hash are required")
        } else if target_kind.trim().is_empty()
            || target_id.trim().is_empty()
            || action.trim().is_empty()
        {
            Some("target and action are required")
        } else {
            None
        };
        drop(tx);
        if let Some(message) = validation {
            self.cos_operation_reject_locked(
                &mut conn,
                ctx,
                idempotency_key,
                request_hash,
                target_kind,
                target_id,
                expected_revision,
                action,
                payload,
                message,
                &now,
            )?;
            return Err(ChatError::Invalid(message.into()));
        }
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let applied = (|| {
            let result = apply(&tx, ctx)?;
            insert_record(
                &tx,
                ctx,
                idempotency_key,
                request_hash,
                target_kind,
                target_id,
                expected_revision,
                action,
                payload,
                "applied",
                &result,
                &now,
            )?;
            audit_event(&tx, ctx, target_kind, target_id, "applied")?;
            let card = ChatCard {
                kind: ChatCardKind::Operation,
                id: ctx.operation_id.clone(),
                title: action.into(),
                state: "applied".into(),
                href: format!("/cos/operations/{}", ctx.operation_id),
                actor: ChatActor::Cos,
                reason: Some(ctx.reason.clone()),
                operation_id: Some(ctx.operation_id.clone()),
            };
            let event_id = append_event(
                &tx,
                &ctx.thread_id,
                Some(&ctx.run_id),
                None,
                ChatEventType::Card,
                &ChatEventData::Card(ChatCardData { card }),
                &now,
            )?;
            tx.execute(
                "UPDATE cos_operations SET event_id=?2 WHERE id=?1",
                params![ctx.operation_id, event_id],
            )?;
            get_conn(&tx, &ctx.operation_id)?
                .ok_or_else(|| ChatError::not_found("operation", &ctx.operation_id))
        })();
        match applied {
            Ok(row) => {
                tx.commit()?;
                Ok(row)
            }
            Err(error) => {
                drop(tx);
                self.cos_operation_reject_locked(
                    &mut conn,
                    ctx,
                    idempotency_key,
                    request_hash,
                    target_kind,
                    target_id,
                    expected_revision,
                    action,
                    payload,
                    &error.to_string(),
                    &now,
                )?;
                Err(error)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn cos_operation_reject_locked(
        &self,
        conn: &mut rusqlite::Connection,
        ctx: &AuditContext,
        key: &str,
        hash: &str,
        target_kind: &str,
        target_id: &str,
        revision: Option<&str>,
        action: &str,
        payload: &Value,
        error: &str,
        now: &str,
    ) -> Result<(), ChatError> {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = json!({"error":error});
        let mut rejected_ctx = ctx.clone();
        if rejected_ctx.reason.trim().is_empty() {
            rejected_ctx.reason = format!("rejected: {error}");
        }
        if rejected_ctx.operation_id.parse::<Ulid>().is_err() {
            rejected_ctx.operation_id = Ulid::new().to_string();
        }
        insert_record(
            &tx,
            &rejected_ctx,
            key,
            hash,
            target_kind,
            target_id,
            revision,
            action,
            payload,
            "rejected",
            &result,
            now,
        )?;
        audit_event(&tx, &rejected_ctx, target_kind, target_id, "rejected")?;
        tx.commit()?;
        Ok(())
    }
}
