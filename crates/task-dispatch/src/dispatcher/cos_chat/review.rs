//! ADR 2026-10-27-cron-cos-review: a `cos_review` cron job posts one input per scheduled time.
//! ADR 2026-10-28-cos-review-back-thread D2: the input goes to the general CoS's back thread
//! (kind `review`, hidden from the person's lists), never to the general thread. The input lists
//! what changed since the previous review (project briefs, ADR 2026-10-27-project-brief) and the
//! open stagnation findings (ADR 2026-10-27-project-stagnation-inbox). Store reads and one queued
//! message only: the CoS run is started later by the normal queue claim, the dispatcher never
//! calls an LLM here. The run reports to the person with `chat.review_report` only when there is
//! something to tell (D4'/D5').

use task_core::chat::{COS_REVIEW_INPUT_KEY_PREFIX, ChatMessageRole, ChatMessageState, ChatThread};
use task_core::{CronJob, CronJobId, SqliteStore, TaskStore};
use task_ops::cron_jobs::{CronInputPost, CronInputSink};
use task_ops::stagnation::StagnationFinding;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::launch::CosChatLaunch;

/// Items of each list an input carries (the rest is counted, CoS reads details on demand).
pub(crate) const REVIEW_LIST_MAX: usize = 20;
/// Inbox items one review input reads (open, and closed since the previous review).
const REVIEW_WAITS_MAX: usize = 100;
/// Characters of a changed brief's `state` an input keeps.
const REVIEW_STATE_CHARS: usize = 200;

/// The instruction used when the job's `template.objective` is empty.
/// ADR 2026-10-28-cos-review-back-thread D5': report to the person only when there is something to
/// tell, through `chat.review_report`; otherwise one line in this (review) thread.
pub(crate) const COS_REVIEW_INSTRUCTION: &str = "定期の見直しです。この thread は統括 CoS の裏 thread で、人は読みません。\
下は DB から決定的に集めた、前回の見直しから変わった案件の要約、いま出ている停滞、いま開いている人待ち、前回から閉じた人待ちの一覧です。\
報告に書く人待ちは「いま開いている人待ち」にあるものか、報告直前に API で開いていると確かめたものだけにし、\
前の session の記憶で状態を書かないでください。閉じた人待ちと前回までに報告した件は「増えた」と書きません。案件をまたいで、\
人の操作待ちの長さ・停滞・優先・資源を確かめ、手を打つ必要がある案件だけ GET /api/v1/projects/{id} で詳しく読んでください。\
人に伝えるべきこと（新しい停滞、人の判断が要る件、案件の要約の重要な変化）があるときだけ、chat.review_report \
（POST /api/v1/cos/review-report、review_message_id はこの入力の id）で統括 CoS チャットに 1 件報告してください。\
報告は人が数十秒で読める分量にし、案件名・task id・人がする事を書きます。伝えることが無ければ報告せず、\
この thread に「変化なし」と 1 行だけ書いて終えてください。";

/// The input's `client_message_id`: one per job and scheduled time, so a retry after a restart
/// finds the same input instead of posting a second one.
pub(crate) fn review_key(job: CronJobId, scheduled_for: OffsetDateTime) -> String {
    format!("cron-cos-review:{job}:{}", scheduled_for.unix_timestamp())
}

/// [`CronInputSink`] for the daemon tick. `launch = None` (CoS chat not installed) fails the fire.
pub(crate) struct CosReviewSink<'a> {
    pub launch: Option<&'a CosChatLaunch>,
    pub findings: &'a [StagnationFinding],
    pub now: OffsetDateTime,
}

