//! CoS unavailable fallback (ADR 2026-10-05-cos-chat-home D6「通知の一本化と退避」,
//! store contract: ADR 2026-10-06-cos-inbox-triage).
//!
//! Deterministic and LLM-free. Every tick the dispatcher looks at the unresolved
//! `cos_inbox_items` rows and routes an item straight to the person when CoS
//! cannot answer it:
//!
//! - `cos.enabled=false`;
//! - its CoS run ended without an outcome for it: failed or crashed (this covers
//!   launch failures such as every candidate account out of quota or logged
//!   out), stopped/interrupted, or even completed successfully;
//! - nothing answered it within `unavailable_after_secs` of its arrival,
//!   whatever the reason (capacity wait, stopped queue, disk guard, a run still
//!   going). This deadline is independent of the run's wall-clock limit;
//! - an authorization item (ADR 2026-10-29-cos-authorization-fallback-after-triage)
//!   is not timed out by `unavailable_after_secs` while CoS works on it: the
//!   person is told only after CoS escalates it or its run ends without an
//!   outcome. Its safety net is `authorization_unavailable_after_secs`, counted
//!   from its arrival while no run has started and from the run's start while
//!   the run is live.
//!
//! The route is taken through the store's outbox claim (`route=fallback`, one
//! row per source triple, shared with escalation), so a fallback and an
//! escalation racing on the same revision leave exactly one pending outbox.
//! The item becomes `fallback`; the claim never auto-approves anything. A
//! keyed system message in the inbox thread tells the next CoS session that
//! the item is already with the person, and `cos_triage_claim` only takes
//! `pending` rows, so a recovered CoS neither resends nor answers the same
//! revision. The scan reads SQLite only, so a daemon restart recovers every
//! unresolved row (including a claim whose state mark was lost to a crash).
//! The outbox is a `notifications` row, not a notice, so a failed delivery
//! never becomes a new CoS wait.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::Connection;
use serde_json::json;
use task_core::chat::{ChatActor, ChatCard};
use task_core::{NoticeId, NoticeStore};
use task_ops::human_inbox::{InboxItem, InboxKind};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::launch::CosChatLaunch;
use super::triage::COS_TRIAGE_NOTICE_KIND;
use crate::dispatcher::Dispatcher;
use task_core::chat::triage::{
    COS_FALLBACK_SUMMARY_KEY_PREFIX, COS_FALLBACK_SUMMARY_MAX_ATTEMPTS, COS_FALLBACK_SUMMARY_PREFIX,
};

/// Fixed headline of every fallback (D6).
pub const COS_FALLBACK_HEADLINE: &str = "CoS 不在のため直接通知";
/// Used when neither CoS nor the source offers a recommendation.
pub const COS_FALLBACK_NO_RECOMMENDATION: &str = "CoS の推奨なし";
/// Bound of the rendered text (Discord content limit is 2000).
const TEXT_MAX_CHARS: usize = 1900;
/// Bound of the body quoted in the chat hand-off line (ADR 2026-10-28-cos-inbox-chat-summary D1).
/// A longer body is cut with the link to its full text.
pub(crate) const CHAT_BODY_MAX_CHARS: usize = 4000;

/// One unresolved item with the state of its run and its route, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FallbackCandidate {
    pub item_id: String,
    pub source_kind: String,
    pub source_key: String,
    pub source_revision: String,
    pub created_at: String,
    /// The title and body of the revision as ingested (ADR 2026-10-28-cos-inbox-chat-summary D2).
    pub summary: String,
    pub detail: String,
    pub run_id: Option<String>,
    pub run_state: Option<String>,
    pub run_reason: Option<String>,
    /// `chat_runs.started_at` of that run.
    pub run_started_at: Option<String>,
    /// `cos_notification_routes.route` already claimed for this item.
    pub route: Option<String>,
}

/// The deadlines of the fallback (`[cos.triage]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FallbackDeadlines {
    /// `unavailable_after_secs`: every item except authorization.
    pub after_secs: u64,
    /// `authorization_unavailable_after_secs`: the safety net of an authorization item.
    pub authorization_after_secs: u64,
}

