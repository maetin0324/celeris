//! ADR-0090: クラスタ job（PBS / Slurm）の durable wait の dispatcher 側。
//!
//! - run の終わり（`Terminal::Waiting`）の検証と wait の組み立て（[`Dispatcher::cluster_job_wait_for`]）。
//! - tick の poll（[`Dispatcher::poll_cluster_job_waits`]）: `waiting` の wait を `poll_secs` ごとに高々 1 回、
//!   クラスタの ssh master 越しに `qstat -xf` / `sacct` で調べ（OS スレッドに逃がし、結果は次の tick 以降に拾う）、
//!   状態が変わったら `ClusterJobWaitPolled`、すべて終われば `ClusterJobWaitFinished{satisfied}` と task / unit の再開、
//!   上限を過ぎたら `ClusterJobWaitFinished{timed_out}` と人への質問。
//! - continuation の前置きに渡す job の最終状態（[`cluster_jobs_from_events`] / [`Dispatcher::work_unit_cluster_jobs`]）。
//!
//! LLM は呼ばない。job を `qdel` しない（ADR-0090 D6）。

use std::time::Duration;

use task_core::cluster_job::{
    self, ClusterJobState, ClusterJobStatus, ClusterJobWait, ClusterJobWaitRequest,
    ClusterJobWaitState,
};
use task_core::{Event, Status, Task, TaskId, Trigger, WorkUnitBlockedReason, WorkUnitStatus};
use time::OffsetDateTime;

use super::{DispatchError, Dispatcher, rfc3339};

/// poll の ssh の打ち切り（秒）。
const POLL_TIMEOUT: Duration = Duration::from_secs(60);

/// ADR-0090 D2: poll の要求（フックに渡す）。`script` はクラスタで流す 1 行（`setup` の後に `qstat -xf ...`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterJobPollRequest {
    pub cluster: String,
    pub host: String,
    pub script: String,
    pub timeout: Duration,
}

/// ADR-0090 D2: クラスタ job の poll のフック。本番は `task_worker::run_remote_command_blocking`
/// （`ssh -o BatchMode=yes <host> -- <script>`。人が張った ControlMaster を借りる）。`None` なら poll しない
/// （上限の判定だけは行う）。試験は偽物を挿す（外部ネットワークに出ない）。
pub type ClusterJobPoller = std::sync::Arc<
    dyn Fn(&ClusterJobPollRequest) -> Result<task_worker::RemoteCommandOutput, String>
        + Send
        + Sync,
>;

/// 本番のフック（celeris が `set_cluster_job_poller` で挿す）。
pub fn ssh_cluster_job_poller() -> ClusterJobPoller {
    std::sync::Arc::new(|req: &ClusterJobPollRequest| {
        let ssh_command = task_worker::SshSettings::new("", "", "/").ssh_command;
        task_worker::run_remote_command_blocking(&ssh_command, &req.host, &req.script, req.timeout)
    })
}

fn parse_ts(s: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
}

/// 状態の変化の比較に使う形（job id・状態・終了コード。生の文字は比べない）。
fn state_key(statuses: &[ClusterJobStatus]) -> Vec<(String, ClusterJobState, Option<i32>)> {
    statuses
        .iter()
        .map(|s| (s.job_id.clone(), s.state, s.exit_status))
        .collect()
}

/// ADR-0090 D2: continuation の前置きに渡す job の結果（wait と、その終わりの event の job の状態から）。
pub(super) fn continuation_of(
    wait: &ClusterJobWait,
    state: ClusterJobWaitState,
    jobs: &[ClusterJobStatus],
) -> task_worker::ClusterJobsContinuation {
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
                })
        })
        .collect();
    task_worker::ClusterJobsContinuation {
        cluster: wait.cluster.clone(),
        scheduler: wait.scheduler.as_str().to_string(),
        state: state.as_str().to_string(),
        jobs: cluster_job::preamble_lines(&statuses),
        summary: wait.summary.clone(),
    }
}

