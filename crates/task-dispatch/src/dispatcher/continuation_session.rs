//! ADR-0140 D1/D2: WU（と atomic task。付記 session-container）の execute continuation を同じ Claude Code
//! session で resume するか、checkpoint 前置きの新しい session に倒すか（store の読み書きと記録。判断そのものは
//! `crate::sessions::decide_continuation`、純粋・テスト容易）。LLM は使わない。

use super::*;

use crate::sessions::{
    ContinuationDecision, ContinuationFacts, ContinuationFreshReason, ContinuationRole,
};

/// run の進行（`WorkerProgress`）に残す、continuation の session の判断の行の頭。
pub(super) const CONTINUATION_SESSION_PREFIX: &str = "continuation session:";
/// `StoreSink::session_resume_failed` が resume 拒否を run の進行に残す文言の頭（次の dispatch が
/// `resume_rejected` を判定する印）。
pub(super) const CONTINUATION_RESUME_REJECTED: &str = "continuation session resume rejected";

/// この run の実行面（`decide_continuation` の #8 に要る、dispatch 側の事実）。
pub(super) struct ContinuationSurface<'a> {
    pub provider: Option<&'a str>,
    pub cwd: Option<&'a str>,
    pub container: bool,
}

impl Dispatcher {
    /// ADR-0140 D1: WU の worker run の session を決める。`wu` が `None` なら計画の無い atomic task
    /// （直行経路を含む）の worker run で、task 単位の 1 本（`node_sessions.work_unit_id IS NULL`）を使う
    /// （付記 session-container）。`extras.session`（`--resume` / `--session-id`）と
    /// `extras.continuation_session`（sink が resume 拒否で retire する key）を書き、resume 拒否のやり直しでは
    /// `extras.continuation_override` に直近の checkpoint を入れる。判断は run の進行に 1 行残す。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn resolve_continuation_session(
        &self,
        task: &Task,
        wu: Option<&task_core::WorkUnitRow>,
        run_id: &str,
        role: ContinuationRole,
        adapter_id: &str,
        account: Option<&str>,
        surface: &ContinuationSurface<'_>,
        extras: &mut RunExtras,
    ) -> Result<(), DispatchError> {
        let now = OffsetDateTime::now_utc();
        let wu_id = wu.map(|w| w.id.as_str());
        // atomic task は `runs` 索引の `work_unit_id = NULL` の worker run が続きの系列（planner・reviewer は除く）。
        let runs = match wu {
            Some(wu) => self.store.runs_for_work_unit(&wu.id)?,
            None => self
                .store
                .runs_for_task(task.id)?
                .into_iter()
                .filter(|r| r.work_unit_id.is_none() && r.role == task_core::RunIndexRole::Worker)
                .collect(),
        };
        let previous = runs
            .iter()
            .filter(|r| r.run_id != run_id && r.status != task_core::RunIndexStatus::Running)
            .max_by_key(|r| r.seq);
        let events = match previous {
            Some(_) => self.store.events_for(task.id)?,
            None => Vec::new(),
        };
        let previous_end = previous.map(|p| previous_run_end(&events, p));
        let previous_resume_rejected = previous.is_some_and(|p| {
            events.iter().any(|(_, e)| {
                matches!(e, Event::WorkerProgress { run_id, msg, .. }
                    if *run_id == p.run_id && msg.starts_with(CONTINUATION_RESUME_REJECTED))
            })
        });
        let previous_comment_interrupt =
            previous.is_some_and(|p| interrupted_by_comment(&events, &p.run_id));
        let stored = self.store.work_unit_session_current(task.id, wu_id)?;
        let decision = crate::sessions::decide_continuation(&ContinuationFacts {
            role,
            work_unit_id: wu_id,
            previous_end,
            previous_resume_rejected,
            previous_comment_interrupt,
            fresh_requested: !self.config.execution.continuation_session_resume,
            adapter: adapter_id,
            account,
            provider: surface.provider,
            cwd: surface.cwd,
            container: surface.container,
            stored: stored.as_ref(),
            rollover_tokens: self.config.session_rollover_tokens,
        });
        let line = match decision {
            ContinuationDecision::Resume => {
                let Some(stored) = stored else {
                    return Ok(());
                };
                extras.session = Some(task_worker::protocol::SessionHandle {
                    adapter: adapter_id.to_string(),
                    session_id: stored.session_id.clone(),
                    resume: true,
                });
                format!(
                    "{CONTINUATION_SESSION_PREFIX} resumed (session={})",
                    stored.session_id
                )
            }
            ContinuationDecision::Fresh { reason, retire } => {
                if retire {
                    self.store.work_unit_session_retire(task.id, wu_id, now)?;
                }
                if reason.starts_session() {
                    let session_id = crate::sessions::new_session_id(adapter_id);
                    self.store
                        .work_unit_session_create(&task_core::WorkUnitSession::new(
                            task.assignee.clone().unwrap_or_default(),
                            task.id,
                            wu_id.map(str::to_string),
                            adapter_id,
                            account.map(str::to_string),
                            surface.provider.map(str::to_string),
                            surface.cwd.map(str::to_string),
                            session_id.clone(),
                            now,
                        ))?;
                    extras.session = Some(task_worker::protocol::SessionHandle {
                        adapter: adapter_id.to_string(),
                        session_id,
                        resume: false,
                    });
                }
                // 拒否された resume のやり直し: 失敗した run は checkpoint を残さないので、直近の
                // checkpoint を持つ run から続きの文脈を組み立てる（WU は `ready` に戻っている）。
                if reason == ContinuationFreshReason::ResumeRejected
                    && extras.continuation_override.is_none()
                {
                    let run_seq = match wu {
                        Some(wu) => wu.runs + 1,
                        None => runs.iter().map(|r| r.seq).max().unwrap_or(0) + 1,
                    };
                    extras.continuation_override =
                        latest_checkpoint_context(run_seq, &runs, run_id);
                }
                format!(
                    "{CONTINUATION_SESSION_PREFIX} fresh (reason={})",
                    reason.as_str()
                )
            }
        };
        extras.continuation_session = extras
            .session
            .as_ref()
            .map(|_| (task.id, wu_id.map(str::to_string)));
        // 役割・アダプタの都合で判断の対象外（従来どおり）の run には記録を足さない。
        let quiet = matches!(
            decision,
            ContinuationDecision::Fresh {
                reason: ContinuationFreshReason::RoleFresh
                    | ContinuationFreshReason::AdapterUnsupported,
                ..
            }
        );
        if !quiet {
            self.store.append_event(
                task.id,
                &Event::worker_progress_with(
                    run_id,
                    line,
                    task_core::ProgressFields::of(task_core::ProgressKind::Status),
                ),
            )?;
        }
        Ok(())
    }
}