impl CronInputSink for CosReviewSink<'_> {
    fn post_cos_review(
        &mut self,
        job: &CronJob,
        scheduled_for: OffsetDateTime,
        previous: Option<OffsetDateTime>,
    ) -> Result<CronInputPost, String> {
        let Some(launch) = self.launch else {
            return Err("CoS chat is not configured in this daemon".into());
        };
        if !launch.config.enabled {
            return Err("CoS chat is disabled".into());
        }
        if let Some(reason) = &launch.config.unavailable_reason {
            return Err(format!("CoS chat is unavailable: {reason}"));
        }
        let store = launch.store.as_ref();
        let thread = store
            .cos_review_thread_open(self.now)
            .map_err(|e| e.to_string())?
            .thread;
        let key = review_key(job.id, scheduled_for);
        if let Some(existing) = store
            .chat_message_by_client_id(&thread.id, &key)
            .map_err(|e| e.to_string())?
        {
            return Ok(CronInputPost::Posted(format!(
                "input {} already in the review thread {}",
                existing.id, thread.id
            )));
        }
        let mut previous = previous;
        let mut note = String::new();
        if thread.queue_paused {
            // A person stopped a review run (`…/stop` pauses the queue) and nobody may resume the
            // review thread's queue. The waiting inputs are dropped and this input, which resumes
            // the queue, covers their period too, so the review does not skip forever.
            let dropped = drop_queued_inputs(store, &thread, self.now)?;
            if dropped > 0 {
                previous = last_reviewed(store, &thread, job.id)?;
                note = format!(
                    " (the queue was paused: {dropped} waiting input(s) cancelled, period from {})",
                    previous.map_or_else(|| "the first review".to_string(), rfc3339)
                );
            } else {
                note = " (the paused queue was resumed)".into();
            }
        } else if thread.queued_count > 0 {
            return Ok(CronInputPost::Busy(format!(
                "the review thread {} still has {} queued input(s)",
                thread.id, thread.queued_count
            )));
        }
        let text = review_text(store, job, scheduled_for, previous, self.findings)?;
        let posted = store
            .cos_review_input_post(&key, &text, self.now)
            .map_err(|e| e.to_string())?;
        Ok(CronInputPost::Posted(format!(
            "posted input {} to the review thread {}{note}",
            posted.response.message.id, thread.id
        )))
    }
}

/// Cancels the queued inputs of the (paused) review thread. Only cron inputs can be there.
fn drop_queued_inputs(
    store: &SqliteStore,
    thread: &ChatThread,
    now: OffsetDateTime,
) -> Result<usize, String> {
    if thread.queued_count == 0 {
        return Ok(0);
    }
    let mut dropped = 0;
    for message in queued_page(store, thread)? {
        if message.role == ChatMessageRole::User && message.state == ChatMessageState::Queued {
            store
                .chat_message_cancel(&thread.id, &message.id, now)
                .map_err(|e| e.to_string())?;
            dropped += 1;
        }
    }
    Ok(dropped)
}

fn queued_page(
    store: &SqliteStore,
    thread: &ChatThread,
) -> Result<Vec<task_core::chat::ChatMessage>, String> {
    Ok(store
        .chat_message_list(&thread.id, &Default::default())
        .map_err(|e| e.to_string())?
        .items)
}

/// The scheduled time of this job's latest input a run took (not cancelled, not waiting): the
/// start of the period an input replacing cancelled ones covers. `None` (every brief) when the
/// recent messages hold none.
fn last_reviewed(
    store: &SqliteStore,
    thread: &ChatThread,
    job: CronJobId,
) -> Result<Option<OffsetDateTime>, String> {
    let prefix = format!("{COS_REVIEW_INPUT_KEY_PREFIX}{job}:");
    Ok(queued_page(store, thread)?
        .into_iter()
        .rev()
        .filter(|m| {
            m.role == ChatMessageRole::User
                && !matches!(
                    m.state,
                    ChatMessageState::Queued | ChatMessageState::Cancelled
                )
        })
        .find_map(|m| {
            let secs = m.client_message_id.as_deref()?.strip_prefix(&prefix)?;
            OffsetDateTime::from_unix_timestamp(secs.parse().ok()?).ok()
        }))
}

fn rfc3339(at: OffsetDateTime) -> String {
    at.format(&Rfc3339).unwrap_or_default()
}

