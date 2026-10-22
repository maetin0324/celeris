//! Durable CoS triage intake and route arbitration (ADR 2026-10-06).
use std::collections::HashSet;

use rusqlite::{OptionalExtension, Transaction, params};
use time::OffsetDateTime;
use ulid::Ulid;

use super::store::{
    ChatError, chat_ts, cos_thread_ensure_tx, immediate, task_root_project, writer,
};
use crate::store::SqliteStore;

pub const COS_TRIAGE_BATCH_MAX: usize = 20;
const ROUTE_VERSION_CURSOR: &str = "cos_route_version";

/// The producer derives revision from the underlying question, never from title or read state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CosTriageSource {
    pub source_kind: String,
    pub source_key: String,
    pub source_revision: String,
    pub source_event_id: Option<i64>,
    pub operation_id: Option<String>,
    pub summary: String,
    pub policy_version: String,
    /// ADR 2026-10-22-cos-fixed-threads D3: the task the item is about. Its root's project picks
    /// the CoS thread; `None` (or no project) goes to the general CoS.
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CosTriageClaim {
    pub thread_id: String,
    pub message_id: String,
    pub run_id: String,
    pub item_ids: Vec<String>,
}

fn cursor_name(source: &str) -> Result<String, ChatError> {
    if source.is_empty() || source.len() > 128 || source.contains(':') {
        return Err(ChatError::Invalid("invalid triage source cursor".into()));
    }
    Ok(format!("cos_triage:{source}"))
}

/// D3: the CoS thread of one item (its task's root project, else general).
fn item_thread(
    tx: &Transaction<'_>,
    item: &CosTriageSource,
    at: &str,
) -> Result<String, ChatError> {
    let project = task_root_project(tx, item.task_id.as_deref())?;
    cos_thread_ensure_tx(tx, project.as_deref(), at)
}

/// D3: the outbox packet names the CoS the item belongs to (`"cos": "[統括 CoS]"` or
/// `"[CoS: <project>]"`), so the person sees which CoS is asking. Non-object bodies are kept.
fn with_cos_label(tx: &Transaction<'_>, thread_id: &str, body: &str) -> Result<String, ChatError> {
    let Ok(serde_json::Value::Object(mut packet)) = serde_json::from_str(body) else {
        return Ok(body.to_owned());
    };
    let label = super::store::thread_get_conn(tx, thread_id)?
        .as_ref()
        .and_then(super::store::cos_thread_label);
    if let Some(label) = label {
        packet.insert("cos".into(), serde_json::Value::String(label));
    }
    Ok(serde_json::Value::Object(packet).to_string())
}

fn insert_items(
    tx: &Transaction<'_>,
    items: &[CosTriageSource],
    at: &str,
) -> Result<usize, ChatError> {
    let mut inserted = 0;
    for item in items {
        if item.operation_id.is_some() {
            continue;
        }
        if item.source_kind.is_empty()
            || item.source_key.is_empty()
            || item.source_revision.is_empty()
        {
            return Err(ChatError::Invalid("empty triage source triple".into()));
        }
        let thread_id = item_thread(tx, item, at)?;
        inserted += tx.execute(
            "INSERT INTO cos_inbox_items(id,source_kind,source_key,source_revision,source_event_id,thread_id,message_id,state,policy_version,summary,created_at,updated_at) \
             VALUES(?1,?2,?3,?4,?5,?6,'','pending',?7,?9,?8,?8) \
             ON CONFLICT(source_kind,source_key,source_revision) DO NOTHING",
            params![Ulid::new().to_string(),item.source_kind,item.source_key,item.source_revision,item.source_event_id,thread_id,item.policy_version,at,item.summary],
        )?;
    }
    Ok(inserted)
}

fn put_cursor(tx: &Transaction<'_>, name: &str, value: &str) -> Result<(), ChatError> {
    tx.execute(
        "INSERT INTO feed_cursor(name,value) VALUES(?1,?2) ON CONFLICT(name) DO UPDATE SET value=excluded.value",
        params![name,value],
    )?;
    Ok(())
}