/// ADR-0140 付記（2026-10-04、comment-resume）: `run_id` の run が人のコメントの割り込み（ADR-0044 D2）で
/// 止まったか。印は 2 通り: atomic task の run は `post_human_comment` が `WorkerFinished{outcome:
/// "interrupted: comment"}` を直接積む。工程の lease の下の WU の run は lease を持たないので、割り込み
/// （`Transitioned{reason: "comment", to: ready}`）の後に `abort_stale_runs` が `interrupted: aborted …` で閉じる。
/// 後者は、その `WorkerFinished` の直前の `Transitioned` がコメントの割り込みであることで見分ける
/// （cancel・lease 回収・shutdown は別の理由の遷移か `interrupted: <別の理由>` なので当たらない）。
fn interrupted_by_comment(events: &[(u64, Event)], run_id: &str) -> bool {
    let Some(idx) = events
        .iter()
        .rposition(|(_, e)| matches!(e, Event::WorkerFinished { run_id: r, .. } if r == run_id))
    else {
        return false;
    };
    let Event::WorkerFinished { outcome, .. } = &events[idx].1 else {
        return false;
    };
    if outcome == task_ops::comment::INTERRUPTED_OUTCOME {
        return true;
    }
    if !outcome.starts_with(task_ops::comment::INTERRUPTED_OUTCOME_PREFIX) {
        return false;
    }
    events[..idx]
        .iter()
        .rev()
        .find_map(|(_, e)| match e {
            Event::Transitioned { reason, to, .. } => {
                Some(reason == "comment" && *to == Status::Ready)
            }
            _ => None,
        })
        .unwrap_or(false)
}

