use task_core::Event;
use task_core::cluster_job::{
    self, ClusterJobState, ClusterJobStatus, ClusterJobWait, ClusterJobWaitState,
};

/// ADR-0090 D2: continuation の前置きに渡す job の結果（wait と、その終わりの event の job の状態から）。
pub fn continuation_of(
    wait: &ClusterJobWait,
    state: ClusterJobWaitState,
    jobs: &[ClusterJobStatus],
) -> crate::ClusterJobsContinuation {
    let statuses: Vec<ClusterJobStatus> = wait
        .jobs
        .iter()
        .map(|j| {
            jobs.iter()
                .find(|s| &s.job_id == j)
                .cloned()
                .unwrap_or(ClusterJobStatus {
                    job_id: j.clone(),
                    state: ClusterJobState::Unknown,
                    exit_status: None,
                    raw_state: None,
                    scheduling: Vec::new(),
                    queued_since: None,
                    queue_notified: false,
                })
        })
        .collect();
    crate::ClusterJobsContinuation {
        cluster: wait.cluster.clone(),
        scheduler: wait.scheduler.as_str().to_string(),
        state: state.as_str().to_string(),
        jobs: cluster_job::preamble_lines(&statuses),
        summary: wait.summary.clone(),
    }
}

/// ADR-0090 D2: 暗黙の WorkUnit（atomic の run）の直前の wait の結果を events から組む。直前のワーカー run が
/// `waiting` で終わっていて、その run の wait が終わっている（`ClusterJobWaitFinished`）ときだけ `Some`。
pub fn cluster_jobs_from_events(events: &[(u64, Event)]) -> Option<crate::ClusterJobsContinuation> {
    let last_run = events.iter().rev().find_map(|(_, ev)| match ev {
        Event::WorkerFinished {
            run_id,
            role: None,
            end,
            ..
        } => Some((run_id.clone(), *end)),
        _ => None,
    })?;
    if last_run.1 != Some(task_core::RunEnd::Waiting) {
        return None;
    }
    let wait = events.iter().rev().find_map(|(_, ev)| match ev {
        Event::ClusterJobWaitStarted { wait } if wait.run_id == last_run.0 => Some(wait.clone()),
        _ => None,
    })?;
    let (state, jobs, detail) = events.iter().rev().find_map(|(_, ev)| match ev {
        Event::ClusterJobWaitFinished {
            wait_id,
            state,
            jobs,
            detail,
            ..
        } if wait_id == &wait.wait_id => Some((*state, jobs.clone(), detail.clone())),
        _ => None,
    })?;
    let mut result = continuation_of(&wait, state, &jobs);
    if matches!(detail.as_str(), "job_failed" | "queue_stalled") {
        result.state = detail;
    }
    Some(result)
}

/// 直前のワーカー run（暗黙の WorkUnit）が `waiting` で終わったか（continuation の前置きを出すか）。
pub fn last_worker_run_waited(events: &[(u64, Event)]) -> bool {
    events
        .iter()
        .rev()
        .find_map(|(_, ev)| match ev {
            Event::WorkerFinished {
                role: None, end, ..
            } => Some(*end == Some(task_core::RunEnd::Waiting)),
            _ => None,
        })
        .unwrap_or(false)
}

pub fn wake_reason(events: &[(u64, Event)], id: &str) -> Option<String> {
    events.iter().rev().find_map(|(_, event)| match event {
        Event::ClusterJobWaitFinished {
            wait_id, detail, ..
        } if wait_id == id && matches!(detail.as_str(), "job_failed" | "queue_stalled") => {
            Some(detail.clone())
        }
        _ => None,
    })
}
