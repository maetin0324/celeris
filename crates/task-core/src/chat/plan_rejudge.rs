//! ADR 2026-10-30-cos-review-open-waits D4: a plan approval that CoS escalated because the task
//! still had an unanswered decision (skill cos-inbox-triage §3 (f)) goes back to CoS once the
//! person has answered the task's decisions. Same state as the relay re-judge (付記 A2): the item's
//! input message. The claim moves it past the answer, so one answer hands an item over once.
use rusqlite::params;
use time::OffsetDateTime;

use super::store::{ChatError, chat_ts, immediate, writer};
use super::triage::{COS_TRIAGE_CLAIM_PREFIX, CosTriageClaim, bind_claim_tx};
use crate::store::SqliteStore;

/// The second line of a re-judging run's input message.
pub const COS_PLAN_REJUDGE_PREFIX: &str = "決定の回答による計画の再判断: ";

/// The condition shared by the candidate query and the claim: an escalated plan gate item judged
/// by a triage run, whose task has a decision answered after that judgment and no open decision.
const PLAN_REJUDGE_WHERE: &str = "i.state='escalated' AND i.source_key LIKE 'plan_gate-%' \
     AND i.message_id!='' AND j.role='system' \
     AND EXISTS(SELECT 1 FROM decisions d WHERE i.source_key LIKE 'plan_gate-' || d.task_id || '-v%' \
       AND d.status='answered' AND d.answered_at IS NOT NULL \
       AND julianday(d.answered_at)>julianday(j.created_at)) \
     AND NOT EXISTS(SELECT 1 FROM decisions d WHERE i.source_key LIKE 'plan_gate-' || d.task_id || '-v%' \
       AND d.status='open')";

/// One escalated plan gate item to hand to CoS again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanRejudgeCandidate {
    pub item_id: String,
    pub thread_id: String,
    pub source_key: String,
}

impl SqliteStore {
    /// D4: escalated plan gate items whose task's decisions were all answered after the judgment.
    pub fn cos_triage_plan_rejudge_candidates(
        &self,
    ) -> Result<Vec<PlanRejudgeCandidate>, ChatError> {
        let conn = writer(self)?;
        let mut stmt = conn.prepare(&format!(
            "SELECT i.id,i.thread_id,i.source_key FROM cos_inbox_items i \
             JOIN chat_messages j ON j.id=i.message_id AND j.thread_id=i.thread_id \
             WHERE {PLAN_REJUDGE_WHERE} ORDER BY i.created_at,i.id"
        ))?;
        let rows = stmt.query_map([], |r| {
            Ok(PlanRejudgeCandidate {
                item_id: r.get(0)?,
                thread_id: r.get(1)?,
                source_key: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// D4: bind the still-qualifying items of `item_ids` in `thread_id` to one system message and
    /// one chat run, as a triage claim does. `None` while a run of the thread is live or when no
    /// item qualifies any more.
    pub fn cos_triage_plan_rejudge_claim(
        &self,
        thread_id: &str,
        run_id: &str,
        item_ids: &[String],
        now: OffsetDateTime,
    ) -> Result<Option<CosTriageClaim>, ChatError> {
        let at = chat_ts(now);
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let active: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_runs WHERE thread_id=?1 AND state IN ('running','stopping'))",
            [thread_id],
            |r| r.get(0),
        )?;
        if active {
            return Ok(None);
        }
        let mut ids = Vec::new();
        for id in item_ids {
            let ok: bool = tx.query_row(
                &format!(
                    "SELECT EXISTS(SELECT 1 FROM cos_inbox_items i \
                     JOIN chat_messages j ON j.id=i.message_id AND j.thread_id=i.thread_id \
                     WHERE i.id=?1 AND i.thread_id=?2 AND {PLAN_REJUDGE_WHERE})"
                ),
                params![id, thread_id],
                |r| r.get(0),
            )?;
            if ok {
                ids.push(id.clone());
            }
        }
        if ids.is_empty() {
            return Ok(None);
        }
        let list = serde_json::to_string(&ids)?;
        let body = format!("{COS_TRIAGE_CLAIM_PREFIX}{list}\n{COS_PLAN_REJUDGE_PREFIX}{list}");
        let message_id = bind_claim_tx(&tx, thread_id, run_id, &ids, &body, &at)?;
        tx.commit()?;
        Ok(Some(CosTriageClaim {
            thread_id: thread_id.to_owned(),
            message_id,
            run_id: run_id.to_owned(),
            item_ids: ids,
        }))
    }
}
