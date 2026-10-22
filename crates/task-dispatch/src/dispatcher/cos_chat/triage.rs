//! CoS inbox intake (ADR 2026-10-05-cos-chat-home D3「一次対応の起動とルーティング」,
//! store contract: ADR 2026-10-06-cos-inbox-triage).
//!
//! The dispatcher never decides whether a human is needed. It only turns new
//! waits from the ADR-0133 derived inbox (`task_ops::human_inbox`) and the
//! notice store (`notice_list`) into `cos_inbox_items`, then starts one CoS run
//! over at most twenty pending items through the ordinary chat-run path.
//!
//! - Source `events`: the cursor is the global event id. Only events after the
//!   cursor are read. When a non-noise event touches a task, the derived inbox
//!   is built once for that tick and only items of touched tasks are offered.
//!   Item identity is `(InboxKind, InboxItem.id, InboxItem.created_at)`, so a
//!   title edit or a later unrelated event never makes a new revision.
//! - Source `notices`: the cursor is the newest `(last_at, id)` seen. The
//!   revision is the bundle count, so read state and title never count.
//! - Startup/reconcile compares the current derived inbox with stored rows
//!   (never the event history). The first run without a cursor ingests the
//!   unresolved waits once and starts the cursors at the current head.
//! - Waits written by a CoS operation (the operation transaction ends with the
//!   `CosOperation` audit event) and CoS's own replies carry an operation id
//!   and are dropped by the store.

use std::collections::{BTreeMap, HashMap, HashSet};

use task_core::chat::triage::{COS_TRIAGE_BATCH_MAX, CosTriageSource};
use task_core::chat::{ChatCardKind, ChatRun};
use task_core::{Event, EventRow, Notice, NoticeKind, NoticeQuery, NoticeStore, TaskId, TaskStore};
use task_ops::human_inbox::{HumanInbox, InboxItem, InboxKind};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::launch::{ClaimSource, CosChatLaunch};
use crate::dispatcher::Dispatcher;

/// Cursor names under `feed_cursor` (`cos_triage:<source>`).
pub const COS_TRIAGE_EVENTS_SOURCE: &str = "events";
pub const COS_TRIAGE_NOTICES_SOURCE: &str = "notices";
/// `cos_inbox_items.source_kind` of a notice bundle.
pub const COS_TRIAGE_NOTICE_KIND: &str = "notice";

const EVENT_BATCH: usize = 512;
/// Bounded work per tick; the remainder continues from the saved cursor.
const EVENT_PAGES_PER_TICK: usize = 8;
const NOTICE_PAGE: usize = 100;

/// `[cos.triage]` as resolved by the daemon (ADR 2026-10-05 §config).
#[derive(Debug, Clone, PartialEq)]
pub struct CosTriageSettings {
    pub policy_skill: String,
    pub policy_version: String,
    pub min_confidence: f64,
    pub human_required: Vec<String>,
    /// D6: a wait with no CoS outcome after this many seconds goes to the
    /// unavailable fallback (fallback leaf).
    pub unavailable_after_secs: u64,
}

