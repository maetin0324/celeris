//! ADR 2026-10-07-cos-inbox-thread-conversation D1/D4: the deterministic digest of one triage
//! run, written as an assistant message of the inbox thread after the run ends.
//!
//! No LLM and no judgment: the text is assembled from `cos_inbox_items` (state, reason,
//! summary) and the decision packet CoS put in the escalation outbox. Items a stopped or
//! interrupted run left `running` go back to `pending` (a human interrupt must not push them
//! to the unavailable fallback); items a completed run left `running` are reported as
//! unhandled and the fallback leaf takes them.

use task_core::chat::triage::{COS_TRIAGE_DIGEST_KEY_PREFIX, CosTriageItemReport};
use task_core::chat::{ChatActor, ChatCard};
use time::OffsetDateTime;

use super::launch::CosChatLaunch;
use super::triage::card_kind;

/// Runs digested per pass (bounded work per tick).
const DIGEST_RUNS_PER_PASS: usize = 20;

/// Japanese name of a `cos_inbox_items.source_kind`.
pub(crate) fn kind_label(source_kind: &str) -> &'static str {
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

/// The judgment line of one item, and whether the person still has to act on it.
fn judgment(item: &CosTriageItemReport, run_state: &str) -> &'static str {
    match item.state.as_str() {
        "answered" => "代わりに答えた",
        "observed" => "見ただけ（判断は不要）",
        "escalated" => "人に回した",
        "fallback" => "未処理のまま人へ直接通知した（CoS 不在の退避）",
        "resolved" => "元の待ちが閉じていた（何もしていない）",
        "pending" => "中断されたので次の run で扱う",
        "running" if matches!(run_state, "stopped" | "interrupted") => {
            "中断されたので次の run で扱う"
        }
        "running" => "未処理（人へ直接通知する）",
        _ => "状態不明",
    }
}

fn options_line(item: &CosTriageItemReport) -> Option<String> {
    let d = item.decision.as_ref()?;
    if d.options.is_empty() {
        return None;
    }
    let labels: Vec<String> = d
        .options
        .iter()
        .map(|o| {
            if d.recommended.as_deref() == Some(o.key.as_str()) {
                format!("{}（推奨）", o.label)
            } else {
                o.label.clone()
            }
        })
        .collect();
    Some(labels.join(" / "))
}

/// The digest text and cards of one run. Pure.
pub(crate) fn digest_message(
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
            kind_label(&item.source_kind),
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
                if !d.web_path.is_empty() {
                    href = d.web_path.clone();
                }
            }
            if item.state == "fallback" {
                // The original wait is still open: the card stays answerable.
                state = "pending".into();
            }
        } else if item.state == "running" && matches!(run_state, "stopped" | "interrupted") {
            state = "pending".into();
        }
        cards.push(ChatCard {
            kind: card_kind(&item.source_kind),
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

impl CosChatLaunch {
    /// One digest pass: every finished triage run without a digest gets its message; stopped
    /// and interrupted runs give their unhandled items back to the queue.
    pub(crate) fn triage_digest(&mut self, now: OffsetDateTime) {
        // Keep the reservation until an empty scan succeeds. A bounded pass or any
        // lookup/write failure must continue on the next tick without another run.
        self.triage.digest_due = true;
        let runs = match self
            .store
            .cos_triage_runs_without_digest(DIGEST_RUNS_PER_PASS)
        {
            Ok(runs) => runs,
            Err(error) => {
                tracing::warn!(%error, "CoS triage digest scan failed");
                return;
            }
        };
        if runs.is_empty() {
            self.triage.digest_due = false;
            return;
        }
        let thread = match self.inbox_thread() {
            Ok(Some(thread)) => thread,
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(%error, "CoS triage digest inbox thread lookup failed");
                return;
            }
        };
        for (run_id, run_state) in runs {
            if let Err(error) = self.digest_one(&thread, &run_id, &run_state, now) {
                tracing::warn!(%error, %run_id, "CoS triage digest failed");
            }
        }
    }

    fn digest_one(
        &mut self,
        thread: &str,
        run_id: &str,
        run_state: &str,
        now: OffsetDateTime,
    ) -> Result<(), String> {
        let items = self
            .store
            .cos_triage_run_report(run_id)
            .map_err(|e| e.to_string())?;
        let reason = self
            .store
            .chat_run_get(thread, run_id)
            .ok()
            .and_then(|run| run.reason);
        let (text, cards) = digest_message(run_id, run_state, reason.as_deref(), &items);
        self.store
            .chat_assistant_message_add_once(
                thread,
                &format!("{COS_TRIAGE_DIGEST_KEY_PREFIX}{run_id}"),
                &text,
                &cards,
                Some(run_id),
                now,
            )
            .map_err(|e| e.to_string())?;
        if matches!(run_state, "stopped" | "interrupted") {
            let requeued = self
                .store
                .cos_triage_requeue_run(run_id, now)
                .map_err(|e| e.to_string())?;
            if requeued > 0 {
                self.triage.dirty = true;
            }
        }
        Ok(())
    }
}
