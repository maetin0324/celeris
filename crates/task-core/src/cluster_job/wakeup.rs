//! Early wake policy. History comes from durable wait rows, never worker memory.
use super::{
    ClusterJobPollFailure, ClusterJobState, ClusterJobStatus, ClusterJobWait, ClusterScheduler,
    POLL_ERROR_MAX_CHARS, WAKE_JOB_FAILED, WAKE_POLL_FAILED, WAKE_QUEUE_STALLED,
};
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
                    ClusterScheduler::Pbs | ClusterScheduler::Nqsv => {
                        super::job_matches(&j.job_id, &job.job_id)
                    }
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
        Some(WAKE_JOB_FAILED)
    } else if queued {
        Some(WAKE_QUEUE_STALLED)
    } else {
        None
    }
}

/// 失敗の poll（job の状態を返さない poll。ADR 2026-11-25-cluster-scheduler-from-config D4）を記録した job の状態と、
/// 続きの run を起こすか（`Some(WAKE_POLL_FAILED)`）。
///
/// - job の状態は wait の `last_status`（無ければ `Unknown`）をそのまま残し、各 job の `poll_failure` に連続した失敗の
///   最初の時刻（既にあればそれ、無ければ `now`）と最後の失敗の要約を書く。
/// - 起こすのは `now - since >= threshold` のときだけ。最初の失敗では `now - since = 0` なので起きない。job の状態を
///   返した poll の結果（`parse_poll_output`）は `poll_failure` を持たないので、Q・R を読めた poll で連続は切れる。
pub fn observe_poll_failure(
    wait: &ClusterJobWait,
    error: &str,
    now: OffsetDateTime,
    threshold: u64,
) -> (Vec<ClusterJobStatus>, Option<&'static str>) {
    let mut jobs: Vec<ClusterJobStatus> = wait
        .jobs
        .iter()
        .map(|j| {
            wait.last_status
                .iter()
                .find(|s| &s.job_id == j)
                .cloned()
                .unwrap_or_else(|| ClusterJobStatus::unknown(j))
        })
        .collect();
    let since = jobs
        .iter()
        .filter_map(|j| j.poll_failure.as_ref())
        .filter_map(|f| timestamp(&f.since))
        .min()
        .unwrap_or(now);
    let failure = ClusterJobPollFailure {
        since: since
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        error: super::truncate_chars(error.trim(), POLL_ERROR_MAX_CHARS),
    };
    for job in &mut jobs {
        job.poll_failure = Some(failure.clone());
    }
    let wake = (now - since).whole_seconds() >= i64::try_from(threshold).unwrap_or(i64::MAX);
    (jobs, wake.then_some(WAKE_POLL_FAILED))
}
