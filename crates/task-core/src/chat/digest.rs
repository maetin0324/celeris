//! S4 (ADR 2026-11-02-ts-control-layer-boundary S4-2/S4-3/S4-9): the `cos_digest` domain.
//!
//! The digest of one finished triage run (ADR 2026-10-07-cos-inbox-thread-conversation D1/D4)
//! as a pure decision: the dispatcher reads a [`CosDigestInput`] snapshot, [`cos_digest_decide`]
//! returns the commands, [`validate_cos_digest_commands`] checks them, and the dispatcher applies
//! them (`chat_assistant_message_add_once`, `cos_triage_requeue_run`).
//!
//! No LLM and no judgment: the text is assembled from `cos_inbox_items` (state, reason,
//! summary) and the decision packet CoS put in the escalation outbox. Items a stopped or
//! interrupted run left `running` go back to `pending` (a human interrupt must not push them
//! to the unavailable fallback); items a completed run left `running` are reported as
//! unhandled and the fallback leaf takes them.

use std::collections::HashSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::store::{CHAT_CLIENT_KEY_MAX_BYTES, CHAT_MESSAGE_TEXT_MAX_BYTES};
use super::triage::{COS_TRIAGE_DIGEST_KEY_PREFIX, CosTriageItemReport, CosTriageOption};
use super::{ChatActor, ChatCard, ChatCardKind};

/// The snapshot one digest decision reads. `items` is `cos_triage_run_report(run_id)` in its
/// order; `reason` is the chat run's reason (`null` when it cannot be read). `run_state` stays a
/// string: an unexpected value is reported as is (`run は <state> で終わった`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CosDigestInput {
    pub thread: String,
    pub run_id: String,
    pub run_state: String,
    pub reason: Option<String>,
    pub items: Vec<CosTriageItemReport>,
}

/// One effect the dispatcher applies, in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CosDigestCommand {
    /// The digest message of the run (`chat_assistant_message_add_once`).
    PostDigest {
        thread: String,
        idempotency_key: String,
        text: String,
        cards: Vec<ChatCard>,
        run_id: String,
    },
    /// Give the items the run left `running` back to the queue (`cos_triage_requeue_run`).
    RequeueRun { run_id: String },
}

/// Why a command list must not be applied (S4-3). `code` is the safety-violation code; the
/// format rules (6) have none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CosDigestViolation {
    /// Rule 1: the idempotency key is not `cos-triage-digest:<run_id>`.
    KeyMismatch,
    /// Rule 2: a thread or run id differs from the snapshot.
    ScopeMismatch,
    /// Rule 3: a card names no item of the run, or there are more cards than items.
    CardOutsideItems,
    /// Rule 4: `requeue_run` for a run that was not stopped or interrupted.
    RequeueStateInvalid,
    /// Rule 5: more than one command of a kind.
    DuplicateCommand,
    /// Rule 6: no `post_digest`.
    MissingPostDigest,
    /// Rule 6: the text exceeds `CHAT_MESSAGE_TEXT_MAX_BYTES`.
    TextTooLarge,
    /// Rule 6: the key exceeds `CHAT_CLIENT_KEY_MAX_BYTES`.
    KeyTooLarge,
}

impl CosDigestViolation {
    /// The `safety_violations` code (S2-8 frame); `None` for format violations.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::KeyMismatch => Some("key_mismatch"),
            Self::ScopeMismatch => Some("scope_mismatch"),
            Self::CardOutsideItems => Some("card_outside_items"),
            Self::RequeueStateInvalid => Some("requeue_state_invalid"),
            Self::DuplicateCommand => Some("duplicate_command"),
            Self::MissingPostDigest | Self::TextTooLarge | Self::KeyTooLarge => None,
        }
    }
}

impl std::fmt::Display for CosDigestViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::KeyMismatch => "digest idempotency key does not match the run",
            Self::ScopeMismatch => "digest command names another thread or run",
            Self::CardOutsideItems => "digest card names no item of the run",
            Self::RequeueStateInvalid => "requeue for a run that was not stopped or interrupted",
            Self::DuplicateCommand => "more than one digest command of a kind",
            Self::MissingPostDigest => "no post_digest command",
            Self::TextTooLarge => "digest text is too large",
            Self::KeyTooLarge => "digest idempotency key is too large",
        };
        f.write_str(text)
    }
}

