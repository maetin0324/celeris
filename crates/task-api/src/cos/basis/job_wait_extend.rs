//! ADR 2026-10-30-cos-job-wait-extend: the routine category (g) `job_wait_extend`.
//!
//! When the durable wait of cluster jobs reaches its limit (ADR-0090), the dispatcher asks for an
//! authorization (extend / cancel the jobs and go on / withdraw). Waiting longer for jobs that are
//! still queued uses no node-hours and changes no allocation, so CoS may answer "extend" alone. The
//! API checks what the store can decide: the authorization is the one the timeout raised, every job
//! of that wait was still queued at the last poll, the answer is `once` with the note
//! [`EXTEND_NOTE`], the basis names the approval, and the same jobs have not timed out more than
//! [`JOB_WAIT_EXTEND_MAX`] times.

use task_core::approval::{ApprovalId, ApprovalStore};
use task_core::cluster_job::{ClusterJobState, ClusterJobStatus, ClusterJobWaitState};
use task_core::store::SqliteStore;
use task_core::{Event, TaskStore};
use task_ops::human_inbox::InboxKind;

use super::{
    BASIS_EXTEND_LIMIT, BASIS_INVALID, BASIS_JOB_NOT_QUEUED, BasisRefKind, BasisTarget,
    DelegatedBasis, problem,
};
use crate::problem::{ApiProblem, store_problem};

/// The authorization option of an extension (`standing` would add a permanent rule).
pub(crate) const EXTEND_OPTION: &str = "once";
/// The only answer text of an extension: the next run waits for the same jobs again.
pub(crate) const EXTEND_NOTE: &str = "延長";
/// D3: how many wait limits of the same jobs CoS may answer by extending (the next one goes to a person).
pub(crate) const JOB_WAIT_EXTEND_MAX: usize = 3;

/// The timed-out wait an authorization was raised for.
#[derive(Debug, PartialEq, Eq)]
struct TimedOut {
    cluster: String,
    /// The declared jobs with their state at the last poll (`Unknown` when never seen).
    jobs: Vec<ClusterJobStatus>,
}

/// The wait whose timeout raised `question`: the last `QuestionRaised` with that text, preceded
/// (among the cluster wait events) by `ClusterJobWaitFinished{timed_out}`, and no wait after it.
fn timed_out_wait(events: &[(u64, Event)], question: &str) -> Option<TimedOut> {
    let asked = events
        .iter()
        .rposition(|(_, e)| matches!(e, Event::QuestionRaised { text, .. } if text == question))?;
    let waited_after = events[asked..].iter().any(|(_, e)| {
        matches!(
            e,
            Event::ClusterJobWaitStarted { .. }
                | Event::ClusterJobWaitPolled { .. }
                | Event::ClusterJobWaitFinished { .. }
        )
    });
    if waited_after {
        return None;
    }
    let (wait_id, last) = events[..asked].iter().rev().find_map(|(_, e)| match e {
        Event::ClusterJobWaitFinished {
            wait_id,
            state,
            jobs,
            ..
        } => Some((*state == ClusterJobWaitState::TimedOut).then_some((wait_id, jobs))),
        Event::ClusterJobWaitStarted { .. } | Event::ClusterJobWaitPolled { .. } => Some(None),
        _ => None,
    })??;
    let wait = started(events, wait_id)?;
    let jobs = wait
        .jobs
        .iter()
        .map(|j| {
            last.iter()
                .find(|s| &s.job_id == j)
                .cloned()
                .unwrap_or_else(|| unknown(j))
        })
        .collect();
    Some(TimedOut {
        cluster: wait.cluster.clone(),
        jobs,
    })
}

/// A declared job the polls never reported (`ClusterJobStatus::unknown` is private to task-core).
fn unknown(job_id: &str) -> ClusterJobStatus {
    ClusterJobStatus {
        job_id: job_id.to_owned(),
        state: ClusterJobState::Unknown,
        exit_status: None,
        raw_state: None,
    }
}

