//! CoS outbox rendering and the final, deterministic source check.
use std::path::Path;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde_json::Value;
use task_core::chat::triage::CosTriageSource;
use task_core::{Notification, NotificationKind, NotificationStore, SqliteStore, TaskStore};
use task_ops::daemon::DaemonSnapshot;
use task_ops::human_inbox::InboxKind;
use task_ops::view::ViewContext;
use time::OffsetDateTime;

use super::{CONTENT_MAX_CHARS, NotifyConfig, excerpt};

/// A missing or invalid web URL is a visible configuration failure, never a sent notification.
pub const NO_WEB_URL: &str = "notify.web_base_url is missing or invalid";

fn field<'a>(value: &'a Value, name: &str) -> &'a str {
    value.get(name).and_then(Value::as_str).unwrap_or("")
}

/// Only the structured packet's public summary and options enter Discord. The URL is
/// supplied by the daemon; the worker cannot provide an external host.
pub fn render(row: &Notification, config: &NotifyConfig) -> Result<String, &'static str> {
    if row.kind == NotificationKind::RootDoneSummary {
        // The dispatcher fixed the whole text (CoS summary or the deterministic fallback) together with
        // the thread and task links, after checking `web_base_url` (ADR 2026-10-15 D5).
        return Ok(super::clamp_content(&row.body));
    }
    let base = config.base_url().ok_or(NO_WEB_URL)?;
    let parsed = reqwest::Url::parse(base).map_err(|_| NO_WEB_URL)?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(NO_WEB_URL);
    }
    let packet: Value = serde_json::from_str(&row.body).map_err(|_| "invalid CoS outbox packet")?;
    let path = field(&packet, "web_path");
    if !path.starts_with('/') || path.starts_with("//") || path.contains(['\r', '\n']) {
        return Err("invalid CoS web path");
    }
    let heading = match row.kind {
        NotificationKind::CosEscalation if !field(&packet, "headline").is_empty() => {
            field(&packet, "headline")
        }
        NotificationKind::CosEscalation => "CoS から判断のお願い",
        NotificationKind::CosFallback => "CoS 不在のため直接通知",
        _ => return Err("unsupported outbound route"),
    };
    // ADR 2026-10-22-cos-fixed-threads D3: which CoS (general or the project's) is asking.
    let cos = field(&packet, "cos");
    let heading = if cos.is_empty() {
        heading.to_owned()
    } else {
        format!("{} {heading}", excerpt(cos, 80))
    };
    let options = packet.get("options").and_then(Value::as_array);
    let option_text = options
        .map(|xs| {
            xs.iter()
                .take(8)
                .map(|x| {
                    format!(
                        "{}: {}",
                        excerpt(field(x, "key"), 32),
                        excerpt(field(x, "label"), 90)
                    )
                })
                .collect::<Vec<_>>()
                .join(" / ")
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "web で回答".into());
    let recommended = field(&packet, "recommended");
    let reason = field(&packet, "recommendation_reason");
    // D3: 推奨なしは null と理由。理由があれば推奨なしでも落とさない。
    let recommendation = if recommended.is_empty() && reason.is_empty() {
        "CoS の推奨なし".to_string()
    } else if recommended.is_empty() {
        format!("CoS の推奨なし（{}）", excerpt(reason, 180))
    } else if reason.is_empty() {
        excerpt(recommended, 60)
    } else {
        format!("{}（{}）", excerpt(recommended, 60), excerpt(reason, 180))
    };
    let blocking = field(&packet, "blocking");
    let unavailable = field(&packet, "reason");
    let mut body = format!(
        "{heading}\n要点: {}\n選択肢: {}\n推奨: {recommendation}\n止まっている範囲: {}",
        excerpt(field(&packet, "summary"), 350),
        excerpt(&option_text, 500),
        if blocking.is_empty() {
            "元の待ち"
        } else {
            blocking
        },
    );
    // ADR 2026-10-30-cos-review-open-waits D3: where the item comes from and what an answer does.
    let background = field(&packet, "background");
    if !background.is_empty() {
        body.push_str(&format!("\n背景: {}", excerpt(background, 300)));
    }
    let on_answer = field(&packet, "on_answer");
    if !on_answer.is_empty() {
        body.push_str(&format!("\n答えると: {}", excerpt(on_answer, 200)));
    }
    if row.kind == NotificationKind::CosFallback && !unavailable.is_empty() {
        body.push_str(&format!("\n不在理由: {}", excerpt(unavailable, 180)));
    }
    let suffix = format!("\n{}{}\n回答はリンク先で", base.trim_end_matches('/'), path);
    if suffix.chars().count() >= CONTENT_MAX_CHARS {
        return Err("CoS web link is too long");
    }
    let budget = CONTENT_MAX_CHARS.saturating_sub(suffix.chars().count());
    if body.chars().count() > budget {
        body = body
            .chars()
            .take(budget.saturating_sub(1))
            .collect::<String>()
            + "…";
    }
    Ok(body + &suffix)
}

fn db(db: &Path) -> Result<Connection, String> {
    Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| e.to_string())
}

/// The source of a CoS outbound row, checked immediately before POST.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceCheck {
    /// The same revision is still unresolved.
    Live,
    /// Closed, superseded by a newer revision, or never linked.
    Resolved,
    /// ADR 2026-10-29-stagnation-triage-notify-wiring D2: a stagnation kind and no stagnation
    /// snapshot. Not knowing is neither live nor resolved.
    Unknown,
}

/// What the tick does with a CoS outbound row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sendable {
    Send,
    /// Marked superseded in the same transaction (and the item resolved if its source closed).
    Withdrawn,
    /// Left pending, untouched: the source could not be checked (stagnation unknown).
    Held,
}