impl Default for CosTriageSettings {
    fn default() -> Self {
        Self {
            policy_skill: "cos-inbox-triage".into(),
            policy_version: "1".into(),
            min_confidence: 0.85,
            human_required: [
                "fundamental_change",
                "external_publish",
                "destructive",
                "resource_overrun",
                "security",
                "explicit_human",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            unavailable_after_secs: 120,
        }
    }
}

/// Process-local hints only. The truth (items, cursors, runs) is in SQLite.
#[derive(Debug, Default)]
pub(crate) struct CosTriageState {
    /// The startup comparison has run in this process.
    reconciled: bool,
    /// Pending items may exist; try one claim when the inbox thread is idle.
    pub(super) dirty: bool,
    /// A run of the inbox thread finished (or the process started): write the digests
    /// of finished triage runs (ADR 2026-10-07-cos-inbox-thread-conversation D1).
    pub(crate) digest_due: bool,
    /// Threads whose run this process started; when one leaves `running`, a digest may be due.
    pub(crate) live_runs: HashSet<String>,
    /// Root-done steps that already logged a failure in this process (log once, retry every tick).
    pub(crate) root_done_warned: HashSet<String>,
}

impl Dispatcher {
    /// Ask the next tick to compare the current derived inbox with stored rows
    /// again (a reconcile pass; the startup pass happens on its own).
    pub fn request_cos_triage_reconcile(&mut self) {
        if let Some(launch) = self.cos_chat_launch.as_mut() {
            launch.triage.reconciled = false;
        }
    }
}

/// Events that never open, revise or close a human wait. Skipping them keeps a
/// busy run from rebuilding the derived inbox on every tick.
fn is_noise(event: &Event) -> bool {
    matches!(
        event,
        Event::WorkerProgress { .. }
            | Event::ArtifactProduced { .. }
            | Event::ClusterJobWaitPolled { .. }
            | Event::QuotaEstimated { .. }
            | Event::CheckpointSaved { .. }
            | Event::WorkUnitCheckStarted { .. }
            | Event::IntegrationCheckStarted { .. }
            | Event::BrowserUpdated { .. }
            | Event::ProviderThrottled { .. }
            | Event::RoutingDecided { .. }
    )
}

/// Tasks touched by a batch, with the CoS operation that wrote every
/// non-noise event of that task in this batch (if any).
///
/// An operation writes its domain events and then the `CosOperation` audit
/// event in one transaction, so they hold consecutive global ids of one task.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct TouchedTasks {
    pub tasks: BTreeMap<TaskId, Option<String>>,
}

pub(crate) fn touched_tasks(rows: &[EventRow]) -> TouchedTasks {
    // Attributed event ids: walk back from each audit event over the
    // contiguous run of the same task.
    let mut attributed: HashMap<u64, String> = HashMap::new();
    for (i, row) in rows.iter().enumerate() {
        let Event::CosOperation { operation_id, .. } = &row.event else {
            continue;
        };
        attributed.insert(row.id, operation_id.clone());
        let mut j = i;
        while j > 0 {
            let prev = &rows[j - 1];
            if prev.task_id != row.task_id
                || prev.id + 1 != rows[j].id
                || matches!(prev.event, Event::CosOperation { .. })
            {
                break;
            }
            attributed.insert(prev.id, operation_id.clone());
            j -= 1;
        }
    }
    let mut out = TouchedTasks::default();
    for row in rows.iter().filter(|r| !is_noise(&r.event)) {
        let op = attributed.get(&row.id).cloned();
        match out.tasks.get_mut(&row.task_id) {
            // One unattributed event makes the task a genuine new wait.
            Some(existing) => {
                if op.is_none() {
                    *existing = None;
                }
            }
            None => {
                out.tasks.insert(row.task_id, op);
            }
        }
    }
    out
}

fn inbox_source(item: &InboxItem, policy_version: &str) -> CosTriageSource {
    CosTriageSource {
        source_kind: item.kind.as_str().to_owned(),
        source_key: item.id.clone(),
        source_revision: item.created_at.clone(),
        source_event_id: None,
        operation_id: None,
        summary: item.title.clone(),
        policy_version: policy_version.to_owned(),
        task_id: item_tasks(item).next().map(|t| t.to_string()),
    }
}

/// 決定的に outbox へ直送する種別（CoS の判断・LLM run を要さない。ADR 2026-10-15-credential-wait-skip-triage）。
fn is_direct(item: &InboxItem) -> bool {
    item.kind == InboxKind::CredentialWait
}