fn started<'a>(
    events: &'a [(u64, Event)],
    wait_id: &str,
) -> Option<&'a task_core::cluster_job::ClusterJobWait> {
    events.iter().find_map(|(_, e)| match e {
        Event::ClusterJobWaitStarted { wait } if wait.wait_id == wait_id => Some(wait.as_ref()),
        _ => None,
    })
}

/// How many waits of the task on the same cluster that share a job with `current` timed out
/// (the current one included).
fn timeouts_of_same_jobs(events: &[(u64, Event)], current: &TimedOut) -> usize {
    events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::ClusterJobWaitFinished {
                wait_id,
                state: ClusterJobWaitState::TimedOut,
                ..
            } => started(events, wait_id),
            _ => None,
        })
        .filter(|w| {
            w.cluster == current.cluster
                && w.jobs
                    .iter()
                    .any(|j| current.jobs.iter().any(|s| &s.job_id == j))
        })
        .count()
}

pub(super) fn validate(
    store: &SqliteStore,
    basis: &DelegatedBasis,
    target: &BasisTarget,
) -> Result<(), ApiProblem> {
    let invalid = |detail: String| problem(BASIS_INVALID, detail);
    if target.kind != Some(InboxKind::Authorization) {
        return Err(invalid(
            "job_wait_extend answers the authorization of a cluster job wait limit".into(),
        ));
    }
    // D2 (3): only the extension. `standing` adds a permanent rule, `denied` withdraws the task, and
    // cancelling the jobs or changing the allocation is written in the note: all go to a person.
    if target.option.as_deref() != Some(EXTEND_OPTION)
        || target.note.as_deref().map(str::trim) != Some(EXTEND_NOTE)
    {
        return Err(invalid(format!(
            "job_wait_extend answers {EXTEND_OPTION} with the note {EXTEND_NOTE:?} only; cancelling \
             the jobs, withdrawing, changing nodes or resources and standing go to a person"
        )));
    }
    let Some(approval_id) = target
        .approval_id
        .as_deref()
        .and_then(|id| id.parse::<ApprovalId>().ok())
    else {
        return Err(invalid(
            "job_wait_extend needs the approval it decides".into(),
        ));
    };
    let cited = basis
        .refs
        .iter()
        .any(|r| r.kind == BasisRefKind::Approval && r.id.trim() == approval_id.to_string());
    if !cited {
        return Err(invalid(format!(
            "job_wait_extend needs the ref {{\"kind\":\"approval\",\"id\":\"{approval_id}\"}}"
        )));
    }
    let approval = store
        .approval_get(approval_id)
        .map_err(store_problem)?
        .filter(|a| a.decision.is_none());
    let (Some(approval), Some(task_id)) = (approval, target.task_id) else {
        return Err(invalid(format!("approval {approval_id} is not pending")));
    };
    if approval.task_id != Some(task_id) {
        return Err(invalid(format!(
            "approval {approval_id} does not belong to task {task_id}"
        )));
    }
    let events = store.events_for(task_id).map_err(store_problem)?;
    // D2 (1): the authorization the wait limit raised.
    let Some(current) = timed_out_wait(&events, &approval.question) else {
        return Err(invalid(format!(
            "approval {approval_id} was not raised by a cluster job wait limit; escalate it"
        )));
    };
    // D2 (2): every job still queued, so the extension holds no node and changes no allocation.
    let not_queued: Vec<String> = current
        .jobs
        .iter()
        .filter(|s| s.state != ClusterJobState::Queued)
        .map(|s| format!("{} {:?}", s.job_id, s.state))
        .collect();
    if current.jobs.is_empty() || !not_queued.is_empty() {
        return Err(problem(
            BASIS_JOB_NOT_QUEUED,
            format!(
                "only jobs still queued are extended; not queued: {}; escalate it",
                not_queued.join(", ")
            ),
        ));
    }
    // D3: the limit on extensions of the same jobs.
    let reached = timeouts_of_same_jobs(&events, &current);
    if reached > JOB_WAIT_EXTEND_MAX {
        return Err(problem(
            BASIS_EXTEND_LIMIT,
            format!(
                "the wait of these jobs reached its limit {reached} times (CoS extends up to \
                 {JOB_WAIT_EXTEND_MAX}); escalate it"
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use task_core::cluster_job::{ClusterJobWait, ClusterScheduler};

    fn wait(id: &str, jobs: &[&str]) -> Event {
        Event::ClusterJobWaitStarted {
            wait: Box::new(ClusterJobWait {
                wait_id: id.into(),
                task_id: task_core::TaskId::new(),
                work_unit_id: None,
                run_id: "run-1".into(),
                cluster: "sirius".into(),
                scheduler: ClusterScheduler::Pbs,
                jobs: jobs.iter().map(|j| (*j).to_string()).collect(),
                poll_secs: 300,
                timeout_secs: 28800,
                summary: String::new(),
                checkpoint: None,
                created_at: "2026-10-29T07:12:00Z".into(),
                deadline: "2026-10-29T15:12:00Z".into(),
                last_polled_at: None,
                finished_at: None,
                state: ClusterJobWaitState::Waiting,
                last_status: Vec::new(),
            }),
        }
    }

    fn status(job: &str, state: ClusterJobState) -> ClusterJobStatus {
        ClusterJobStatus {
            job_id: job.into(),
            state,
            exit_status: None,
            raw_state: None,
        }
    }

    fn timed_out(id: &str, jobs: Vec<ClusterJobStatus>) -> Event {
        Event::ClusterJobWaitFinished {
            wait_id: id.into(),
            state: ClusterJobWaitState::TimedOut,
            jobs,
            detail: String::new(),
        }
    }

    fn asked(text: &str) -> Event {
        Event::QuestionRaised {
            run_id: "run-1".into(),
            text: text.into(),
        }
    }

    fn seq(events: Vec<Event>) -> Vec<(u64, Event)> {
        events
            .into_iter()
            .enumerate()
            .map(|(i, e)| (i as u64, e))
            .collect()
    }

    #[test]
    fn timed_out_wait_reads_the_wait_before_the_question() {
        let events = seq(vec![
            wait("w1", &["48196", "48197"]),
            timed_out("w1", vec![status("48196", ClusterJobState::Queued)]),
            asked("q1"),
        ]);
        let got = timed_out_wait(&events, "q1").expect("timed out");
        assert_eq!(got.cluster, "sirius");
        assert_eq!(
            got.jobs,
            vec![status("48196", ClusterJobState::Queued), unknown("48197")]
        );
        assert!(timed_out_wait(&events, "other question").is_none());
    }

    #[test]
    fn timed_out_wait_is_none_for_a_satisfied_wait_or_a_wait_after_the_question() {
        let satisfied = seq(vec![
            wait("w1", &["1"]),
            Event::ClusterJobWaitFinished {
                wait_id: "w1".into(),
                state: ClusterJobWaitState::Satisfied,
                jobs: vec![status("1", ClusterJobState::Finished)],
                detail: String::new(),
            },
            asked("q1"),
        ]);
        assert!(timed_out_wait(&satisfied, "q1").is_none());
        let rewaited = seq(vec![
            wait("w1", &["1"]),
            timed_out("w1", vec![status("1", ClusterJobState::Queued)]),
            asked("q1"),
            wait("w2", &["1"]),
        ]);
        assert!(timed_out_wait(&rewaited, "q1").is_none());
    }

    #[test]
    fn timeouts_count_only_waits_sharing_a_job() {
        let events = seq(vec![
            wait("w0", &["7"]),
            timed_out("w0", vec![status("7", ClusterJobState::Queued)]),
            wait("w1", &["1"]),
            timed_out("w1", vec![status("1", ClusterJobState::Queued)]),
            wait("w2", &["1", "2"]),
            timed_out("w2", vec![status("1", ClusterJobState::Queued)]),
            asked("q"),
        ]);
        let current = timed_out_wait(&events, "q").expect("timed out");
        assert_eq!(timeouts_of_same_jobs(&events, &current), 2);
    }
}