fn cut(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut out: String = flat.chars().take(max).collect();
    out.push('…');
    out
}

/// The input text: instruction, then the briefs updated after `previous` (all briefs on the first
/// review) and the stagnation findings. Deterministic for the same store and findings.
pub(crate) fn review_text(
    store: &SqliteStore,
    job: &CronJob,
    scheduled_for: OffsetDateTime,
    previous: Option<OffsetDateTime>,
    findings: &[StagnationFinding],
) -> Result<String, String> {
    let instruction = match job.template.objective.trim() {
        "" => COS_REVIEW_INSTRUCTION,
        text => text,
    };
    let mut briefs = store.project_brief_list().map_err(|e| e.to_string())?;
    let mut changed: Vec<String> = Vec::new();
    for project in store.project_list().map_err(|e| e.to_string())? {
        if project.archived_at.is_some() {
            continue;
        }
        let Some(brief) = briefs.remove(&project.id) else {
            continue;
        };
        if previous.is_some_and(|p| brief.updated_at <= p) {
            continue;
        }
        let waits: Vec<String> = brief
            .waiting_on
            .iter()
            .map(|w| format!("{}: {}", w.kind.as_str(), cut(&w.note, 80)))
            .collect();
        let waits = if waits.is_empty() {
            String::new()
        } else {
            format!(" 待ち [{}]", waits.join(" / "))
        };
        changed.push(format!(
            "- {} ({}) 更新 {} by {}。現状 {}{waits}",
            project.title,
            project.id,
            rfc3339(brief.updated_at),
            brief.updated_by,
            cut(&brief.state, REVIEW_STATE_CHARS),
        ));
    }
    let mut out = format!(
        "[{}] {instruction}\n\n予定時刻 {}、前回の見直し {}\n",
        job.template.title.trim(),
        rfc3339(scheduled_for),
        previous.map_or_else(|| "なし".to_string(), rfc3339),
    );
    push_list(&mut out, "要約が変わった案件", &changed);
    let stalls: Vec<String> = findings
        .iter()
        .map(|f| {
            format!(
                "- {} {} (since {}) {}",
                f.kind.as_str(),
                f.subject,
                f.since,
                cut(&f.detail, REVIEW_STATE_CHARS)
            )
        })
        .collect();
    push_list(&mut out, "停滞", &stalls);
    // ADR 2026-10-30-cos-review-open-waits D1: the waits on the person open at this review, read
    // from the inbox items, and the ones that closed since the previous review.
    let open: Vec<String> = store
        .cos_triage_open_items(None, REVIEW_WAITS_MAX)
        .map_err(|e| e.to_string())?
        .iter()
        .map(wait_line)
        .collect();
    push_list(&mut out, "いま開いている人待ち", &open);
    if let Some(since) = previous {
        let closed: Vec<String> = store
            .cos_triage_closed_since(since, REVIEW_WAITS_MAX)
            .map_err(|e| e.to_string())?
            .iter()
            .map(wait_line)
            .collect();
        push_list(&mut out, "前回から閉じた人待ち（報告しない）", &closed);
    }
    Ok(out)
}

/// One wait: kind, source (the task id for task items), state, opened time and title.
fn wait_line(item: &task_core::chat::triage::CosTriageItemReport) -> String {
    format!(
        "- {} {} [{}] 開いた {} {}",
        item.source_kind,
        item.source_key,
        item.state,
        item.created_at,
        cut(&item.title(), 80)
    )
}

fn push_list(out: &mut String, heading: &str, items: &[String]) {
    out.push_str(&format!("\n{heading}（{} 件）\n", items.len()));
    if items.is_empty() {
        out.push_str("- なし\n");
        return;
    }
    for item in items.iter().take(REVIEW_LIST_MAX) {
        out.push_str(item);
        out.push('\n');
    }
    if items.len() > REVIEW_LIST_MAX {
        out.push_str(&format!("- ほか {} 件\n", items.len() - REVIEW_LIST_MAX));
    }
}