impl SqliteStore {
    /// Read the saved cursor for one source. An absent cursor means initial ingestion.
    pub fn cos_triage_cursor(&self, source: &str) -> Result<Option<String>, ChatError> {
        let name = cursor_name(source)?;
        let conn = writer(self)?;
        Ok(conn
            .query_row("SELECT value FROM feed_cursor WHERE name=?1", [name], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn cos_triage_route_version(&self) -> Result<Option<String>, ChatError> {
        let conn = writer(self)?;
        Ok(conn
            .query_row(
                "SELECT value FROM feed_cursor WHERE name=?1",
                [ROUTE_VERSION_CURSOR],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Store newly observed source revisions and advance their cursor atomically.
    pub fn cos_triage_ingest_batch(
        &self,
        source: &str,
        cursor: &str,
        items: &[CosTriageSource],
        now: OffsetDateTime,
    ) -> Result<usize, ChatError> {
        let name = cursor_name(source)?;
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let inserted = insert_items(&tx, items, &chat_ts(now))?;
        put_cursor(&tx, &name, cursor)?;
        tx.commit()?;
        Ok(inserted)
    }

    /// Startup/reconcile compares a current unresolved snapshot with stored rows, never events.
    /// The caller passes the complete current snapshot for one source kind.
    pub fn cos_triage_reconcile(
        &self,
        source_kind: &str,
        current: &[CosTriageSource],
        now: OffsetDateTime,
    ) -> Result<usize, ChatError> {
        if current.iter().any(|i| i.source_kind != source_kind) {
            return Err(ChatError::Invalid("mixed reconciliation sources".into()));
        }
        let at = chat_ts(now);
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let inserted = insert_items(&tx, current, &at)?;
        resolve_stale(&tx, source_kind, current, None, &at)?;
        tx.commit()?;
        Ok(inserted)
    }

    /// 元の待ちが閉じた item だけを resolved にする（挿入はしない）。`cos_triage_reconcile` から外れている
    /// 直送種別（credential_wait）用。ADR 2026-10-16-cos-inbox-escalated-resolve-on-source-close。
    pub fn cos_triage_resolve_stale(
        &self,
        source_kind: &str,
        current: &[CosTriageSource],
        now: OffsetDateTime,
    ) -> Result<usize, ChatError> {
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let n = resolve_stale(&tx, source_kind, current, None, &chat_ts(now))?;
        tx.commit()?;
        Ok(n)
    }

    /// event 取り込み用: `task_ids` を `source_key` に含む item だけを対象に、`current_by_kind` に無い
    /// 未解決 item を resolved にする（`resolve_stale` を共有。触れていない task の item は変えない）。
    pub fn cos_triage_resolve_stale_touched(
        &self,
        current_by_kind: &[(&str, Vec<CosTriageSource>)],
        task_ids: &[String],
        now: OffsetDateTime,
    ) -> Result<usize, ChatError> {
        if task_ids.is_empty() {
            return Ok(0);
        }
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let at = chat_ts(now);
        let mut n = 0;
        for (kind, current) in current_by_kind {
            n += resolve_stale(&tx, kind, current, Some(task_ids), &at)?;
        }
        tx.commit()?;
        Ok(n)
    }

    /// D3: CoS threads that have pending items, oldest item first.
    pub fn cos_triage_pending_threads(&self) -> Result<Vec<String>, ChatError> {
        let conn = writer(self)?;
        let mut stmt = conn.prepare(
            "SELECT thread_id FROM cos_inbox_items WHERE state='pending' \
             GROUP BY thread_id ORDER BY MIN(created_at),thread_id",
        )?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Claim at most twenty pending items of one CoS thread and bind one system message to one
    /// chat run of that thread.
    pub fn cos_triage_claim(
        &self,
        thread_id: &str,
        run_id: &str,
        now: OffsetDateTime,
    ) -> Result<Option<CosTriageClaim>, ChatError> {
        let at = chat_ts(now);
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let thread_id = thread_id.to_owned();
        let active: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_runs WHERE thread_id=?1 AND state IN ('running','stopping'))",
            [&thread_id],
            |r| r.get(0),
        )?;
        if active {
            return Ok(None);
        }
        let ids: Vec<String> = {
            let mut stmt = tx.prepare("SELECT id FROM cos_inbox_items WHERE state='pending' AND thread_id=?1 ORDER BY created_at,id LIMIT 20")?;
            stmt.query_map([&thread_id], |r| r.get(0))?
                .collect::<Result<_, _>>()?
        };
        if ids.is_empty() {
            return Ok(None);
        }
        let seq: i64 = tx.query_row(
            "SELECT next_seq FROM chat_threads WHERE id=?1",
            [&thread_id],
            |r| r.get(0),
        )?;
        let message_id = Ulid::new().to_string();
        let body = format!(
            "CoS 受信箱の一次対応: item_ids={}",
            serde_json::to_string(&ids)?
        );
        tx.execute("INSERT INTO chat_messages(id,thread_id,seq,role,text,state,run_id,metadata_json,created_at,updated_at) VALUES(?1,?2,?3,'system',?4,'running',?5,'{}',?6,?6)", params![message_id,thread_id,seq,body,run_id,at])?;
        tx.execute(
            "UPDATE chat_threads SET next_seq=next_seq+1,updated_at=?2 WHERE id=?1",
            params![thread_id, at],
        )?;
        tx.execute("INSERT INTO chat_runs(run_id,thread_id,input_message_id,state,started_at) VALUES(?1,?2,?3,'running',?4)", params![run_id,thread_id,message_id,at])?;
        for id in &ids {
            tx.execute("UPDATE cos_inbox_items SET state='running',run_id=?2,message_id=?3,updated_at=?4 WHERE id=?1 AND state='pending'", params![id,run_id,message_id,at])?;
        }
        tx.commit()?;
        Ok(Some(CosTriageClaim {
            thread_id,
            message_id,
            run_id: run_id.into(),
            item_ids: ids,
        }))
    }

    /// Return unhandled items to the queue only after the caller has established run loss.
    pub fn cos_triage_requeue_run(
        &self,
        run_id: &str,
        now: OffsetDateTime,
    ) -> Result<usize, ChatError> {
        let conn = writer(self)?;
        Ok(conn.execute(
            "UPDATE cos_inbox_items SET state='pending',run_id=NULL,message_id='',updated_at=?2 WHERE run_id=?1 AND state='running'",
            params![run_id,chat_ts(now)],
        )?)
    }

    /// Record a terminal outcome for one item. The caller validates the source decision itself.
    pub fn cos_triage_resolve(
        &self,
        item_id: &str,
        outcome: &str,
        operation_id: Option<&str>,
        reason: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<bool, ChatError> {
        if !matches!(
            outcome,
            "answered" | "observed" | "escalated" | "fallback" | "resolved"
        ) {
            return Err(ChatError::Invalid("invalid triage outcome".into()));
        }
        let conn = writer(self)?;
        Ok(conn.execute("UPDATE cos_inbox_items SET state=?2,operation_id=?3,reason=?4,updated_at=?5 WHERE id=?1 AND state IN ('pending','running')", params![item_id,outcome,operation_id,reason,chat_ts(now)])? == 1)
    }

    /// Arbitration point shared by escalation and unavailable fallback.
    pub fn cos_triage_outbox_claim(
        &self,
        item_id: &str,
        route: &str,
        body: &str,
        now: OffsetDateTime,
    ) -> Result<Option<String>, ChatError> {
        let kind = match route {
            "escalation" => "cos_escalation",
            "fallback" => "cos_fallback",
            _ => return Err(ChatError::Invalid("invalid CoS notification route".into())),
        };
        let at = chat_ts(now);
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let source: Option<(String,String,String,String,String)> = tx.query_row(
            "SELECT source_kind,source_key,source_revision,state,thread_id FROM cos_inbox_items WHERE id=?1", [item_id],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))
        ).optional()?;
        let Some((source_kind, source_key, revision, state, thread_id)) = source else {
            return Err(ChatError::not_found("triage item", item_id));
        };
        if matches!(state.as_str(), "answered" | "observed" | "resolved") {
            return Ok(None);
        }
        let notification_id = Ulid::new().to_string();
        let claimed = tx.execute("INSERT INTO cos_notification_routes(item_id,notification_id,route,source_kind,source_key,source_revision,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?7) ON CONFLICT DO NOTHING", params![item_id,notification_id,route,source_kind,source_key,revision,at])?;
        if claimed == 0 {
            return Ok(None);
        }
        let body = with_cos_label(&tx, &thread_id, body)?;
        tx.execute(
            "INSERT INTO notifications(id,kind,key,body,created_at) VALUES(?1,?2,?3,?4,?5)",
            params![notification_id, kind, item_id, body, at],
        )?;
        tx.commit()?;
        Ok(Some(notification_id))
    }

    /// 決定的な直経路（ADR 2026-10-15-credential-wait-skip-triage）: CoS の判断（LLM run）を要さない待ちを、
    /// triage の `pending` を経ずに最初から `escalated` の item として積み、escalation outbox を同じ transaction で作る。
    /// 同じ (kind, key, revision) は 1 回だけ（既にあれば何もしない）。`cos_triage_claim` は `pending` だけを取るので
    /// この item では CoS run が起きない。
    pub fn cos_triage_direct_escalate(
        &self,
        source: &CosTriageSource,
        body: &str,
        now: OffsetDateTime,
    ) -> Result<Option<String>, ChatError> {
        if source.source_kind.is_empty()
            || source.source_key.is_empty()
            || source.source_revision.is_empty()
        {
            return Err(ChatError::Invalid("empty triage source triple".into()));
        }
        let at = chat_ts(now);
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let thread_id = item_thread(&tx, source, &at)?;
        let item_id = Ulid::new().to_string();
        let inserted = tx.execute(
            "INSERT INTO cos_inbox_items(id,source_kind,source_key,source_revision,source_event_id,thread_id,message_id,state,policy_version,summary,created_at,updated_at) \
             VALUES(?1,?2,?3,?4,NULL,?5,'','escalated',?6,?7,?8,?8) \
             ON CONFLICT(source_kind,source_key,source_revision) DO NOTHING",
            params![item_id, source.source_kind, source.source_key, source.source_revision, thread_id, source.policy_version, source.summary, at],
        )?;
        if inserted == 0 {
            return Ok(None);
        }
        let notification_id = Ulid::new().to_string();
        let body = with_cos_label(&tx, &thread_id, body)?;
        tx.execute(
            "INSERT INTO cos_notification_routes(item_id,notification_id,route,source_kind,source_key,source_revision,created_at,updated_at) VALUES(?1,?2,'escalation',?3,?4,?5,?6,?6)",
            params![item_id, notification_id, source.source_kind, source.source_key, source.source_revision, at],
        )?;
        tx.execute(
            "INSERT INTO notifications(id,kind,key,body,created_at) VALUES(?1,'cos_escalation',?2,?3,?4)",
            params![notification_id, item_id, body, at],
        )?;
        tx.commit()?;
        Ok(Some(notification_id))
    }

    /// The notifier calls this after rechecking the source immediately before POST.
    /// Returns true only while the item and notification still represent an unresolved wait.
    pub fn cos_triage_outbox_sendable(
        &self,
        notification_id: &str,
        source_unresolved: bool,
        now: OffsetDateTime,
    ) -> Result<bool, ChatError> {
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let state: Option<(String,Option<i64>)> = tx.query_row(
            "SELECT i.state,n.ok FROM cos_notification_routes r JOIN cos_inbox_items i ON i.id=r.item_id JOIN notifications n ON n.id=r.notification_id WHERE r.notification_id=?1", [notification_id],
            |r| Ok((r.get(0)?,r.get(1)?))
        ).optional()?;
        let sendable = matches!(state, Some((ref s,None)) if source_unresolved && !matches!(s.as_str(), "answered"|"observed"|"resolved"));
        if !sendable && state.is_some() {
            tx.execute(
                "UPDATE notifications SET ok=0,error='superseded' WHERE id=?1 AND ok IS NULL",
                [notification_id],
            )?;
            if !source_unresolved {
                tx.execute("UPDATE cos_inbox_items SET state='resolved',updated_at=?2 WHERE id=(SELECT item_id FROM cos_notification_routes WHERE notification_id=?1) AND state IN ('pending','running')", params![notification_id,chat_ts(now)])?;
            }
        }
        tx.commit()?;
        Ok(sendable)
    }

    /// One-time route switch, initial unresolved snapshot, cursor and legacy pending links.
    pub fn cos_triage_cutover(
        &self,
        source: &str,
        cursor: &str,
        initial: &[CosTriageSource],
        legacy: &[(String, String, String, String)],
        now: OffsetDateTime,
    ) -> Result<bool, ChatError> {
        let name = cursor_name(source)?;
        let at = chat_ts(now);
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT value FROM feed_cursor WHERE name=?1",
                [ROUTE_VERSION_CURSOR],
                |r| r.get(0),
            )
            .optional()?;
        if existing.as_deref() == Some("cos_v1") {
            return Ok(false);
        }
        insert_items(&tx, initial, &at)?;
        for (notification_id, kind, key, revision) in legacy {
            let item_id: String = tx.query_row("SELECT id FROM cos_inbox_items WHERE source_kind=?1 AND source_key=?2 AND source_revision=?3", params![kind,key,revision], |r| r.get(0))?;
            tx.execute("INSERT INTO cos_legacy_notification_links(notification_id,item_id,created_at) VALUES(?1,?2,?3) ON CONFLICT(notification_id) DO NOTHING", params![notification_id,item_id,at])?;
            tx.execute("UPDATE notifications SET ok=0,error='superseded' WHERE id=?1 AND ok IS NULL AND sent_at IS NULL", [notification_id])?;
        }
        put_cursor(&tx, &name, cursor)?;
        put_cursor(&tx, ROUTE_VERSION_CURSOR, "cos_v1")?;
        tx.commit()?;
        Ok(true)
    }
}

// ---------------------------------------------------------------------------
// ADR 2026-10-07-cos-inbox-thread-conversation: the read side the inbox thread
// needs — the items one triage run handled (digest, D1) and the unresolved
// items passed to a run in the inbox thread as context (D3). SQLite only.
// ---------------------------------------------------------------------------

/// Prefix of the idempotency key of one run's digest message (`<prefix><run_id>`).
pub const COS_TRIAGE_DIGEST_KEY_PREFIX: &str = "cos-triage-digest:";

/// One option of the original wait as the outbox body recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CosTriageOption {
    pub key: String,
    pub label: String,
}

/// What the person must decide, as the escalation / fallback outbox body recorded it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CosTriageDecision {
    pub summary: String,
    pub options: Vec<CosTriageOption>,
    pub recommended: Option<String>,
    pub recommendation_reason: Option<String>,
    pub web_path: String,
}

/// One `cos_inbox_items` row with its route and decision packet (if routed to the person).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CosTriageItemReport {
    pub item_id: String,
    /// The CoS thread the item belongs to (ADR 2026-10-22-cos-fixed-threads D3).
    pub thread_id: String,
    pub source_kind: String,
    pub source_key: String,
    pub source_revision: String,
    pub state: String,
    pub summary: String,
    pub reason: Option<String>,
    pub operation_id: Option<String>,
    pub run_id: Option<String>,
    pub created_at: String,
    /// `escalation` / `fallback` when an outbox row exists for the item.
    pub route: Option<String>,
    pub decision: Option<CosTriageDecision>,
}