/// 直送する item の escalation outbox の本文。値は載せない（用途・待ち task・登録リンクだけ）。
fn direct_body(item: &InboxItem) -> String {
    let mut summary = item.title.clone();
    if let Some(detail) = item.detail.as_deref().filter(|d| !d.is_empty()) {
        summary.push_str(" — ");
        summary.push_str(detail);
    }
    serde_json::json!({
        "route": "escalation",
        "headline": "API キーの登録のお願い（自動通知）",
        "summary": summary,
        "blocking": item.blocking.summary,
        "web_path": "/accounts",
        "options": [],
    })
    .to_string()
}

fn item_tasks(item: &InboxItem) -> impl Iterator<Item = TaskId> + '_ {
    item.task
        .iter()
        .chain(item.blocking.root.iter())
        .chain(item.blocking.tasks.iter())
        .map(|t| t.id)
}

/// Offer the items whose task was touched; a task attributed to a CoS
/// operation passes its operation id so the store drops the item.
pub(crate) fn sources_for_touched(
    inbox: &HumanInbox,
    touched: &TouchedTasks,
    policy_version: &str,
) -> Vec<CosTriageSource> {
    let mut out = Vec::new();
    for item in inbox.items.iter().filter(|i| !is_direct(i)) {
        let hits: Vec<&Option<String>> = item_tasks(item)
            .filter_map(|id| touched.tasks.get(&id))
            .collect();
        if hits.is_empty() {
            continue;
        }
        let mut source = inbox_source(item, policy_version);
        // Only when every touching task was written by CoS itself.
        if hits.iter().all(|op| op.is_some()) {
            source.operation_id = hits.iter().find_map(|op| (*op).clone());
        }
        out.push(source);
    }
    out
}

fn notice_cursor(notice: &Notice) -> Result<String, String> {
    let at = notice.last_at.format(&Rfc3339).map_err(|e| e.to_string())?;
    Ok(format!("{at}|{}", notice.id))
}

/// `(last_at, id)` of a saved notice cursor.
fn parse_notice_cursor(raw: &str) -> Result<(OffsetDateTime, String), String> {
    let (at, id) = raw
        .split_once('|')
        .ok_or_else(|| format!("invalid notice cursor {raw}"))?;
    let at = OffsetDateTime::parse(at, &Rfc3339).map_err(|e| e.to_string())?;
    Ok((at, id.to_owned()))
}

fn notice_newer(notice: &Notice, cursor: &(OffsetDateTime, String)) -> bool {
    (notice.last_at, notice.id.to_string()) > (cursor.0, cursor.1.clone())
}

pub(crate) fn notice_source(notice: &Notice, policy_version: &str) -> Option<CosTriageSource> {
    // CoS's own replies are its output, not new input.
    if notice.kind == NoticeKind::SecretaryReply {
        return None;
    }
    let operation_id = notice
        .target
        .as_ref()
        .filter(|t| t.kind == "cos_operation")
        .map(|t| t.id.clone());
    Some(CosTriageSource {
        source_kind: COS_TRIAGE_NOTICE_KIND.to_owned(),
        source_key: notice.id.to_string(),
        source_revision: notice.count.to_string(),
        source_event_id: None,
        operation_id,
        summary: notice.title.clone(),
        policy_version: policy_version.to_owned(),
        task_id: notice.task_id.clone(),
    })
}

pub(super) fn card_kind(source_kind: &str) -> ChatCardKind {
    match source_kind {
        "decision" => ChatCardKind::Decision,
        "question" => ChatCardKind::Question,
        "authorization" | "acceptance_check" => ChatCardKind::Approval,
        "plan_gate" | "phase_gate" => ChatCardKind::PlanGate,
        COS_TRIAGE_NOTICE_KIND => ChatCardKind::Notice,
        _ => ChatCardKind::Task,
    }
}

