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
//!   going). This deadline is independent of the run's wall-clock limit.
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
use task_ops::human_inbox::InboxItem;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::launch::CosChatLaunch;
use super::triage::COS_TRIAGE_NOTICE_KIND;
use crate::dispatcher::Dispatcher;

/// Fixed headline of every fallback (D6).
pub const COS_FALLBACK_HEADLINE: &str = "CoS 不在のため直接通知";
/// Used when neither CoS nor the source offers a recommendation.
pub const COS_FALLBACK_NO_RECOMMENDATION: &str = "CoS の推奨なし";
/// Bound of the rendered text (Discord content limit is 2000).
const TEXT_MAX_CHARS: usize = 1900;

/// One unresolved item with the state of its run and its route, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FallbackCandidate {
    pub item_id: String,
    pub source_kind: String,
    pub source_key: String,
    pub source_revision: String,
    pub created_at: String,
    pub run_id: Option<String>,
    pub run_state: Option<String>,
    pub run_reason: Option<String>,
    /// `cos_notification_routes.route` already claimed for this item.
    pub route: Option<String>,
}

/// Why CoS is considered absent for one item, or `None` while it may still answer.
pub(crate) fn unavailable_reason(
    c: &FallbackCandidate,
    enabled: bool,
    unavailable_after_secs: u64,
    now: OffsetDateTime,
) -> Option<String> {
    if !enabled {
        return Some("CoS が無効（cos.enabled=false）".into());
    }
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
            _ => {}
        }
    }
    let created = OffsetDateTime::parse(&c.created_at, &Rfc3339).ok()?;
    let waited = (now - created).whole_seconds();
    let limit = i64::try_from(unavailable_after_secs).unwrap_or(i64::MAX);
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
             r.state,r.reason,n.route \
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
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// What the fixed text says about the original wait.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct FallbackSource {
    pub summary: String,
    pub options: Vec<String>,
    pub recommended: Option<String>,
    pub web_path: String,
}

impl FallbackSource {
    fn from_inbox(item: &InboxItem) -> Self {
        let mut summary = item.title.clone();
        if let Some(detail) = item.detail.as_deref().filter(|d| !d.is_empty()) {
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

/// The hand-off line and its card (D2 of the inbox-thread ADR). Pure.
pub(crate) fn handoff_text(
    c: &FallbackCandidate,
    reason: &str,
    report: Option<&task_core::chat::triage::CosTriageItemReport>,
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
    if let Some(d) = decision {
        let point = if d.summary.is_empty() {
            title.clone()
        } else {
            d.summary.clone()
        };
        text.push_str(&format!("\n決めること: {point}"));
        if !d.options.is_empty() {
            let labels: Vec<&str> = d.options.iter().map(|o| o.label.as_str()).collect();
            text.push_str(&format!("\n選択肢: {}", labels.join(" / ")));
        } else {
            text.push_str("\n選択肢: （自由文。web で回答）");
        }
        if !d.web_path.is_empty() {
            text.push_str(&format!("\n回答: {}", d.web_path));
            href = d.web_path.clone();
        }
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

impl CosChatLaunch {
    /// One fallback pass. Errors are logged per item and retried next tick;
    /// they never create a new wait.
    pub(crate) fn triage_fallback(&mut self, dispatcher: &Dispatcher, now: OffsetDateTime) {
        let found = match candidates(&self.config.db_path) {
            Ok(found) => found,
            Err(error) => {
                tracing::warn!(%error, "CoS fallback scan failed");
                return;
            }
        };
        let enabled = self.config.enabled;
        let after = self.config.triage.unavailable_after_secs;
        let due: Vec<(FallbackCandidate, String)> = found
            .into_iter()
            .filter_map(|c| {
                if c.route.is_some() {
                    // Claimed before a crash: only the state mark is missing.
                    return Some((c, "退避の記録を回収（再起動）".to_owned()));
                }
                unavailable_reason(&c, enabled, after, now).map(|r| (c, r))
            })
            .collect();
        if due.is_empty() {
            return;
        }
        // The derived inbox is built at most once per pass, and only when needed.
        let mut inbox: Option<HashMap<String, InboxItem>> = None;
        for (c, reason) in due {
            if let Err(error) = self.fallback_one(dispatcher, &mut inbox, &c, &reason, now) {
                tracing::warn!(%error, item_id = %c.item_id, "CoS fallback failed");
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
    ) -> Result<(), String> {
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
                return Ok(());
            };
            let (_, body) = fallback_body(c, reason, &source);
            let claimed = self
                .store
                .cos_triage_outbox_claim(&c.item_id, "fallback", &body, now)
                .map_err(|e| e.to_string())?;
            if claimed.is_none() {
                // An escalation (or a resolution) won the race; that path owns the item.
                return Ok(());
            }
        }
        self.store
            .cos_triage_resolve(&c.item_id, "fallback", None, Some(reason), now)
            .map_err(|e| e.to_string())?;
        self.handoff_message(c, reason, now);
        Ok(())
    }

    fn notice_source(&self, c: &FallbackCandidate) -> FallbackSource {
        let notice = c
            .source_key
            .parse::<NoticeId>()
            .ok()
            .and_then(|id| self.store.notice_get(id).ok().flatten());
        FallbackSource {
            summary: notice.map_or_else(
                || c.source_key.clone(),
                |n| format!("{} — {}", n.title, n.summary),
            ),
            options: Vec::new(),
            recommended: None,
            web_path: "/notifications".into(),
        }
    }

    /// D6: the next CoS session sees the item as already with the person. ADR
    /// 2026-10-07-cos-inbox-thread-conversation D2: the line names the wait and what the
    /// person must decide; the card stays answerable while the wait is open.
    fn handoff_message(&mut self, c: &FallbackCandidate, reason: &str, now: OffsetDateTime) {
        let thread = match self.inbox_thread() {
            Ok(Some(thread)) => thread,
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(%error, "CoS fallback inbox thread lookup failed");
                return;
            }
        };
        let report = self.store.cos_triage_item_report(&c.item_id).ok().flatten();
        let (text, card) = handoff_text(c, reason, report.as_ref());
        if let Err(error) = self.store.chat_system_message_add_once(
            &thread,
            &handoff_key(&c.item_id),
            &text,
            &[card],
            now,
        ) {
            tracing::warn!(%error, item_id = %c.item_id, "CoS fallback hand-off message failed");
        }
    }
}
