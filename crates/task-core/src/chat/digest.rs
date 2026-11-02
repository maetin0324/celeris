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

// ---------------------------------------------------------------------------------------------
// S4-12/S4-14 (ADR 2026-11-02-ts-control-layer-boundary): the digest pass as one async TS call.
// The op DTOs (`cos_digest:list_runs`, `cos_digest:read_run`) and the pass output, and the pass
// rules 7–9 on top of the per-run rules 1–6.
// ---------------------------------------------------------------------------------------------

/// `cos_digest:list_runs` request (no fields; the bound of 20 is Rust's constant).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CosDigestListRunsRequest {}

/// One run waiting for its digest (`cos_triage_runs_without_digest`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CosDigestRunRef {
    pub thread: String,
    pub run_id: String,
    pub run_state: String,
}

/// `cos_digest:list_runs` reply, in scan order (`started_at`, `run_id`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CosDigestListRunsReply {
    pub runs: Vec<CosDigestRunRef>,
}

/// `cos_digest:read_run` request; the reply is a [`CosDigestInput`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CosDigestReadRunRequest {
    pub thread: String,
    pub run_id: String,
}

/// The commands of one run in a pass output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CosDigestPassRun {
    pub run_id: String,
    pub commands: Vec<CosDigestCommand>,
}

/// The output of `cosDigestPass(ctx)` (and of the Rust reference pass).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CosDigestPassOutput {
    pub runs: Vec<CosDigestPassRun>,
}

/// One listed run and what its read gave (`None`: `read_run` failed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CosDigestPassRead {
    pub run: CosDigestRunRef,
    pub input: Option<CosDigestInput>,
}

/// Why a pass output must not be applied (S4-14). Any violation rejects the whole pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CosDigestPassViolation {
    /// Rules 1–6 on the commands of one run.
    Run {
        run_id: String,
        violation: CosDigestViolation,
    },
    /// Rule 7: a run id that is not in the pass's `list_runs` answer.
    RunOutsideList { run_id: String },
    /// Rule 8: a readable run is absent from the output or appears more than once.
    RunMissing { run_id: String },
    /// Rule 8: a run whose `read_run` failed appears in the output.
    RunUnreadable { run_id: String },
    /// Rule 9: the runs are not in the order of the `list_runs` answer.
    RunOrder,
}

impl CosDigestPassViolation {
    /// The `fallback.code` of the log line (S4-15).
    pub fn fallback_code(&self) -> &'static str {
        match self {
            Self::Run { .. } => "invalid_output",
            Self::RunOutsideList { .. } => "run_outside_list",
            Self::RunMissing { .. } => "run_missing",
            Self::RunUnreadable { .. } => "run_unreadable",
            Self::RunOrder => "run_order",
        }
    }

    /// The `safety_violations` code (`None` for format violations).
    pub fn safety_code(&self) -> Option<&'static str> {
        match self {
            Self::Run { violation, .. } => violation.code(),
            Self::RunOutsideList { .. } => Some("run_outside_list"),
            Self::RunMissing { .. } | Self::RunUnreadable { .. } | Self::RunOrder => None,
        }
    }

    /// The run the violation names (`None` for the order rule).
    pub fn run_id(&self) -> Option<&str> {
        match self {
            Self::Run { run_id, .. }
            | Self::RunOutsideList { run_id }
            | Self::RunMissing { run_id }
            | Self::RunUnreadable { run_id } => Some(run_id),
            Self::RunOrder => None,
        }
    }
}

impl std::fmt::Display for CosDigestPassViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Run { run_id, violation } => write!(f, "run {run_id}: {violation}"),
            Self::RunOutsideList { run_id } => write!(f, "run {run_id} is not in list_runs"),
            Self::RunMissing { run_id } => {
                write!(f, "readable run {run_id} is not in the output exactly once")
            }
            Self::RunUnreadable { run_id } => {
                write!(f, "run {run_id} could not be read but is in the output")
            }
            Self::RunOrder => f.write_str("runs are not in list_runs order"),
        }
    }
}

impl std::error::Error for CosDigestPassViolation {}

/// The Rust reference pass: every readable run in list order, decided by [`cos_digest_decide`].
pub fn cos_digest_pass_decide(reads: &[CosDigestPassRead]) -> CosDigestPassOutput {
    CosDigestPassOutput {
        runs: reads
            .iter()
            .filter_map(|r| r.input.as_ref())
            .map(|input| CosDigestPassRun {
                run_id: input.run_id.clone(),
                commands: cos_digest_decide(input),
            })
            .collect(),
    }
}