impl CosChatLaunch {
    /// One intake pass: startup/reconcile, events after the cursor, notices
    /// after the cursor. Errors are logged; the cursor only advances with the
    /// items it covers, so the next tick retries.
    pub(crate) fn triage_ingest(&mut self, dispatcher: &Dispatcher) {
        let now = dispatcher.now_utc();
        if !self.triage.reconciled {
            match self.triage_reconcile(dispatcher, now) {
                Ok(()) => {
                    self.triage.reconciled = true;
                    self.triage.dirty = true;
                    self.triage.digest_due = true;
                }
                Err(error) => {
                    tracing::warn!(%error, "CoS triage reconcile failed");
                    return;
                }
            }
        }
        if let Err(error) = self.triage_events(dispatcher, now) {
            tracing::warn!(%error, "CoS triage event intake failed");
        }
        if let Err(error) = self.triage_notices(now) {
            tracing::warn!(%error, "CoS triage notice intake failed");
        }
        // D1 of the inbox-thread ADR: the judgment of each finished triage run as an
        // assistant message (and the requeue of an interrupted run's items), before the
        // fallback looks at what is left.
        if self.triage.digest_due {
            self.triage_digest(now);
        }
        // D6: LLM-free direct notice for items CoS cannot answer.
        self.triage_fallback(dispatcher, now);
    }

    /// Items of the current derived inbox (shared with the fallback leaf).
    pub(super) fn human_inbox_items(
        &self,
        dispatcher: &Dispatcher,
        now: OffsetDateTime,
    ) -> Result<Vec<InboxItem>, String> {
        self.human_inbox(dispatcher, now).map(|inbox| inbox.items)
    }

    fn human_inbox(
        &self,
        dispatcher: &Dispatcher,
        now: OffsetDateTime,
    ) -> Result<HumanInbox, String> {
        let ctx = task_ops::view::ViewContext {
            workspace_roots: dispatcher.config.workspace_roots.clone(),
            retry_backoff_base: dispatcher.config.retry_backoff_base,
            retry_backoff_max: dispatcher.config.retry_backoff_max,
            max_requeues: dispatcher.config.max_requeues,
            clusters: HashMap::new(),
        };
        let store: &dyn TaskStore = self.store.as_ref();
        task_ops::human_inbox::human_inbox(store, None, &ctx, now, &|_, _| Vec::new())
            .map_err(|e| e.to_string())
    }

    /// Compare the current derived inbox with stored rows per kind (never the
    /// event history). Without a cursor this is the one-time introduction
    /// pass; the cursors then start at the current heads.
    fn triage_reconcile(
        &mut self,
        dispatcher: &Dispatcher,
        now: OffsetDateTime,
    ) -> Result<(), String> {
        let policy = self.config.triage.policy_version.clone();
        // Read the head first: events after it are re-read (idempotently).
        let head = self.store.latest_event_id().map_err(|e| e.to_string())?;
        let inbox = self.human_inbox(dispatcher, now)?;
        let mut by_kind: BTreeMap<&'static str, Vec<CosTriageSource>> = InboxKind::ALL
            .iter()
            .map(|k| (k.as_str(), Vec::new()))
            .collect();
        for item in &inbox.items {
            by_kind
                .entry(item.kind.as_str())
                .or_default()
                .push(inbox_source(item, &policy));
        }
        for item in inbox.items.iter().filter(|i| is_direct(i)) {
            self.direct_escalate(item, &policy, now)?;
        }
        if let Some(current) = by_kind.remove(InboxKind::CredentialWait.as_str()) {
            // 挿入は direct_escalate が行う。閉じた待ちの escalated item だけここで resolved にする。
            self.store
                .cos_triage_resolve_stale(InboxKind::CredentialWait.as_str(), &current, now)
                .map_err(|e| e.to_string())?;
        }
        let mut inserted = 0;
        for (kind, current) in &by_kind {
            inserted += self
                .store
                .cos_triage_reconcile(kind, current, now)
                .map_err(|e| e.to_string())?;
        }
        if self
            .store
            .cos_triage_cursor(COS_TRIAGE_EVENTS_SOURCE)
            .map_err(|e| e.to_string())?
            .is_none()
        {
            self.store
                .cos_triage_ingest_batch(COS_TRIAGE_EVENTS_SOURCE, &head.to_string(), &[], now)
                .map_err(|e| e.to_string())?;
        }
        if self
            .store
            .cos_triage_cursor(COS_TRIAGE_NOTICES_SOURCE)
            .map_err(|e| e.to_string())?
            .is_none()
        {
            // Notices need no decision: the ones already delivered before
            // introduction are not replayed to CoS.
            let newest = self
                .store
                .notice_list(&NoticeQuery {
                    limit: 1,
                    ..NoticeQuery::default()
                })
                .map_err(|e| e.to_string())?;
            let cursor = match newest.items.first() {
                Some(notice) => notice_cursor(notice)?,
                None => format!(
                    "{}|",
                    OffsetDateTime::UNIX_EPOCH
                        .format(&Rfc3339)
                        .map_err(|e| e.to_string())?
                ),
            };
            self.store
                .cos_triage_ingest_batch(COS_TRIAGE_NOTICES_SOURCE, &cursor, &[], now)
                .map_err(|e| e.to_string())?;
        }
        if inserted > 0 {
            tracing::info!(inserted, "CoS triage reconcile added waits");
        }
        Ok(())
    }