impl std::error::Error for CosDigestViolation {}

/// The run states whose `running` items go back to the queue.
fn requeues(run_state: &str) -> bool {
    matches!(run_state, "stopped" | "interrupted")
}

/// Japanese name of a `cos_inbox_items.source_kind`.
pub fn cos_kind_label(source_kind: &str) -> &'static str {
    match source_kind {
        "decision" => "決定",
        "question" => "質問",
        "authorization" => "認可",
        "acceptance_check" => "受け入れ検査",
        "plan_gate" => "計画の承認",
        "phase_gate" => "段の承認",
        "notice" => "知らせ",
        _ => "項目",
    }
}

/// The card kind of a `cos_inbox_items.source_kind` in CoS triage messages (`notice` is the
/// notice source kind of triage intake).
pub fn cos_card_kind(source_kind: &str) -> ChatCardKind {
    match source_kind {
        "decision" => ChatCardKind::Decision,
        "question" => ChatCardKind::Question,
        "authorization" | "acceptance_check" => ChatCardKind::Approval,
        "plan_gate" | "phase_gate" => ChatCardKind::PlanGate,
        "notice" => ChatCardKind::Notice,
        _ => ChatCardKind::Task,
    }
}

/// The judgment line of one item, and whether the person still has to act on it.
fn judgment(item: &CosTriageItemReport, run_state: &str) -> &'static str {
    match item.state.as_str() {
        "answered" => "代わりに答えた",
        "observed" => "見ただけ（判断は不要）",
        "escalated" => "人に回した",
        "fallback" => "未処理のまま人へ直接通知した（CoS 不在の退避）",
        "resolved" => "元の待ちが閉じていた（何もしていない）",
        "pending" => "中断されたので次の run で扱う",
        "running" if requeues(run_state) => "中断されたので次の run で扱う",
        "running" => "未処理（人へ直接通知する）",
        _ => "状態不明",
    }
}

/// The options of a routed item. With CoS's text for each option (ADR
/// 2026-10-28-cos-inbox-chat-summary D3) every option gets its own line; labels alone stay on one.
fn options_line(item: &CosTriageItemReport) -> Option<String> {
    let d = item.decision.as_ref()?;
    if d.options.is_empty() {
        return None;
    }
    let label = |o: &CosTriageOption| {
        if d.recommended.as_deref() == Some(o.key.as_str()) {
            format!("{}（推奨）", o.label)
        } else {
            o.label.clone()
        }
    };
    if d.options.iter().any(|o| o.detail.is_some()) {
        let lines: Vec<String> = d
            .options
            .iter()
            .map(|o| match &o.detail {
                Some(detail) => format!("\n  - {}: {detail}", label(o)),
                None => format!("\n  - {}", label(o)),
            })
            .collect();
        return Some(lines.concat());
    }
    let labels: Vec<String> = d.options.iter().map(label).collect();
    Some(labels.join(" / "))
}

/// The digest text and cards of one run. Pure.
pub fn cos_digest_message(
    run_id: &str,
    run_state: &str,
    reason: Option<&str>,
    items: &[CosTriageItemReport],
) -> (String, Vec<ChatCard>) {
    let mut text = format!("受信箱の一次対応の結果（{} 件", items.len());
    match run_state {
        "completed" => text.push(')'),
        "failed" => {
            text.push_str("。run は失敗");
            if let Some(reason) = reason.filter(|r| !r.trim().is_empty()) {
                text.push_str(&format!(": {reason}"));
            }
            text.push(')');
        }
        other => text.push_str(&format!("。run は {other} で終わった)")),
    }
    text.push('\n');
    let mut cards = Vec::new();
    for item in items {
        let title = item.title();
        text.push_str(&format!(
            "\n### {}（{}）\n- 判断: {}\n",
            title,
            cos_kind_label(&item.source_kind),
            judgment(item, run_state)
        ));
        text.push_str(&format!(
            "- 理由: {}\n",
            item.reason
                .as_deref()
                .map(str::trim)
                .filter(|r| !r.is_empty())
                .unwrap_or("理由の記録なし")
        ));
        let mut state = item.state.clone();
        let mut href = "/inbox".to_owned();
        let mut card_reason = item.reason.clone();
        if matches!(item.state.as_str(), "escalated" | "fallback") {
            if let Some(d) = &item.decision {
                if let Some(background) = d.background.as_deref().filter(|b| !b.is_empty()) {
                    text.push_str(&format!("- 背景: {background}\n"));
                }
                if !d.summary.is_empty() {
                    text.push_str(&format!("- 人が決めること: {}\n", d.summary));
                }
                if let Some(options) = options_line(item) {
                    text.push_str(&format!("- 選択肢: {options}\n"));
                }
                if let Some(why) = d.recommendation_reason.as_deref().filter(|r| !r.is_empty()) {
                    text.push_str(&format!("- 推奨の理由: {why}\n"));
                    card_reason = Some(why.to_owned());
                }
                if let Some(next) = d.on_answer.as_deref().filter(|n| !n.is_empty()) {
                    text.push_str(&format!("- 答えると: {next}\n"));
                }
                if !d.web_path.is_empty() {
                    href = d.web_path.clone();
                }
            }
            if item.state == "fallback" {
                // The original wait is still open: the card stays answerable.
                state = "pending".into();
            }
        } else if item.state == "running" && requeues(run_state) {
            state = "pending".into();
        }
        cards.push(ChatCard {
            kind: cos_card_kind(&item.source_kind),
            id: item.source_key.clone(),
            title,
            state,
            href,
            actor: ChatActor::Cos,
            reason: card_reason,
            operation_id: item.operation_id.clone(),
        });
    }
    text.push_str(&format!("\n(run {run_id})\n"));
    (text, cards)
}