/// The items whose Discord message waits for CoS's first response
/// (ADR 2026-10-29-cos-authorization-fallback-after-triage D1).
pub(crate) fn waits_for_first_response(source_kind: &str) -> bool {
    source_kind == InboxKind::Authorization.as_str()
}

fn seconds_since(at: &str, now: OffsetDateTime) -> Option<i64> {
    let at = OffsetDateTime::parse(at, &Rfc3339).ok()?;
    Some((now - at).whole_seconds())
}

/// Why CoS is considered absent for one item, or `None` while it may still answer.
pub(crate) fn unavailable_reason(
    c: &FallbackCandidate,
    enabled: bool,
    deadlines: FallbackDeadlines,
    now: OffsetDateTime,
) -> Option<String> {
    if !enabled {
        return Some("CoS が無効（cos.enabled=false）".into());
    }
    let mut live = false;
    if c.run_id.is_some() {
        let detail = c.run_reason.as_deref().unwrap_or("理由なし");
        match c.run_state.as_deref() {
            Some("failed") => {
                let lower = detail.to_ascii_lowercase();
                return Some(
                    if lower.contains("quota")
                        || lower.contains("login")
                        || lower.contains("no usable account")
                    {
                        format!("CoS の全候補が quota 切れ・ログイン不可: {detail}")
                    } else {
                        format!("CoS run が失敗: {detail}")
                    },
                );
            }
            Some("stopped") | Some("interrupted") => {
                return Some(format!("CoS run が途中で止まった: {detail}"));
            }
            Some("completed") => {
                return Some("CoS run は終了したがこの項目は未処理".into());
            }
            // The claim writes the run in the same transaction; a missing row is a lost run.
            None => return Some("CoS run の記録が無い（crash）".into()),
            _ => live = true,
        }
    }
    if waits_for_first_response(&c.source_kind) {
        let limit = i64::try_from(deadlines.authorization_after_secs).unwrap_or(i64::MAX);
        return if live {
            // The run's own wall-clock limit ends it first; this only catches a run that hangs.
            let since = c.run_started_at.as_deref().unwrap_or(&c.created_at);
            let ran = seconds_since(since, now)?;
            (ran > limit).then(|| {
                format!("CoS の一次対応が {limit} 秒を超えて終わらない（run の経過 {ran} 秒）")
            })
        } else {
            let waited = seconds_since(&c.created_at, now)?;
            (waited > limit).then(|| {
                format!(
                    "CoS の一次対応が {limit} 秒を超えても始まらない（容量待ち・停止を含む。待ち {waited} 秒）"
                )
            })
        };
    }
    let waited = seconds_since(&c.created_at, now)?;
    let limit = i64::try_from(deadlines.after_secs).unwrap_or(i64::MAX);
    (waited > limit).then(|| {
        format!("CoS の未応答が {limit} 秒を超えた（容量待ち・停止を含む。待ち {waited} 秒）")
    })
}

fn open(db: &Path) -> Result<Connection, String> {
    let conn = Connection::open(db).map_err(|e| e.to_string())?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    Ok(conn)
}