/// S4-14: rules 7–9 on the pass, then rules 1–6 on each run against its read snapshot.
/// `reads` is the `list_runs` answer in order, each with its read (every listed run is read).
pub fn validate_cos_digest_pass(
    reads: &[CosDigestPassRead],
    output: &CosDigestPassOutput,
) -> Result<(), CosDigestPassViolation> {
    for run in &output.runs {
        if !reads.iter().any(|r| r.run.run_id == run.run_id) {
            return Err(CosDigestPassViolation::RunOutsideList {
                run_id: run.run_id.clone(),
            });
        }
    }
    for read in reads {
        let n = output
            .runs
            .iter()
            .filter(|r| r.run_id == read.run.run_id)
            .count();
        match (&read.input, n) {
            (Some(_), 1) | (None, 0) => {}
            (Some(_), _) => {
                return Err(CosDigestPassViolation::RunMissing {
                    run_id: read.run.run_id.clone(),
                });
            }
            (None, _) => {
                return Err(CosDigestPassViolation::RunUnreadable {
                    run_id: read.run.run_id.clone(),
                });
            }
        }
    }
    let expected = reads
        .iter()
        .filter(|r| r.input.is_some())
        .map(|r| r.run.run_id.as_str());
    if !expected.eq(output.runs.iter().map(|r| r.run_id.as_str())) {
        return Err(CosDigestPassViolation::RunOrder);
    }
    for run in &output.runs {
        let input = reads
            .iter()
            .find(|r| r.run.run_id == run.run_id)
            .and_then(|r| r.input.as_ref());
        if let Some(input) = input {
            validate_cos_digest_commands(input, &run.commands).map_err(|violation| {
                CosDigestPassViolation::Run {
                    run_id: run.run_id.clone(),
                    violation,
                }
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod pass_tests {
    use super::*;

    fn read(run_id: &str, state: &str, readable: bool) -> CosDigestPassRead {
        let run = CosDigestRunRef {
            thread: "t".into(),
            run_id: run_id.into(),
            run_state: state.into(),
        };
        let input = readable.then(|| CosDigestInput {
            thread: "t".into(),
            run_id: run_id.into(),
            run_state: state.into(),
            reason: None,
            items: Vec::new(),
        });
        CosDigestPassRead { run, input }
    }

    #[test]
    fn cos_digest_pass_validation_rejects_each_rule() {
        let reads = vec![
            read("r1", "completed", true),
            read("r2", "interrupted", true),
            read("r3", "stopped", false),
        ];
        let good = cos_digest_pass_decide(&reads);
        assert_eq!(good.runs.len(), 2);
        assert_eq!(validate_cos_digest_pass(&reads, &good), Ok(()));
        let code = |out: &CosDigestPassOutput| {
            validate_cos_digest_pass(&reads, out)
                .err()
                .map(|v| (v.fallback_code(), v.safety_code()))
        };
        let with = |f: &dyn Fn(&mut CosDigestPassOutput)| {
            let mut out = good.clone();
            f(&mut out);
            out
        };
        fn post(out: &mut CosDigestPassOutput) -> &mut CosDigestCommand {
            &mut out.runs[0].commands[0]
        }
        // Rules 1–6 per run.
        let rule1 = with(&|o| {
            if let CosDigestCommand::PostDigest {
                idempotency_key, ..
            } = post(o)
            {
                *idempotency_key = "cos-triage-digest:other".into();
            }
        });
        assert_eq!(code(&rule1), Some(("invalid_output", Some("key_mismatch"))));
        let rule2 = with(&|o| {
            if let CosDigestCommand::PostDigest { thread, .. } = post(o) {
                *thread = "other".into();
            }
        });
        assert_eq!(
            code(&rule2),
            Some(("invalid_output", Some("scope_mismatch")))
        );
        let rule3 = with(&|o| {
            if let CosDigestCommand::PostDigest { cards, .. } = post(o) {
                cards.push(ChatCard {
                    kind: ChatCardKind::Question,
                    id: "nope".into(),
                    title: "x".into(),
                    state: "running".into(),
                    href: String::new(),
                    actor: ChatActor::Cos,
                    reason: None,
                    operation_id: None,
                });
            }
        });
        assert_eq!(
            code(&rule3),
            Some(("invalid_output", Some("card_outside_items")))
        );
        let rule4 = with(&|o| {
            o.runs[0].commands.push(CosDigestCommand::RequeueRun {
                run_id: "r1".into(),
            })
        });
        assert_eq!(
            code(&rule4),
            Some(("invalid_output", Some("requeue_state_invalid")))
        );
        let rule5 = with(&|o| {
            let c = o.runs[0].commands[0].clone();
            o.runs[0].commands.push(c);
        });
        assert_eq!(
            code(&rule5),
            Some(("invalid_output", Some("duplicate_command")))
        );
        let rule6 = with(&|o| o.runs[0].commands.clear());
        assert_eq!(code(&rule6), Some(("invalid_output", None)));
        // Rule 7.
        let rule7 = with(&|o| {
            let mut extra = o.runs[0].clone();
            extra.run_id = "r9".into();
            o.runs.push(extra);
        });
        assert_eq!(
            code(&rule7),
            Some(("run_outside_list", Some("run_outside_list")))
        );
        // Rule 8: a readable run missing, a readable run twice, an unreadable run present.
        let missing = with(&|o| {
            o.runs.remove(1);
        });
        assert_eq!(code(&missing), Some(("run_missing", None)));
        let twice = with(&|o| {
            let again = o.runs[0].clone();
            o.runs.push(again);
        });
        assert_eq!(code(&twice), Some(("run_missing", None)));
        let unreadable = with(&|o| {
            o.runs.push(CosDigestPassRun {
                run_id: "r3".into(),
                commands: Vec::new(),
            })
        });
        assert_eq!(code(&unreadable), Some(("run_unreadable", None)));
        // Rule 9.
        let order = with(&|o| o.runs.reverse());
        assert_eq!(code(&order), Some(("run_order", None)));
    }
}