/// The Rust reference decision: always one `post_digest`; for a stopped or interrupted run a
/// `requeue_run` after it (the message is written first, as before the split).
pub fn cos_digest_decide(input: &CosDigestInput) -> Vec<CosDigestCommand> {
    let (text, cards) = cos_digest_message(
        &input.run_id,
        &input.run_state,
        input.reason.as_deref(),
        &input.items,
    );
    let mut commands = vec![CosDigestCommand::PostDigest {
        thread: input.thread.clone(),
        idempotency_key: format!("{COS_TRIAGE_DIGEST_KEY_PREFIX}{}", input.run_id),
        text,
        cards,
        run_id: input.run_id.clone(),
    }];
    if requeues(&input.run_state) {
        commands.push(CosDigestCommand::RequeueRun {
            run_id: input.run_id.clone(),
        });
    }
    commands
}

/// S4-3: check a command list against its snapshot before it is applied. Any violation rejects
/// the whole list.
pub fn validate_cos_digest_commands(
    input: &CosDigestInput,
    commands: &[CosDigestCommand],
) -> Result<(), CosDigestViolation> {
    let expected_key = format!("{COS_TRIAGE_DIGEST_KEY_PREFIX}{}", input.run_id);
    let source_keys: HashSet<&str> = input.items.iter().map(|i| i.source_key.as_str()).collect();
    let mut posts = 0usize;
    let mut requeue_runs = 0usize;
    for command in commands {
        match command {
            CosDigestCommand::PostDigest {
                thread,
                idempotency_key,
                text,
                cards,
                run_id,
            } => {
                posts += 1;
                if idempotency_key.len() > CHAT_CLIENT_KEY_MAX_BYTES {
                    return Err(CosDigestViolation::KeyTooLarge);
                }
                if *idempotency_key != expected_key {
                    return Err(CosDigestViolation::KeyMismatch);
                }
                if *thread != input.thread || *run_id != input.run_id {
                    return Err(CosDigestViolation::ScopeMismatch);
                }
                if cards.len() > input.items.len()
                    || cards.iter().any(|c| !source_keys.contains(c.id.as_str()))
                {
                    return Err(CosDigestViolation::CardOutsideItems);
                }
                if text.len() > CHAT_MESSAGE_TEXT_MAX_BYTES {
                    return Err(CosDigestViolation::TextTooLarge);
                }
            }
            CosDigestCommand::RequeueRun { run_id } => {
                requeue_runs += 1;
                if *run_id != input.run_id {
                    return Err(CosDigestViolation::ScopeMismatch);
                }
                if !requeues(&input.run_state) {
                    return Err(CosDigestViolation::RequeueStateInvalid);
                }
            }
        }
    }
    if posts > 1 || requeue_runs > 1 {
        return Err(CosDigestViolation::DuplicateCommand);
    }
    if posts == 0 {
        return Err(CosDigestViolation::MissingPostDigest);
    }
    Ok(())
}