/// ADR-0090 D2: 暗黙の WorkUnit（atomic の run）の直前の wait の結果を events から組む。直前のワーカー run が
/// `waiting` で終わっていて、その run の wait が終わっている（`ClusterJobWaitFinished`）ときだけ `Some`。
pub(super) fn cluster_jobs_from_events(
    events: &[(u64, Event)],
) -> Option<task_worker::ClusterJobsContinuation> {
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
    let (state, jobs) = events.iter().rev().find_map(|(_, ev)| match ev {
        Event::ClusterJobWaitFinished {
            wait_id,
            state,
            jobs,
            ..
        } if wait_id == &wait.wait_id => Some((*state, jobs.clone())),
        _ => None,
    })?;
    Some(continuation_of(&wait, state, &jobs))
}

/// 直前のワーカー run（暗黙の WorkUnit）が `waiting` で終わったか（continuation の前置きを出すか）。
pub(super) fn last_worker_run_waited(events: &[(u64, Event)]) -> bool {
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

impl Dispatcher {
    /// ADR-0090 D7: 本番の poll のフックを挿す（celeris 側の配線）。
    pub fn set_cluster_job_poller(&mut self, poller: ClusterJobPoller) {
        self.cluster_job_poller = Some(poller);
    }

    /// ADR-0090 D1/D2: worker の wait の申告を検証し、wait の行を組む。クラスタは申告（省略時は task のクラスタ）で、
    /// `[[clusters]]` にあること。remote の task は自分が走っているクラスタ（`WorkspaceSpec::Remote{cluster}`）と同じで
    /// なければならない。local の task（手元から ssh で投げた job）はクラスタを明示すること。
    pub(super) fn cluster_job_wait_for(
        &self,
        task: &Task,
        run_id: &str,
        work_unit_id: Option<&str>,
        request: &ClusterJobWaitRequest,
        checkpoint: Option<serde_json::Value>,
    ) -> Result<ClusterJobWait, String> {
        let task_cluster = match &task.workspace {
            task_core::WorkspaceSpec::Remote { cluster, .. } => Some(cluster.clone()),
            task_core::WorkspaceSpec::Local { .. } => None,
        };
        let cluster = match (&request.cluster, &task_cluster) {
            (Some(c), Some(t)) if c != t => {
                return Err(format!("cluster {c:?} is not the task's cluster {t:?}"));
            }
            (Some(c), _) => c.clone(),
            (None, Some(t)) => t.clone(),
            (None, None) => {
                return Err(
                    "the task does not run on a cluster; name the cluster in the wait".to_string(),
                );
            }
        };
        let Some(spec) = self.config.clusters.get(&cluster) else {
            return Err(format!(
                "cluster {cluster:?} is not configured ([[clusters]])"
            ));
        };
        let (poll_secs, timeout_secs) = spec.job_wait.clamp(request);
        let now = self.now_utc();
        let deadline =
            now + time::Duration::seconds(i64::try_from(timeout_secs).unwrap_or(i64::MAX));
        Ok(ClusterJobWait {
            wait_id: task_core::execution_plan::new_id(),
            task_id: task.id,
            work_unit_id: work_unit_id.map(str::to_string),
            run_id: run_id.to_string(),
            cluster,
            scheduler: request.scheduler,
            jobs: request.jobs.clone(),
            poll_secs,
            timeout_secs,
            summary: request.summary.clone(),
            checkpoint,
            created_at: rfc3339(now),
            deadline: rfc3339(deadline),
            last_polled_at: None,
            finished_at: None,
            state: ClusterJobWaitState::Waiting,
            last_status: Vec::new(),
        })
    }

    /// ADR-0090 D2: WU の continuation の前置きに渡す、その WU の直前の wait の結果（終わった wait だけ）。
    pub(super) fn work_unit_cluster_jobs(
        &self,
        task_id: TaskId,
        work_unit_id: &str,
        last_run_id: &str,
    ) -> Option<task_worker::ClusterJobsContinuation> {
        let waits = self.store.cluster_job_waits_for_task(task_id).ok()?;
        let wait = waits
            .into_iter()
            .rev()
            .find(|w| w.work_unit_id.as_deref() == Some(work_unit_id) && w.run_id == last_run_id)?;
        if wait.state == ClusterJobWaitState::Waiting {
            return None;
        }
        Some(continuation_of(&wait, wait.state, &wait.last_status))
    }

    /// ADR-0090 D2: tick の poll。`waiting` の wait ごとに (1) 終わった poll の結果を拾う → (2) 上限を過ぎていれば
    /// `timed_out` → (3) `poll_secs` が経っていて poll が走っていなければ、次の poll を OS スレッドで起こす。
    pub(super) fn poll_cluster_job_waits(&mut self) -> Result<(), DispatchError> {
        let waits = self.store.cluster_job_waits_waiting()?;
        let live: std::collections::HashSet<String> =
            waits.iter().map(|w| w.wait_id.clone()).collect();
        self.cluster_job_polls.retain(|id, _| live.contains(id));
        let now = self.now_utc();
        for wait in waits {
            if let Some(rx) = self.cluster_job_polls.get(&wait.wait_id) {
                let result = match rx.try_recv() {
                    Ok(result) => Some(result),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        Some(Err("the poll thread ended without a result".to_string()))
                    }
                };
                if let Some(result) = result {
                    self.cluster_job_polls.remove(&wait.wait_id);
                    if self.on_cluster_job_poll(&wait, result, now)? {
                        continue;
                    }
                } else {
                    // poll が走っている間は上限の判定も次の poll も見送る（結果を先に拾う）。
                    continue;
                }
            }
            if parse_ts(&wait.deadline).is_some_and(|d| d <= now) {
                self.time_out_cluster_job_wait(&wait, now)?;
                continue;
            }
            let Some(poller) = self.cluster_job_poller.clone() else {
                continue;
            };
            let due = wait
                .last_polled_at
                .as_deref()
                .and_then(parse_ts)
                .is_none_or(|last| {
                    (now - last).whole_seconds()
                        >= i64::try_from(wait.poll_secs).unwrap_or(i64::MAX)
                });
            if !due {
                continue;
            }
            let Some(spec) = self.config.clusters.get(&wait.cluster).cloned() else {
                tracing::warn!(task_id = %wait.task_id, wait_id = %wait.wait_id, cluster = %wait.cluster, "cluster job wait: the cluster is no longer configured; not polling (ADR-0090)");
                self.store.cluster_job_wait_touch(
                    &wait.wait_id,
                    &rfc3339(now),
                    &wait.last_status,
                )?;
                continue;
            };
            // 明示的に切れているクラスタ（master が無い）には ssh を流さない（戻ったら次の tick で poll する）。
            if self.cluster_connected.get(&wait.cluster) == Some(&false) {
                continue;
            }
            let mut script = String::new();
            for (k, v) in &spec.env {
                script.push_str(&format!("export {k}={} && ", shell_quote(v)));
            }
            for line in &spec.setup {
                script.push_str(&format!("{{ {line}; }} && "));
            }
            script.push_str(&cluster_job::poll_command(wait.scheduler, &wait.jobs));
            let req = ClusterJobPollRequest {
                cluster: wait.cluster.clone(),
                host: spec.host.clone(),
                script,
                timeout: POLL_TIMEOUT,
            };
            // poll を起こした時刻を先に残す（再起動の直後に同じ wait を続けて poll しない）。
            self.store
                .cluster_job_wait_touch(&wait.wait_id, &rfc3339(now), &wait.last_status)?;
            let (tx, rx) = std::sync::mpsc::channel();
            match std::thread::Builder::new()
                .name(format!("celeris-job-poll-{}", wait.cluster))
                .spawn(move || {
                    let _ = tx.send(poller(&req));
                }) {
                Ok(_) => {
                    self.cluster_job_polls.insert(wait.wait_id.clone(), rx);
                }
                Err(e) => {
                    tracing::warn!(wait_id = %wait.wait_id, error = %e, "could not start the cluster job poll thread");
                }
            }
        }
        Ok(())
    }

    /// poll の結果を反映する。wait を終わらせた（satisfied）なら `true`。
    fn on_cluster_job_poll(
        &mut self,
        wait: &ClusterJobWait,
        result: Result<task_worker::RemoteCommandOutput, String>,
        now: OffsetDateTime,
    ) -> Result<bool, DispatchError> {
        let out = match result {
            Ok(out) => out,
            Err(e) => {
                tracing::warn!(task_id = %wait.task_id, wait_id = %wait.wait_id, cluster = %wait.cluster, error = %e, "cluster job poll failed; retrying after poll_secs (ADR-0090)");
                return Ok(false);
            }
        };
        let statuses =
            cluster_job::parse_poll_output(wait.scheduler, &out.stdout, &out.stderr, &wait.jobs);
        let nothing_known = statuses.iter().all(|s| s.state == ClusterJobState::Unknown);
        if out.exit != Some(0) && nothing_known {
            tracing::warn!(task_id = %wait.task_id, wait_id = %wait.wait_id, cluster = %wait.cluster, exit = ?out.exit, stderr = %out.stderr.trim(), "cluster job poll returned no job states; retrying after poll_secs (ADR-0090)");
            return Ok(false);
        }
        if cluster_job::all_finished(&wait.jobs, &statuses) {
            tracing::info!(task_id = %wait.task_id, wait_id = %wait.wait_id, cluster = %wait.cluster, jobs = %cluster_job::status_line(&statuses), "cluster jobs finished; resuming as a continuation run (ADR-0090)");
            self.finish_cluster_job_wait(
                wait,
                ClusterJobWaitState::Satisfied,
                statuses,
                String::new(),
                now,
            )?;
            return Ok(true);
        }
        if state_key(&statuses) != state_key(&wait.last_status) {
            tracing::info!(task_id = %wait.task_id, wait_id = %wait.wait_id, jobs = %cluster_job::status_line(&statuses), "cluster job states changed (ADR-0090)");
            self.store.append_event(
                wait.task_id,
                &Event::ClusterJobWaitPolled {
                    wait_id: wait.wait_id.clone(),
                    jobs: statuses,
                },
            )?;
        } else {
            self.store
                .cluster_job_wait_touch(&wait.wait_id, &rfc3339(now), &statuses)?;
        }
        Ok(false)
    }

    /// 上限を過ぎた wait。atomic（と v1 の WU）の task は `blocked` のまま人に聞く（延長 / job の取り消し / 取り下げ）。
    /// v2 の WU は unit を continuation に戻し、続きの run が前置きの指示どおり人に聞く（task は止めない）。
    fn time_out_cluster_job_wait(
        &mut self,
        wait: &ClusterJobWait,
        now: OffsetDateTime,
    ) -> Result<(), DispatchError> {
        let statuses = wait.job_statuses();
        tracing::warn!(task_id = %wait.task_id, wait_id = %wait.wait_id, cluster = %wait.cluster, jobs = %cluster_job::status_line(&statuses), "cluster job wait timed out (ADR-0090)");
        let detail = format!("the deadline {} passed", wait.deadline);
        let Some(task) = self.store.get(wait.task_id)? else {
            return Ok(());
        };
        let held_task = task.status == Status::Blocked
            && task_ops::phase_gate::last_transition_reason(&self.store.events_for(task.id)?)
                == Some(cluster_job::REASON_WAITING);
        if wait.work_unit_id.is_none() || (held_task && self.wu_is_v1(&task)?) {
            // 人への質問（task は `blocked` のまま。回答で `ready` に戻り、次の run が答えと job の状態を受け取る）。
            let text = format!(
                "クラスタ `{}` の job の待ちが上限（{} 秒）に達しました: {}。\
                 延長する（job を待ち直す）／job を取り消して続ける／取り下げる（この task を中止）のどれにするか答えてください\
                 （回答は次の run に渡ります。celeris は job を qdel しません）。",
                wait.cluster,
                wait.timeout_secs,
                cluster_job::status_line(&statuses)
            );
            let mut events = vec![Event::ClusterJobWaitFinished {
                wait_id: wait.wait_id.clone(),
                state: ClusterJobWaitState::TimedOut,
                jobs: wait.last_status.clone(),
                detail,
            }];
            events.push(Event::QuestionRaised {
                run_id: wait.run_id.clone(),
                text: text.clone(),
            });
            if let Some(wu_id) = &wait.work_unit_id {
                // v1 の WU: 回答の後の照合（`resume_after_answer`）が `ready` に戻せるよう、blocked(question) にする。
                if let Some((row, ev)) = self.cluster_wait_unit_row(
                    wait.task_id,
                    wu_id,
                    WorkUnitStatus::Blocked,
                    Some(WorkUnitBlockedReason::Question),
                    "cluster_jobs_timed_out",
                    &wait.run_id,
                    now,
                )? {
                    events.push(ev);
                    self.store
                        .work_units_apply(wait.task_id, Vec::new(), vec![row], events)?;
                } else {
                    for ev in &events {
                        self.store.append_event(wait.task_id, ev)?;
                    }
                }
            } else {
                for ev in &events {
                    self.store.append_event(wait.task_id, ev)?;
                }
            }
            if let Err(e) =
                crate::approvals::record_question_approval(self.store.as_ref(), &task, &text, now)
            {
                tracing::warn!(task_id = %task.id, error = %e, "failed to record the approval for the cluster job wait timeout");
            }
            return Ok(());
        }
        self.finish_cluster_job_wait(
            wait,
            ClusterJobWaitState::TimedOut,
            wait.last_status.clone(),
            detail,
            now,
        )
    }

    /// wait を `state`（satisfied / v2 の WU の timed_out）で閉じ、unit を `needs_continuation` に戻し、
    /// wait で止めていた task を `ready` に戻す（`ClusterJobResume`）。
    fn finish_cluster_job_wait(
        &mut self,
        wait: &ClusterJobWait,
        state: ClusterJobWaitState,
        jobs: Vec<ClusterJobStatus>,
        detail: String,
        now: OffsetDateTime,
    ) -> Result<(), DispatchError> {
        let finished = Event::ClusterJobWaitFinished {
            wait_id: wait.wait_id.clone(),
            state,
            jobs,
            detail,
        };
        let mut task_events = Vec::new();
        match &wait.work_unit_id {
            Some(wu_id) => {
                let reason = if state == ClusterJobWaitState::Satisfied {
                    "cluster_jobs_finished"
                } else {
                    "cluster_jobs_timed_out"
                };
                let mut events = vec![finished];
                let mut rows = Vec::new();
                if let Some((row, ev)) = self.cluster_wait_unit_row(
                    wait.task_id,
                    wu_id,
                    WorkUnitStatus::NeedsContinuation,
                    None,
                    reason,
                    &wait.run_id,
                    now,
                )? {
                    rows.push(row);
                    events.push(ev);
                }
                self.store
                    .work_units_apply(wait.task_id, Vec::new(), rows, events)?;
            }
            None => task_events.push(finished),
        }
        let Some(task) = self.store.get(wait.task_id)? else {
            return Ok(());
        };
        let held = task.status == Status::Blocked
            && task_ops::phase_gate::last_transition_reason(&self.store.events_for(task.id)?)
                == Some(cluster_job::REASON_WAITING);
        if held {
            match self.store.apply_transition_with_events(
                task.id,
                Trigger::ClusterJobResume,
                task_events.clone(),
            ) {
                Ok(outcome) => {
                    tracing::info!(task_id = %task.id, next = ?outcome.next, "task resumed after its cluster jobs (ADR-0090)");
                    return Ok(());
                }
                Err(e) => {
                    tracing::warn!(task_id = %task.id, error = %e, "could not resume the task after its cluster jobs");
                }
            }
        }
        for ev in &task_events {
            self.store.append_event(task.id, ev)?;
        }
        Ok(())
    }

    /// wait を持つ unit の新しい行（`blocked(cluster_jobs)` のときだけ）と `WorkUnitTransitioned`。
    #[allow(clippy::too_many_arguments)]
    fn cluster_wait_unit_row(
        &self,
        task_id: TaskId,
        wu_id: &str,
        to: WorkUnitStatus,
        blocked_reason: Option<WorkUnitBlockedReason>,
        reason: &str,
        run_id: &str,
        now: OffsetDateTime,
    ) -> Result<Option<(task_core::WorkUnitRow, Event)>, DispatchError> {
        let units = self.store.work_units_for(task_id)?;
        let Some(u) = units.into_iter().find(|u| {
            u.id == wu_id
                && u.status == WorkUnitStatus::Blocked
                && u.blocked_reason == Some(WorkUnitBlockedReason::ClusterJobs)
        }) else {
            return Ok(None);
        };
        let mut row = u.clone();
        row.status = to;
        row.blocked_reason = blocked_reason;
        row.updated_at = rfc3339(now);
        let ev = Event::WorkUnitTransitioned {
            work_unit_id: u.id.clone(),
            key: u.key.clone(),
            from: u.status,
            to,
            reason: reason.to_string(),
            run_id: Some(run_id.to_string()),
        };
        Ok(Some((row, ev)))
    }

    /// 有効な計画が段階を持たない（v1）か。
    fn wu_is_v1(&self, task: &Task) -> Result<bool, DispatchError> {
        Ok(self
            .store
            .execution_plan_active(task.id)?
            .is_some_and(|p| !task_core::is_phased_schema(&p.spec.schema)))
    }
}

/// シェルの 1 引数に引用する（`'...'`）。
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