impl CosTriageItemReport {
    /// The title shown to people; falls back to the source triple for rows ingested before 0059.
    pub fn title(&self) -> String {
        if self.summary.trim().is_empty() {
            format!("{}:{}", self.source_kind, self.source_key)
        } else {
            self.summary.clone()
        }
    }
}

/// Parse the outbox body of either route. Escalation options are `{key,label}`; the fallback body
/// lists labels only (its `recommended` is a label too).
pub fn decision_from_outbox(body: &str) -> Option<CosTriageDecision> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let options = v["options"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|o| match o {
                    serde_json::Value::String(label) => Some(CosTriageOption {
                        key: label.clone(),
                        label: label.clone(),
                    }),
                    serde_json::Value::Object(_) => Some(CosTriageOption {
                        key: o["key"].as_str()?.to_owned(),
                        label: o["label"].as_str().unwrap_or(o["key"].as_str()?).to_owned(),
                    }),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    Some(CosTriageDecision {
        summary: v["summary"].as_str().unwrap_or_default().to_owned(),
        options,
        recommended: v["recommended"].as_str().map(str::to_owned),
        recommendation_reason: v["recommendation_reason"]
            .as_str()
            .or_else(|| v["recommendation_text"].as_str())
            .map(str::to_owned),
        web_path: v["web_path"].as_str().unwrap_or_default().to_owned(),
    })
}

const REPORT_SELECT: &str = "SELECT i.id,i.source_kind,i.source_key,i.source_revision,i.state,i.summary,i.reason,i.operation_id,i.run_id,i.created_at,n.route,nt.body,i.thread_id \
    FROM cos_inbox_items i \
    LEFT JOIN cos_notification_routes n ON n.item_id=i.id \
    LEFT JOIN notifications nt ON nt.id=n.notification_id";

fn report_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<CosTriageItemReport> {
    let body: Option<String> = r.get(11)?;
    Ok(CosTriageItemReport {
        item_id: r.get(0)?,
        thread_id: r.get(12)?,
        source_kind: r.get(1)?,
        source_key: r.get(2)?,
        source_revision: r.get(3)?,
        state: r.get(4)?,
        summary: r.get(5)?,
        reason: r.get(6)?,
        operation_id: r.get(7)?,
        run_id: r.get(8)?,
        created_at: r.get(9)?,
        route: r.get(10)?,
        decision: body.as_deref().and_then(decision_from_outbox),
    })
}

impl SqliteStore {
    /// D1: the items one triage run claimed, in claim order (requeued items no longer count).
    pub fn cos_triage_run_report(
        &self,
        run_id: &str,
    ) -> Result<Vec<CosTriageItemReport>, ChatError> {
        let conn = writer(self)?;
        let mut stmt = conn.prepare(&format!(
            "{REPORT_SELECT} WHERE i.run_id=?1 ORDER BY i.created_at,i.id"
        ))?;
        let rows = stmt.query_map([run_id], report_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// One item with its route and decision packet.
    pub fn cos_triage_item_report(
        &self,
        item_id: &str,
    ) -> Result<Option<CosTriageItemReport>, ChatError> {
        let conn = writer(self)?;
        Ok(conn
            .query_row(
                &format!("{REPORT_SELECT} WHERE i.id=?1"),
                [item_id],
                report_row,
            )
            .optional()?)
    }

    /// D3: unresolved items (waiting on the person, handed over, or not yet judged), newest first.
    /// `thread_id: Some` limits them to one CoS thread (a project CoS); `None` is every thread
    /// (the general CoS overlooks all projects, ADR 2026-10-22-cos-fixed-threads D3).
    pub fn cos_triage_open_items(
        &self,
        thread_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<CosTriageItemReport>, ChatError> {
        let conn = writer(self)?;
        let mut stmt = conn.prepare(&format!(
            "{REPORT_SELECT} WHERE i.state IN ('escalated','fallback','pending','running') \
             AND (?2 IS NULL OR i.thread_id=?2) \
             ORDER BY i.created_at DESC,i.id DESC LIMIT ?1"
        ))?;
        let rows = stmt.query_map(
            params![i64::try_from(limit).unwrap_or(i64::MAX), thread_id],
            report_row,
        )?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// D1: finished triage runs (system input message, at least one item) of any CoS thread that
    /// have no digest message yet: `(thread_id, run_id, run state)` oldest first.
    pub fn cos_triage_runs_without_digest(
        &self,
        limit: usize,
    ) -> Result<Vec<(String, String, String)>, ChatError> {
        let conn = writer(self)?;
        let mut stmt = conn.prepare(
            "SELECT r.thread_id,r.run_id,r.state FROM chat_runs r \
             JOIN chat_messages m ON m.id=r.input_message_id \
             WHERE m.role='system' \
             AND r.state IN ('completed','failed','stopped','interrupted') \
             AND EXISTS(SELECT 1 FROM cos_inbox_items i WHERE i.run_id=r.run_id) \
             AND NOT EXISTS(SELECT 1 FROM chat_messages d WHERE d.thread_id=r.thread_id AND d.client_message_id=?1||r.run_id) \
             ORDER BY r.started_at,r.run_id LIMIT ?2",
        )?;
        let rows = stmt.query_map(
            params![
                COS_TRIAGE_DIGEST_KEY_PREFIX,
                i64::try_from(limit).unwrap_or(i64::MAX)
            ],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

/// 現在の未解決 snapshot に無い `pending`／`running`／`escalated` の item を resolved にする。
/// `escalated` は人に渡した後も元の待ちが閉じれば処理済みである（events には触れない）。
fn resolve_stale(
    tx: &Transaction<'_>,
    source_kind: &str,
    current: &[CosTriageSource],
    touched: Option<&[String]>,
    at: &str,
) -> Result<usize, ChatError> {
    let live: HashSet<(&str, &str)> = current
        .iter()
        .filter(|i| i.operation_id.is_none())
        .map(|i| (i.source_key.as_str(), i.source_revision.as_str()))
        .collect();
    let stale: Vec<String> = {
        let mut stmt = tx.prepare("SELECT id,source_key,source_revision FROM cos_inbox_items WHERE source_kind=?1 AND state IN ('pending','running','escalated')")?;
        let rows = stmt.query_map([source_kind], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|(_, k, r)| !live.contains(&(k.as_str(), r.as_str())))
            .filter(|(_, k, _)| touched.is_none_or(|ts| ts.iter().any(|t| k.contains(t.as_str()))))
            .map(|(id, _, _)| id)
            .collect()
    };
    for id in &stale {
        tx.execute(
            "UPDATE cos_inbox_items SET state='resolved',updated_at=?2 WHERE id=?1",
            params![id, at],
        )?;
    }
    Ok(stale.len())
}