/// Unresolved rows, plus a fallback route whose state mark a crash lost.
/// Rows routed to escalation belong to the resolve API and are skipped.
pub(crate) fn candidates(db: &Path) -> Result<Vec<FallbackCandidate>, String> {
    let conn = open(db)?;
    let mut stmt = conn
        .prepare(
            "SELECT i.id,i.source_kind,i.source_key,i.source_revision,i.created_at,i.run_id,\
             r.state,r.reason,n.route,i.summary,i.detail,r.started_at \
             FROM cos_inbox_items i \
             LEFT JOIN chat_runs r ON r.run_id=i.run_id \
             LEFT JOIN cos_notification_routes n ON n.item_id=i.id \
             WHERE i.state IN ('pending','running') AND (n.route IS NULL OR n.route='fallback') \
             ORDER BY i.created_at,i.id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(FallbackCandidate {
                item_id: r.get(0)?,
                source_kind: r.get(1)?,
                source_key: r.get(2)?,
                source_revision: r.get(3)?,
                created_at: r.get(4)?,
                run_id: r.get(5)?,
                run_state: r.get(6)?,
                run_reason: r.get(7)?,
                route: r.get(8)?,
                summary: r.get(9)?,
                detail: r.get(10)?,
                run_started_at: r.get(11)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// What the fixed text says about the original wait.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct FallbackSource {
    /// The title of the wait (the card title of the general thread).
    pub title: String,
    pub summary: String,
    pub options: Vec<String>,
    pub recommended: Option<String>,
    pub web_path: String,
}

impl FallbackSource {
    fn from_inbox(item: &InboxItem) -> Self {
        let mut summary = item.title.clone();
        // ADR 2026-10-28-cos-inbox-chat-summary D1: the unclipped body, not the 600-char detail.
        if let Some(detail) = item
            .detail_full
            .as_deref()
            .or(item.detail.as_deref())
            .filter(|d| !d.is_empty())
        {
            summary.push_str(" — ");
            summary.push_str(detail);
        }
        let recommended = item.recommended.as_ref().map(|key| {
            item.options
                .iter()
                .find(|o| &o.key == key)
                .map_or_else(|| key.clone(), |o| o.label.clone())
        });
        Self {
            title: item.title.clone(),
            summary,
            options: item.options.iter().map(|o| o.label.clone()).collect(),
            recommended,
            // Same derivation as the resolve API's escalation link.
            web_path: match &item.task {
                Some(task) => format!("/tasks/{}", task.id),
                None => "/inbox".into(),
            },
        }
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The fixed fallback text and the outbox body (JSON, same envelope as escalation).
pub(crate) fn fallback_body(
    c: &FallbackCandidate,
    reason: &str,
    source: &FallbackSource,
) -> (String, String) {
    let options = if source.options.is_empty() {
        "（選択肢なし。web で回答）".to_owned()
    } else {
        format!("{}（自由文は web で回答）", source.options.join(" / "))
    };
    let recommendation = source.recommended.as_ref().map_or_else(
        || COS_FALLBACK_NO_RECOMMENDATION.to_owned(),
        |r| format!("{r}（元の待ちの提示。CoS の推奨ではない）"),
    );
    let text = truncate(
        &format!(
            "{COS_FALLBACK_HEADLINE}\n不在理由: {reason}\n要点: {}\n選択肢: {options}\n推奨: {recommendation}\n回答: {}",
            source.summary, source.web_path
        ),
        TEXT_MAX_CHARS,
    );
    let body = json!({
        "route": "fallback",
        "headline": COS_FALLBACK_HEADLINE,
        "item_id": c.item_id,
        "source_kind": c.source_kind,
        "source_key": c.source_key,
        "source_revision": c.source_revision,
        "unavailable_reason": reason,
        "title": source.title,
        "summary": source.summary,
        "options": source.options,
        "recommended": source.recommended,
        "recommendation_text": recommendation,
        "web_path": source.web_path,
        "text": text,
    });
    (text, body.to_string())
}

/// Idempotency key of the hand-off message in the inbox thread.
fn handoff_key(item_id: &str) -> String {
    format!("cos-fallback:{item_id}")
}

/// The body of one notice item for the person (ADR 2026-10-28-cos-inbox-chat-summary D2): the
/// title and summary of the revision as ingested. The bundle may have grown since (a later event
/// is a new revision with its own row), so the live notice is read only for rows ingested before
/// migration 0101 and only while it is still the same revision.
pub(crate) fn notice_text(c: &FallbackCandidate, store: &dyn NoticeStore) -> String {
    if !c.detail.trim().is_empty() {
        return format!("{} — {}", c.summary, c.detail);
    }
    let live = c
        .source_key
        .parse::<NoticeId>()
        .ok()
        .and_then(|id| store.notice_get(id).ok().flatten())
        .filter(|n| n.count.to_string() == c.source_revision);
    match live {
        Some(n) => format!("{} — {}", n.title, n.summary),
        None if !c.summary.trim().is_empty() => c.summary.clone(),
        None => c.source_key.clone(),
    }
}

/// `text` cut at `max` chars with where to read the rest (ADR 2026-10-28-cos-inbox-chat-summary D1).
pub(crate) fn clip_with_link(text: &str, max: usize, full_at: &str) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let head: String = text.chars().take(max).collect();
    format!("{head}…（続きは {full_at}）")
}

/// The hand-off line and its card (D2 of the inbox-thread ADR). Pure.
///
/// ADR 2026-10-28-cos-inbox-chat-summary D1: the line quotes the whole body of the wait (the
/// ingested revision, up to `CHAT_BODY_MAX_CHARS` with the link to the rest), not the 600-char
/// inbox detail, so the person can read what happened and every option's text in the chat.
pub(crate) fn handoff_text(
    c: &FallbackCandidate,
    reason: &str,
    report: Option<&task_core::chat::triage::CosTriageItemReport>,
    summary_requested: bool,
) -> (String, ChatCard) {
    let title = report
        .map(|r| r.title())
        .unwrap_or_else(|| format!("{}:{}", c.source_kind, c.source_key));
    let kind = super::digest::kind_label(&c.source_kind);
    let decision = report.and_then(|r| r.decision.as_ref());
    let mut text = format!(
        "{COS_FALLBACK_HEADLINE}: 「{title}」（{kind}）は人へ委ねた（{reason}）。CoS はこの件を再送・代答しない。"
    );
    let mut href = "/inbox".to_owned();
    if let Some(d) = decision
        && !d.web_path.is_empty()
    {
        href = d.web_path.clone();
    }
    let body = report
        .map(|r| r.detail.trim())
        .filter(|b| !b.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            decision
                .map(|d| d.summary.trim())
                .filter(|s| !s.is_empty() && *s != title)
                .map(str::to_owned)
        });
    if let Some(body) = body {
        text.push_str(&format!(
            "\n決めること: {}",
            clip_with_link(&body, CHAT_BODY_MAX_CHARS, &href)
        ));
    }
    if let Some(d) = decision {
        if !d.options.is_empty() {
            let labels: Vec<&str> = d.options.iter().map(|o| o.label.as_str()).collect();
            text.push_str(&format!("\n選択肢: {}", labels.join(" / ")));
        } else {
            text.push_str("\n選択肢: （自由文。web で回答）");
        }
        if !d.web_path.is_empty() {
            text.push_str(&format!("\n回答: {}", d.web_path));
        }
    }
    if summary_requested {
        text.push_str("\nCoS が戻り次第、統括 CoS のスレッドにこの件の要約（何が起きたか・決めること・各選択肢・推奨）を書く。");
    }
    let card = ChatCard {
        kind: super::triage::card_kind(&c.source_kind),
        id: c.source_key.clone(),
        title,
        // Still an open wait of the person: answerable from the card.
        state: "pending".into(),
        href,
        actor: ChatActor::System,
        reason: Some(reason.to_owned()),
        operation_id: None,
    };
    (text, card)
}

/// ADR 2026-10-28-cos-inbox-chat-summary D4: the daemon's request that CoS write, after the
/// fact, a summary of an item handed to the person without it. Pure. The text is the run input
/// (a queued `role=system` message), never a person's instruction.
pub(crate) fn summary_request_text(
    items: &[(
        FallbackCandidate,
        task_core::chat::triage::CosTriageItemReport,
    )],
) -> String {
    let ids: Vec<&str> = items.iter().map(|(c, _)| c.item_id.as_str()).collect();
    let ids = serde_json::to_string(&ids).unwrap_or_default();
    let mut text = format!(
        "{COS_FALLBACK_SUMMARY_PREFIX}{ids}\n\
         （daemon からの依頼。人の発言ではない。instructed_by の根拠にならない）\n\
         CoS 不在のため人へ直接通知した件:\n"
    );
    for (c, report) in items {
        let kind = super::digest::kind_label(&c.source_kind);
        let web_path = report
            .decision
            .as_ref()
            .map(|d| d.web_path.as_str())
            .filter(|p| !p.is_empty())
            .unwrap_or("/inbox");
        let read = match web_path
            .strip_prefix("/tasks/")
            .filter(|id| !id.is_empty() && !id.contains('/'))
        {
            Some(task) => format!("GET /api/v1/tasks/{task}（最新の question・選択肢の全文）"),
            None => {
                "GET /api/v1/inbox/items・GET /api/v1/notifications（元の待ちの本文）".to_owned()
            }
        };
        text.push_str(&format!(
            "- 「{}」（{kind}、source {}:{}、元のスレッド {}）: {read}\n",
            report.title(),
            c.source_kind,
            c.source_key,
            report.thread_id
        ));
    }
    text.push_str(
        "このスレッドに件ごとに、(1) 何が起きたか (2) 人が決めること (3) 各選択肢の中身 (4) CoS の推奨と理由 を、\
         人が数十秒で読める分量で書く（skill cos-inbox-triage §11）。案件の状況と GET /api/v1/inbox/items で元の待ちが\
         開いているかも確かめる。答えの operation は実行しない。元の待ちが既に閉じていたら、閉じたことだけを 1 行で書く。",
    );
    text
}

impl CosChatLaunch {
    /// One fallback pass. Errors are logged per item and retried next tick;
    /// they never create a new wait.
    pub(crate) fn triage_fallback(&mut self, dispatcher: &Dispatcher, now: OffsetDateTime) {
        if self.config.enabled && self.config.unavailable_reason.is_none() {
            self.retry_failed_summary_requests(now);
        }
        let found = match candidates(&self.config.db_path) {
            Ok(found) => found,
            Err(error) => {
                tracing::warn!(%error, "CoS fallback scan failed");
                return;
            }
        };
        let enabled = self.config.enabled;
        let deadlines = FallbackDeadlines {
            after_secs: self.config.triage.unavailable_after_secs,
            authorization_after_secs: self.config.triage.authorization_unavailable_after_secs,
        };
        let due: Vec<(FallbackCandidate, String)> = found
            .into_iter()
            .filter_map(|c| {
                if c.route.is_some() {
                    // Claimed before a crash: only the state mark is missing.
                    return Some((c, "退避の記録を回収（再起動）".to_owned()));
                }
                unavailable_reason(&c, enabled, deadlines, now).map(|r| (c, r))
            })
            .collect();
        if due.is_empty() {
            return;
        }
        // The derived inbox is built at most once per pass, and only when needed.
        let mut inbox: Option<HashMap<String, InboxItem>> = None;
        let mut handed = Vec::new();
        for (c, reason) in due {
            match self.fallback_one(dispatcher, &mut inbox, &c, &reason, now) {
                Ok(Some(report)) => handed.push((c, report)),
                Ok(None) => {}
                Err(error) => tracing::warn!(%error, item_id = %c.item_id, "CoS fallback failed"),
            }
        }
        // D4: one summary request per pass (at most a triage batch each), not one run per item.
        if self.config.enabled && self.config.unavailable_reason.is_none() {
            for chunk in handed.chunks(task_core::chat::triage::COS_TRIAGE_BATCH_MAX) {
                self.queue_summary_request(chunk, now);
            }
        }
    }

    fn fallback_one(
        &mut self,
        dispatcher: &Dispatcher,
        inbox: &mut Option<HashMap<String, InboxItem>>,
        c: &FallbackCandidate,
        reason: &str,
        now: OffsetDateTime,
    ) -> Result<Option<task_core::chat::triage::CosTriageItemReport>, String> {
        if c.route.is_none() {
            let source = if c.source_kind == COS_TRIAGE_NOTICE_KIND {
                Some(self.notice_source(c))
            } else {
                if inbox.is_none() {
                    let built = self.human_inbox_items(dispatcher, now)?;
                    *inbox = Some(
                        built
                            .into_iter()
                            .map(|item| (item.id.clone(), item))
                            .collect(),
                    );
                }
                inbox
                    .as_ref()
                    .and_then(|items| items.get(&c.source_key))
                    .filter(|item| item.kind.as_str() == c.source_kind)
                    .map(FallbackSource::from_inbox)
            };
            let Some(source) = source else {
                // The wait closed meanwhile: nothing to tell the person.
                self.store
                    .cos_triage_resolve(&c.item_id, "resolved", None, Some("source closed"), now)
                    .map_err(|e| e.to_string())?;
                return Ok(None);
            };
            let (_, body) = fallback_body(c, reason, &source);
            let claimed = self
                .store
                .cos_triage_outbox_claim(&c.item_id, "fallback", &body, now)
                .map_err(|e| e.to_string())?;
            if claimed.is_none() {
                // An escalation (or a resolution) won the race; that path owns the item.
                return Ok(None);
            }
        }
        self.store
            .cos_triage_resolve(&c.item_id, "fallback", None, Some(reason), now)
            .map_err(|e| e.to_string())?;
        Ok(self.handoff_message(c, reason, now))
    }

    fn notice_source(&self, c: &FallbackCandidate) -> FallbackSource {
        FallbackSource {
            title: c.summary.clone(),
            summary: notice_text(c, self.store.as_ref()),
            options: Vec::new(),
            recommended: None,
            web_path: "/notifications".into(),
        }
    }

    /// D6: the next CoS session sees the item as already with the person. ADR
    /// 2026-10-07-cos-inbox-thread-conversation D2: the line names the wait and what the
    /// person must decide; the card stays answerable while the wait is open.
    fn handoff_message(
        &mut self,
        c: &FallbackCandidate,
        reason: &str,
        now: OffsetDateTime,
    ) -> Option<task_core::chat::triage::CosTriageItemReport> {
        // ADR 2026-10-22-cos-fixed-threads D3: the hand-off line goes to the item's CoS thread.
        let report = match self.store.cos_triage_item_report(&c.item_id) {
            Ok(Some(report)) => report,
            Ok(None) => return None,
            Err(error) => {
                tracing::warn!(%error, item_id = %c.item_id, "CoS fallback item lookup failed");
                return None;
            }
        };
        let thread = report.thread_id.clone();
        // D4: CoS writes the summary later, once it runs again (capacity wait, a slow run). Not
        // when CoS is disabled or its launch is known to fail: a request would only fail too.
        let summary_requested = self.config.enabled && self.config.unavailable_reason.is_none();
        let (text, card) = handoff_text(c, reason, Some(&report), summary_requested);
        if let Err(error) = self.store.chat_system_message_add_once(
            &thread,
            &handoff_key(&c.item_id),
            &text,
            &[card],
            now,
        ) {
            tracing::warn!(%error, item_id = %c.item_id, "CoS fallback hand-off message failed");
        }
        Some(report)
    }

    /// ADR 2026-10-28-cos-inbox-chat-summary D4: queue the summary request in the general CoS
    /// thread (the person's single window, ADR 2026-10-28-single-human-window-general-cos). The
    /// normal queue claim runs it once CoS is available; its final text is the chat reply.
    /// D4: a summary request whose CoS run failed is queued again, up to
    /// `COS_FALLBACK_SUMMARY_MAX_ATTEMPTS` runs in all.
    fn retry_failed_summary_requests(&mut self, now: OffsetDateTime) {
        let general = match self.store.cos_general_thread_id() {
            Ok(Some(thread)) => thread,
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(%error, "CoS general thread lookup failed");
                return;
            }
        };
        match self.store.chat_system_queue_retry_failed(
            &general,
            COS_FALLBACK_SUMMARY_KEY_PREFIX,
            COS_FALLBACK_SUMMARY_MAX_ATTEMPTS,
            now,
        ) {
            Ok(queued) => {
                for message in queued {
                    tracing::info!(message_id = %message.id, "CoS fallback summary request queued again");
                }
            }
            Err(error) => tracing::warn!(%error, "CoS fallback summary retry failed"),
        }
    }

    fn queue_summary_request(
        &mut self,
        items: &[(
            FallbackCandidate,
            task_core::chat::triage::CosTriageItemReport,
        )],
        now: OffsetDateTime,
    ) {
        let Some((first, _)) = items.first() else {
            return;
        };
        let general = match self.store.cos_general_thread_ensure(now) {
            Ok(thread) => thread,
            Err(error) => {
                tracing::warn!(%error, item_id = %first.item_id, "CoS general thread unavailable");
                return;
            }
        };
        let text = summary_request_text(items);
        if let Err(error) = self.store.chat_system_queue_once(
            &general,
            &format!("{COS_FALLBACK_SUMMARY_KEY_PREFIX}{}", first.item_id),
            &text,
            now,
        ) {
            tracing::warn!(%error, item_id = %first.item_id, "CoS fallback summary request failed");
        }
    }
}