impl Sendable {
    pub fn is_send(self) -> bool {
        self == Sendable::Send
    }
}

/// Confirm the source is still the same unresolved revision immediately before POST.
/// `snapshot` carries the daemon's stagnation check (`Dispatcher::stagnation_snapshot`); the
/// stagnation kinds are `Unknown` without it, never `Resolved`.
pub fn source_unresolved(
    db_path: &Path,
    store: &dyn TaskStore,
    view: &ViewContext,
    snapshot: Option<&DaemonSnapshot>,
    row: &Notification,
    now: OffsetDateTime,
) -> Result<SourceCheck, String> {
    let conn = db(db_path)?;
    let source: Option<(String, String, String)> = conn
        .query_row(
            "SELECT source_kind,source_key,source_revision FROM cos_inbox_items WHERE id=?1",
            [&row.key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((kind, key, revision)) = source else {
        return Ok(SourceCheck::Resolved);
    };
    let newer: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM cos_inbox_items WHERE source_kind=?1 AND source_key=?2 AND created_at>(SELECT created_at FROM cos_inbox_items WHERE id=?3))",
        params![kind, key, row.key], |r| r.get(0),
    ).map_err(|e| e.to_string())?;
    if newer {
        return Ok(SourceCheck::Resolved);
    }
    if kind == "notice" {
        use task_core::feed::NoticeQuery;
        let mut query = NoticeQuery {
            limit: 500,
            ..NoticeQuery::default()
        };
        loop {
            let page = store.notice_list(&query).map_err(|e| e.to_string())?;
            if page.items.iter().any(|n| {
                n.is_unread() && n.id.to_string() == key && n.count.to_string() == revision
            }) {
                return Ok(SourceCheck::Live);
            }
            let count = page.items.len();
            query.offset += count;
            if count == 0 || query.offset >= page.total as usize {
                return Ok(SourceCheck::Resolved);
            }
        }
    }
    if snapshot.is_none() && InboxKind::is_stagnation_kind(&kind) {
        return Ok(SourceCheck::Unknown);
    }
    let inbox = task_ops::human_inbox::human_inbox(store, snapshot, view, now, &|_, _| Vec::new())
        .map_err(|e| e.to_string())?;
    let live = inbox
        .items
        .iter()
        .any(|i| i.kind.as_str() == kind && i.id == key && i.created_at == revision);
    Ok(if live {
        SourceCheck::Live
    } else {
        SourceCheck::Resolved
    })
}

/// The store makes the final withdrawal atomic with the notification state.
pub fn withdraw_if_resolved(
    store: &SqliteStore,
    db_path: &Path,
    view: &ViewContext,
    snapshot: Option<&DaemonSnapshot>,
    row: &Notification,
    now: OffsetDateTime,
) -> Result<Sendable, String> {
    let check = source_unresolved(db_path, store, view, snapshot, row, now)?;
    // Unknown is checked as live: an answered/observed/resolved item or a sent row is still
    // withdrawn by the store, but an open one is held instead of sent.
    let sendable = store
        .cos_triage_outbox_sendable(&row.id.to_string(), check != SourceCheck::Resolved, now)
        .map_err(|e| e.to_string())?;
    Ok(match (sendable, check) {
        (false, _) => Sendable::Withdrawn,
        (true, SourceCheck::Unknown) => Sendable::Held,
        (true, _) => Sendable::Send,
    })
}

/// Call once at daemon startup. Legacy unsent rows become CoS input and are
/// superseded in the same transaction as the cos_v1 route marker.
pub fn cutover(store: &SqliteStore, view: &ViewContext, now: OffsetDateTime) -> Result<(), String> {
    if store
        .cos_triage_route_version()
        .map_err(|e| e.to_string())?
        .as_deref()
        == Some("cos_v1")
    {
        return Ok(());
    }
    let inbox = task_ops::human_inbox::human_inbox(store, None, view, now, &|_, _| Vec::new())
        .map_err(|e| e.to_string())?;
    let mut initial: Vec<CosTriageSource> = inbox
        .items
        .iter()
        .map(|i| CosTriageSource {
            source_kind: i.kind.as_str().into(),
            source_key: i.id.clone(),
            source_revision: i.created_at.clone(),
            source_event_id: None,
            operation_id: None,
            summary: i.title.clone(),
            detail: String::new(),
            policy_version: "1".into(),
            task_id: i
                .task
                .iter()
                .chain(i.blocking.root.iter())
                .chain(i.blocking.tasks.iter())
                .map(|t| t.id.to_string())
                .next(),
        })
        .collect();
    let mut legacy = Vec::new();
    for row in store.notification_pending().map_err(|e| e.to_string())? {
        if matches!(
            row.kind,
            NotificationKind::CosEscalation | NotificationKind::CosFallback
        ) {
            continue;
        }
        let key = row.id.to_string();
        initial.push(CosTriageSource {
            source_kind: "legacy_notification".into(),
            source_key: key.clone(),
            source_revision: "1".into(),
            source_event_id: None,
            operation_id: None,
            summary: excerpt(&row.body, 120),
            detail: String::new(),
            policy_version: "1".into(),
            task_id: None,
        });
        legacy.push((
            key,
            "legacy_notification".into(),
            row.id.to_string(),
            "1".into(),
        ));
    }
    let head = store.latest_event_id().map_err(|e| e.to_string())?;
    store
        .cos_triage_cutover("events", &head.to_string(), &initial, &legacy, now)
        .map_err(|e| e.to_string())?;
    Ok(())
}
