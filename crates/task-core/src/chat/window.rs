//! ADR 2026-10-28-single-human-window-general-cos: the person talks to the general CoS only.
//! D2: an item of a project thread routed to the person posts one card (札) in the general
//! thread, keyed by the source triple; its state is read from the item every time.
//! D3: the general CoS forwards the person's own words to a project thread as a relay.
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use time::OffsetDateTime;

use super::store::{
    ChatError, ChatMessagePosted, MessageProvenance, NewMessage, cos_thread_ensure_tx,
    emit_message, insert_message, message_post_with, message_require, relay_record, thread_require,
};
use super::{
    ChatActor, ChatCard, ChatCardKind, ChatEscalationRef, ChatForwardRequest, ChatMessageRole,
    ChatMessageState, ChatPostMessageRequest, ChatRelayRequest, ChatRelaySource, ChatSendMode,
    ChatThreadKind,
};

/// Prefix of the card's `client_message_id` (`<prefix><kind>:<key>:<revision>`).
pub const COS_ESCALATION_CARD_KEY_PREFIX: &str = "cos-escalation-card:";

/// Item states that still wait on the person (or on CoS): the card is open.
fn open_state(state: &str) -> bool {
    matches!(state, "pending" | "running" | "escalated" | "fallback")
}

/// The card kind of an inbox source kind (the web answers decision/question/approval/plan gate
/// in place).
fn card_kind(source_kind: &str) -> ChatCardKind {
    match source_kind {
        "decision" => ChatCardKind::Decision,
        "question" => ChatCardKind::Question,
        "approval" | "authorization" => ChatCardKind::Approval,
        "phase_gate" | "plan_gate" => ChatCardKind::PlanGate,
        _ => ChatCardKind::Notice,
    }
}