    /// 直送: triage の待ちを作らず escalation outbox へ（既にあれば何もしない）。
    fn direct_escalate(
        &mut self,
        item: &InboxItem,
        policy: &str,
        now: OffsetDateTime,
    ) -> Result<(), String> {
        let source = inbox_source(item, policy);
        let queued = self
            .store
            .cos_triage_direct_escalate(&source, &direct_body(item), now)
            .map_err(|e| e.to_string())?;
        if queued.is_some() {
            tracing::info!(item = %item.id, "credential wait queued for direct notification");
        }
        Ok(())
    }

    fn triage_events(
        &mut self,
        dispatcher: &Dispatcher,
        now: OffsetDateTime,
    ) -> Result<(), String> {
        let policy = self.config.triage.policy_version.clone();
        let Some(raw) = self
            .store
            .cos_triage_cursor(COS_TRIAGE_EVENTS_SOURCE)
            .map_err(|e| e.to_string())?
        else {
            return Ok(());
        };
        let mut cursor: u64 = raw
            .parse()
            .map_err(|_| format!("invalid event cursor {raw}"))?;
        let mut inbox: Option<HumanInbox> = None;
        for _ in 0..EVENT_PAGES_PER_TICK {
            let rows = self
                .store
                .events_since(cursor, EVENT_BATCH)
                .map_err(|e| e.to_string())?;
            let Some(last) = rows.last().map(|r| r.id) else {
                break;
            };
            let touched = touched_tasks(&rows);
            if !touched.tasks.is_empty() {
                if inbox.is_none() {
                    inbox = Some(self.human_inbox(dispatcher, now)?);
                }
                if let Some(inbox) = &inbox {
                    for item in inbox.items.iter().filter(|i| is_direct(i)) {
                        if item_tasks(item).any(|id| touched.tasks.contains_key(&id)) {
                            self.direct_escalate(item, &policy, now)?;
                        }
                    }
                }
            }
            if !touched.tasks.is_empty()
                && let Some(inbox) = &inbox
            {
                let mut by_kind: BTreeMap<&'static str, Vec<CosTriageSource>> = InboxKind::ALL
                    .iter()
                    .map(|k| (k.as_str(), Vec::new()))
                    .collect();
                for item in &inbox.items {
                    by_kind
                        .entry(item.kind.as_str())
                        .or_default()
                        .push(inbox_source(item, &policy));
                }
                let current: Vec<_> = by_kind.into_iter().collect();
                let ids: Vec<String> = touched.tasks.keys().map(|id| id.to_string()).collect();
                self.store
                    .cos_triage_resolve_stale_touched(&current, &ids, now)
                    .map_err(|e| e.to_string())?;
            }
            let sources = if touched.tasks.is_empty() {
                Vec::new()
            } else {
                if inbox.is_none() {
                    inbox = Some(self.human_inbox(dispatcher, now)?);
                }
                inbox
                    .as_ref()
                    .map(|inbox| sources_for_touched(inbox, &touched, &policy))
                    .unwrap_or_default()
            };
            let inserted = self
                .store
                .cos_triage_ingest_batch(COS_TRIAGE_EVENTS_SOURCE, &last.to_string(), &sources, now)
                .map_err(|e| e.to_string())?;
            cursor = last;
            if inserted > 0 {
                self.triage.dirty = true;
            }
            if rows.len() < EVENT_BATCH {
                break;
            }
        }
        Ok(())
    }

