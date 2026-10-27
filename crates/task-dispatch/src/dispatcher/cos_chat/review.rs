//! ADR 2026-10-27-cron-cos-review: a `cos_review` cron job posts one input per scheduled time to
//! the general CoS thread. The input lists what changed since the previous review (project
//! briefs, ADR 2026-10-27-project-brief) and the open stagnation findings (ADR
//! 2026-10-27-project-stagnation-inbox). Store reads and one queued message only: the CoS run is
//! started later by the normal queue claim, the dispatcher never calls an LLM here.

use task_core::chat::{ChatOpenThreadRequest, ChatPostMessageRequest, ChatSendMode};
use task_core::{CronJob, CronJobId, SqliteStore, TaskStore};
use task_ops::cron_jobs::{CronInputPost, CronInputSink};
use task_ops::stagnation::StagnationFinding;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::launch::CosChatLaunch;

/// Items of each list an input carries (the rest is counted, CoS reads details on demand).
pub(crate) const REVIEW_LIST_MAX: usize = 20;
/// Characters of a changed brief's `state` an input keeps.
const REVIEW_STATE_CHARS: usize = 200;

/// The instruction used when the job's `template.objective` is empty.
pub(crate) const COS_REVIEW_INSTRUCTION: &str = "定期の見直しです。下は DB から決定的に集めた、前回の見直しから変わった案件の要約と、\
いま出ている停滞の一覧です。案件をまたいで、人の操作待ちの長さ・停滞・優先・資源を確かめ、手を打つ必要がある案件だけ \
GET /api/v1/projects/{id} で詳しく読んでください。変化が無く手を打つ必要も無ければ、そう 1 行だけ書いてください。";

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
            .cos_thread_open(&ChatOpenThreadRequest { project_id: None }, self.now)
            .map_err(|e| e.to_string())?
            .thread;
        let key = review_key(job.id, scheduled_for);
        if let Some(existing) = store
            .chat_message_by_client_id(&thread.id, &key)
            .map_err(|e| e.to_string())?
        {
            return Ok(CronInputPost::Posted(format!(
                "input {} already in the general CoS thread {}",
                existing.id, thread.id
            )));
        }
        if thread.queued_count > 0 {
            return Ok(CronInputPost::Busy(format!(
                "the general CoS thread {} still has {} queued input(s)",
                thread.id, thread.queued_count
            )));
        }
        let text = review_text(store, job, scheduled_for, previous, self.findings)?;
        let posted = store
            .chat_message_post(
                &thread.id,
                &ChatPostMessageRequest {
                    client_message_id: key,
                    text,
                    attachment_ids: Vec::new(),
                    reply_to_id: None,
                    mode: ChatSendMode::Queue,
                    resume_queue: false,
                },
                self.now,
            )
            .map_err(|e| e.to_string())?;
        Ok(CronInputPost::Posted(format!(
            "posted input {} to the general CoS thread {}",
            posted.response.message.id, thread.id
        )))
    }
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
    Ok(out)
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