/// D2: called in the transaction that creates the escalation / fallback outbox of `item_id`.
/// When the item belongs to a project thread, posts the card in the general thread once.
/// `body` is the outbox packet (`summary`, `options`, `recommended`, `web_path`, `task_id`).
pub(crate) fn post_escalation_card_tx(
    tx: &Connection,
    item_id: &str,
    body: &str,
    at: &str,
) -> Result<(), ChatError> {
    let row: Option<(String, String, String, String, String)> = tx
        .query_row(
            "SELECT source_kind,source_key,source_revision,thread_id,state FROM cos_inbox_items WHERE id=?1",
            [item_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let Some((kind, key, revision, source_thread_id, state)) = row else {
        return Ok(());
    };
    let source = thread_require(tx, &source_thread_id)?;
    if source.kind != ChatThreadKind::Project {
        return Ok(());
    }
    let client_key = format!("{COS_ESCALATION_CARD_KEY_PREFIX}{kind}:{key}:{revision}");
    let general = cos_thread_ensure_tx(tx, None, at)?;
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM chat_messages WHERE thread_id=?1 AND client_message_id=?2)",
        params![general, client_key],
        |r| r.get(0),
    )?;
    if exists {
        return Ok(());
    }
    let packet: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let summary = packet["summary"].as_str().unwrap_or_default().trim();
    let summary = if summary.is_empty() {
        format!("{kind}:{key}")
    } else {
        summary.to_owned()
    };
    // The resolve API and the direct route name the task; the fallback body links `/tasks/<id>`.
    let task_id = packet["task_id"]
        .as_str()
        .or_else(|| {
            packet["web_path"]
                .as_str()
                .and_then(|p| p.strip_prefix("/tasks/"))
                .filter(|id| !id.is_empty() && !id.contains('/'))
        })
        .map(str::to_owned);
    // ADR 2026-10-28-cos-inbox-chat-summary D3: an option with CoS's text says what choosing it means.
    let options: Vec<(String, Option<String>)> = packet["options"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|o| {
                    let label = o.as_str().or_else(|| o["label"].as_str())?;
                    let detail = o["detail"]
                        .as_str()
                        .map(str::trim)
                        .filter(|d| !d.is_empty())
                        .map(str::to_owned);
                    Some((label.to_owned(), detail))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut text = format!("{}の件: {summary}", source.title);
    if let Some(task) = &task_id {
        text.push_str(&format!("\ntask: {task}"));
    }
    if options.iter().any(|(_, d)| d.is_some()) {
        text.push_str("\n選択肢:");
        for (label, detail) in &options {
            match detail {
                Some(detail) => text.push_str(&format!("\n- {label}: {detail}")),
                None => text.push_str(&format!("\n- {label}")),
            }
        }
        if let Some(recommended) = packet["recommended"].as_str() {
            text.push_str(&format!("\n推奨: {recommended}"));
        }
    } else if !options.is_empty() {
        let labels: Vec<&str> = options.iter().map(|(l, _)| l.as_str()).collect();
        text.push_str(&format!("\n選択肢: {}", labels.join(" / ")));
        if let Some(recommended) = packet["recommended"].as_str() {
            text.push_str(&format!("（推奨: {recommended}）"));
        }
    }
    text.push_str(&format!("\n元のスレッド: {}", source.title));
    let escalation = ChatEscalationRef {
        item_id: item_id.to_owned(),
        source_kind: kind.clone(),
        source_key: key.clone(),
        source_thread_id: source_thread_id.clone(),
        project_id: source.project_id.clone(),
        task_id,
        state: state.clone(),
        open: open_state(&state),
    };
    let card = ChatCard {
        kind: card_kind(&kind),
        id: key,
        // The fallback body names the wait's title; its summary carries the whole body.
        title: packet["title"]
            .as_str()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map_or(summary, str::to_owned),
        state,
        href: packet["web_path"].as_str().unwrap_or_default().to_owned(),
        actor: ChatActor::Cos,
        reason: packet["recommendation_reason"].as_str().map(str::to_owned),
        operation_id: packet["operation_id"].as_str().map(str::to_owned),
    };
    let meta = json!({
        "posted_by": "cos_escalation_card",
        "escalation": escalation,
        "cards": [card],
    });
    let id = insert_message(
        tx,
        NewMessage {
            thread_id: &general,
            role: ChatMessageRole::Assistant,
            text: &text,
            state: ChatMessageState::Completed,
            client_message_id: Some(&client_key),
            reply_to_id: None,
            run_id: None,
            metadata: &meta,
        },
        at,
    )?;
    let message = message_require(tx, &general, &id)?;
    emit_message(tx, &message, at)?;
    Ok(())
}

/// D2: the stored card with the item's current state (not saved: read every time). A closed
/// item shows the card as `resolved`.
pub(crate) fn live_escalation(
    conn: &Connection,
    mut stored: ChatEscalationRef,
    cards: &mut [ChatCard],
) -> Result<ChatEscalationRef, ChatError> {
    let state: Option<String> = conn
        .query_row(
            "SELECT state FROM cos_inbox_items WHERE id=?1",
            [&stored.item_id],
            |r| r.get(0),
        )
        .optional()?;
    stored.state = state.unwrap_or_else(|| "resolved".into());
    stored.open = open_state(&stored.state);
    for card in cards.iter_mut().filter(|c| c.id == stored.source_key) {
        card.state = if stored.open {
            stored.state.clone()
        } else {
            "resolved".into()
        };
    }
    Ok(stored)
}

/// D3 `chat.relay_forward`: the general CoS (`from_thread`) forwards the person's message
/// `req.message_id` to the project thread `to_thread`. The text and `said_at` are the person's
/// own (never CoS's words); the scope needs 1..=20 tasks of the project and an expiry.
pub fn chat_relay_forward_tx(
    tx: &Connection,
    from_thread: &str,
    to_thread: &str,
    req: &ChatForwardRequest,
    relayed_by: &str,
    now: OffsetDateTime,
) -> Result<ChatMessagePosted, ChatError> {
    let from = thread_require(tx, from_thread)?;
    if from.kind != ChatThreadKind::General {
        return Err(ChatError::Invalid(format!(
            "forwards start from the general CoS thread (thread {from_thread} is {:?})",
            from.kind
        )));
    }
    let to = thread_require(tx, to_thread)?;
    if to.kind != ChatThreadKind::Project {
        return Err(ChatError::Invalid(format!(
            "forwards go to a project CoS thread (thread {to_thread} is {:?})",
            to.kind
        )));
    }
    let source = message_require(tx, from_thread, &req.message_id)?;
    let author = source.author.clone().unwrap_or_default();
    if source.role != ChatMessageRole::User
        || source.state == ChatMessageState::Cancelled
        || source.relay.is_some()
        || !author.starts_with("human:")
    {
        return Err(ChatError::Invalid(format!(
            "message {} is not a person's own message of the general thread",
            req.message_id
        )));
    }
    if req.task_ids.is_empty() {
        return Err(ChatError::Invalid(
            "a forward names the tasks it applies to (task_ids must not be empty)".into(),
        ));
    }
    if req.expires_at.trim().is_empty() {
        return Err(ChatError::Invalid("a forward needs expires_at".into()));
    }
    let relay_req = ChatRelayRequest {
        client_message_id: req.client_message_id.clone(),
        text: source.text.clone(),
        said_at: source.created_at.clone(),
        task_ids: req.task_ids.clone(),
        expires_at: Some(req.expires_at.clone()),
    };
    let mut relay = relay_record(tx, to_thread, &relay_req, relayed_by, now)?;
    relay.source = Some(ChatRelaySource {
        thread_id: from_thread.to_owned(),
        message_id: source.id.clone(),
        author: Some(author),
    });
    message_post_with(
        tx,
        to_thread,
        &ChatPostMessageRequest {
            client_message_id: req.client_message_id.clone(),
            text: source.text,
            attachment_ids: Vec::new(),
            reply_to_id: None,
            mode: ChatSendMode::Queue,
            resume_queue: false,
        },
        &MessageProvenance {
            author: Some(relayed_by.to_owned()),
            relay: Some(relay),
            mcp_receipt: false,
        },
        now,
    )
}

impl crate::store::SqliteStore {
    /// [`chat_relay_forward_tx`] in its own transaction (`now` is injected).
    pub fn chat_relay_forward(
        &self,
        from_thread: &str,
        to_thread: &str,
        req: &ChatForwardRequest,
        relayed_by: &str,
        now: OffsetDateTime,
    ) -> Result<ChatMessagePosted, ChatError> {
        let mut conn = super::store::writer(self)?;
        let tx = super::store::immediate(&mut conn)?;
        let posted = chat_relay_forward_tx(&tx, from_thread, to_thread, req, relayed_by, now)?;
        tx.commit()?;
        Ok(posted)
    }
}