    fn triage_notices(&mut self, now: OffsetDateTime) -> Result<(), String> {
        let policy = self.config.triage.policy_version.clone();
        let Some(raw) = self
            .store
            .cos_triage_cursor(COS_TRIAGE_NOTICES_SOURCE)
            .map_err(|e| e.to_string())?
        else {
            return Ok(());
        };
        let cursor = parse_notice_cursor(&raw)?;
        // Newest first; stop at the first bundle not newer than the cursor.
        let mut newer: Vec<Notice> = Vec::new();
        let mut offset = 0;
        'pages: loop {
            let page = self
                .store
                .notice_list(&NoticeQuery {
                    limit: NOTICE_PAGE,
                    offset,
                    ..NoticeQuery::default()
                })
                .map_err(|e| e.to_string())?;
            let len = page.items.len();
            for notice in page.items {
                if !notice_newer(&notice, &cursor) {
                    break 'pages;
                }
                newer.push(notice);
            }
            if len < NOTICE_PAGE {
                break;
            }
            offset += len;
        }
        let Some(head) = newer.first() else {
            return Ok(());
        };
        let head = notice_cursor(head)?;
        newer.reverse();
        let sources: Vec<CosTriageSource> = newer
            .iter()
            .filter_map(|n| notice_source(n, &policy))
            .collect();
        let inserted = self
            .store
            .cos_triage_ingest_batch(COS_TRIAGE_NOTICES_SOURCE, &head, &sources, now)
            .map_err(|e| e.to_string())?;
        if inserted > 0 {
            self.triage.dirty = true;
        }
        Ok(())
    }

    /// Start one CoS run over up to twenty pending items in each idle CoS thread that has
    /// pending items (ADR 2026-10-22-cos-fixed-threads D3). While a thread's run is live its
    /// items stay pending (durable queue).
    pub(crate) fn triage_launch(&mut self, dispatcher: &mut Dispatcher) -> Result<(), String> {
        if !self.triage.dirty {
            return Ok(());
        }
        let threads = self
            .store
            .cos_triage_pending_threads()
            .map_err(|e| e.to_string())?;
        // Busy threads keep their items for the next pass.
        let mut more = false;
        for thread in threads {
            if self.running.contains_key(&thread) || !self.has_capacity(dispatcher) {
                more = true;
                continue;
            }
            self.start_thread(dispatcher, &thread, ClaimSource::Triage)?;
            more |= self.triage.dirty;
        }
        self.triage.dirty = more;
        Ok(())
    }

    /// `ClaimSource::Triage`: bind at most twenty items to a system message
    /// and the new chat run in one transaction (store contract).
    pub(super) fn triage_claim(
        &mut self,
        thread_id: &str,
        run_id: &str,
        now: OffsetDateTime,
    ) -> Result<Option<ChatRun>, String> {
        let Some(claim) = self
            .store
            .cos_triage_claim(thread_id, run_id, now)
            .map_err(|e| e.to_string())?
        else {
            self.triage.dirty = false;
            return Ok(None);
        };
        // More than a batch: claim the rest after this run.
        self.triage.dirty = claim.item_ids.len() >= COS_TRIAGE_BATCH_MAX;
        self.store
            .chat_run_get(&claim.thread_id, run_id)
            .map(Some)
            .map_err(|e| e.to_string())
    }
}