/// ADR-0140 付記（2026-10-04、comment-resume）: resume を拒否された run（[`CONTINUATION_RESUME_REJECTED`] の
/// 進行を持つ）の `WorkerFinished` を除いた events。claude は resume の拒否で前置きを読まずに終わるので、
/// その run は人のコメントの割り込みを「受け取った」ことにならない（`interrupting_comment` の消化の印から外す）。
/// 拒否された run が無ければ写しを作らない。
pub(super) fn without_resume_rejected_finishes(
    events: &[(u64, Event)],
) -> std::borrow::Cow<'_, [(u64, Event)]> {
    let rejected: std::collections::HashSet<&str> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::WorkerProgress { run_id, msg, .. }
                if msg.starts_with(CONTINUATION_RESUME_REJECTED) =>
            {
                Some(run_id.as_str())
            }
            _ => None,
        })
        .collect();
    if rejected.is_empty() {
        return std::borrow::Cow::Borrowed(events);
    }
    std::borrow::Cow::Owned(
        events
            .iter()
            .filter(|(_, e)| {
                !matches!(e, Event::WorkerFinished { run_id, .. } if rejected.contains(run_id.as_str()))
            })
            .cloned()
            .collect(),
    )
}

/// 直前の run の終わり方。`WorkerFinished.end` を正とし、無ければ `runs` 索引の状態から読む
/// （予算切れの種別が分からないときは turns 扱い。分類できないものは continuation ではない）。
fn previous_run_end(events: &[(u64, Event)], previous: &task_core::RunRow) -> task_core::RunEnd {
    let from_event = events.iter().rev().find_map(|(_, e)| match e {
        Event::WorkerFinished { run_id, end, .. } if *run_id == previous.run_id => *end,
        _ => None,
    });
    if let Some(end) = from_event {
        return end;
    }
    match previous.status {
        task_core::RunIndexStatus::Yielded => task_core::RunEnd::Yielded,
        task_core::RunIndexStatus::Waiting => task_core::RunEnd::Waiting,
        task_core::RunIndexStatus::BudgetExhausted => task_core::RunEnd::BudgetExhausted {
            kind: task_core::BudgetKind::Turns,
        },
        task_core::RunIndexStatus::Completed => task_core::RunEnd::Completed,
        task_core::RunIndexStatus::Question => task_core::RunEnd::Question,
        task_core::RunIndexStatus::Cancelled => task_core::RunEnd::Cancelled,
        task_core::RunIndexStatus::Running
        | task_core::RunIndexStatus::Failed
        | task_core::RunIndexStatus::HarnessError => task_core::RunEnd::Failed { retryable: true },
    }
}

/// resume 拒否のやり直しに載せる続きの文脈（checkpoint を持つ直近の run から）。
fn latest_checkpoint_context(
    run_seq: u32,
    runs: &[task_core::RunRow],
    run_id: &str,
) -> Option<task_worker::ContinuationContext> {
    let last = runs
        .iter()
        .filter(|r| r.run_id != run_id && r.checkpoint.is_some())
        .max_by_key(|r| r.seq)?;
    let checkpoint = serde_json::to_value(last.checkpoint.as_ref()?).ok()?;
    Some(task_worker::ContinuationContext {
        run_seq,
        previous_end: "resume_rejected".to_string(),
        checkpoint,
        prior_runs: runs
            .iter()
            .filter(|r| r.run_id != run_id)
            .map(|r| format!("Run #{} {}", r.seq, r.status.as_str()))
            .collect(),
        cluster_jobs: None,
    })
}
