//! Early wake policy. History comes from durable wait rows, never worker memory.
use super::{ClusterJobState, ClusterJobStatus, ClusterJobWait, ClusterScheduler};
use time::OffsetDateTime;

fn timestamp(s: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
}

fn failed(job: &ClusterJobStatus) -> bool {
    job.state == ClusterJobState::Finished
        && (job.exit_status.is_some_and(|n| n != 0)
            || job.raw_state.as_deref().is_some_and(|s| {
                super::slurm_state(s) == ClusterJobState::Finished && !s.starts_with("COMPLETED")
            }))
}

/// Enrich fresh observations with durable queue clocks and notification receipts.
///
/// Both wake reasons fire once per occurrence, judged only from durable wait rows:
/// - `queue_stalled`: once per job (`queue_notified` survives re-waits).
/// - `job_failed`: once per failure. A failed job wakes the worker only when its latest
///   earlier observation (this wait's `last_status`, else the newest past wait of the same
///   task, WorkUnit, cluster and scheduler; `Unknown` rows are not observations) was not
///   already a failure. Re-waiting with an inspected failed job therefore does not wake again,
///   while a same-ID rerun that was seen in Q/R/H and then fails wakes again. A rerun that starts
///   and fails between two polls is not detected (the scheduler shows the same failure).
pub fn observe(
    wait: &ClusterJobWait,
    history: &[ClusterJobWait],
    jobs: &mut [ClusterJobStatus],
    now: OffsetDateTime,
    threshold: u64,
) -> Option<&'static str> {
    let mut failure = false;
    let mut queued = false;
    for job in jobs {
        let previous: Vec<_> = std::iter::once(wait)
            .chain(history.iter().rev().filter(|w| w.wait_id != wait.wait_id))
            .filter(|w| {
                w.cluster == wait.cluster
                    && w.scheduler == wait.scheduler
                    && w.work_unit_id == wait.work_unit_id
            })
            .filter_map(|w| {
                w.last_status.iter().find(|j| match wait.scheduler {
                    ClusterScheduler::Pbs => super::job_matches(&j.job_id, &job.job_id),
                    ClusterScheduler::Slurm => j.job_id == job.job_id,
                })
            })
            .collect();
        job.queue_notified = previous.iter().any(|j| j.queue_notified);
        if matches!(job.state, ClusterJobState::Queued | ClusterJobState::Held) {
            job.queued_since = previous
                .first()
                .and_then(|p| p.queued_since.clone())
                .or_else(|| {
                    now.format(&time::format_description::well_known::Rfc3339)
                        .ok()
                });
            if !job.queue_notified
                && job
                    .queued_since
                    .as_deref()
                    .and_then(timestamp)
                    .is_some_and(|start| {
                        (now - start).whole_seconds()
                            >= i64::try_from(threshold).unwrap_or(i64::MAX)
                    })
            {
                job.queue_notified = true;
                queued = true;
            }
        }
        let last_seen = previous
            .iter()
            .find(|p| p.state != ClusterJobState::Unknown);
        if failed(job) && !last_seen.is_some_and(|p| failed(p)) {
            failure = true;
        }
    }
    if failure {
        Some("job_failed")
    } else if queued {
        Some("queue_stalled")
    } else {
        None
    }
}
