//! ADR-0130 D5: order the ADR-0118 pre-review sync queue so that long-stale task branches
//! are synced first.
//!
//! `recover_reviews` keeps one in-memory entry per waiting `reviewing` task (wait order and the
//! number of consecutive eligible ticks it was not selected). When two or more candidates wait
//! in the same tick, each is re-measured against its current target just before ordering
//! (ADR-0130 D4: the priority only uses values measured right before the review sync). The
//! order is a hint: it takes no capacity or lock, never interrupts a running review, and the
//! sync itself (target re-read, stale re-sync, `SyncOutcome::Conflict` → IntegrationRepair, no
//! attempt consumption) is unchanged in `spawn_review`.

use std::cmp::Ordering;
use task_core::behind_target::{BehindTargetSnapshot, summarize_behind_target};
use task_core::{Task, TaskId};

use super::{DispatchError, Dispatcher};

/// Consecutive eligible ticks without being selected before a task is raised to the FIFO
/// relief slot ahead of the age order.
pub(super) const STALE_PRIORITY_RELIEF_TICKS: u32 = 3;

/// One waiter of the pre-review sync queue (in memory; rebuilt after a restart).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ReviewSyncWait {
    /// Wait start order (monotonic per dispatcher).
    pub(super) seq: u64,
    /// Consecutive eligible ticks in which the review was not started.
    pub(super) skipped_ticks: u32,
}

/// Ordering key of one candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct StaleCandidate {
    pub(super) task_id: TaskId,
    pub(super) wait: ReviewSyncWait,
    /// Freshly measured representative behind commits (`None` = not measured / unreadable).
    pub(super) behind_commits: Option<u64>,
    pub(super) behind_age_seconds: Option<u64>,
}

impl StaleCandidate {
    fn relieved(&self) -> bool {
        self.wait.skipped_ticks >= STALE_PRIORITY_RELIEF_TICKS
    }

    fn stale(&self) -> bool {
        self.behind_commits.is_some_and(|c| c > 0)
    }
}

/// ADR-0130 D5 order: FIFO relief (wait order) → behind > 0 (age desc, commits desc, wait
/// order, task id) → the rest in the existing wait order.
pub(super) fn stale_priority_cmp(a: &StaleCandidate, b: &StaleCandidate) -> Ordering {
    let class = |c: &StaleCandidate| match (c.relieved(), c.stale()) {
        (true, _) => 0u8,
        (false, true) => 1,
        (false, false) => 2,
    };
    class(a).cmp(&class(b)).then_with(|| {
        if class(a) == 1 {
            b.behind_age_seconds
                .cmp(&a.behind_age_seconds)
                .then(b.behind_commits.cmp(&a.behind_commits))
                .then(a.wait.seq.cmp(&b.wait.seq))
                .then(a.task_id.cmp(&b.task_id))
        } else {
            a.wait.seq.cmp(&b.wait.seq)
        }
    })
}

impl Dispatcher {
    /// Keep the queue entries of `candidates` only and give new waiters the next wait order
    /// (in `candidates` order, i.e. the store's listing order).
    fn enter_review_sync_queue(&mut self, candidates: &[Task]) {
        self.review_sync_queue
            .retain(|id, _| candidates.iter().any(|t| t.id == *id));
        for task in candidates {
            if !self.review_sync_queue.contains_key(&task.id) {
                let seq = self.review_sync_seq;
                self.review_sync_seq += 1;
                self.review_sync_queue.insert(
                    task.id,
                    ReviewSyncWait {
                        seq,
                        skipped_ticks: 0,
                    },
                );
            }
        }
    }

    /// Re-measure behind of every local Git repo of `task` against its review sync target.
    fn measure_review_sync_behind(
        &mut self,
        task: &Task,
    ) -> Result<Vec<BehindTargetSnapshot>, DispatchError> {
        let mut snapshots = Vec::new();
        for (repo_id, worktree, target_ref) in self.review_sync_targets(task)? {
            if let Some(snapshot) =
                self.observe_behind_target(task.id, repo_id, &worktree, &target_ref)
            {
                snapshots.push(snapshot);
            }
        }
        Ok(snapshots)
    }

    /// ADR-0130 D5: the order in which `recover_reviews` offers the waiting tasks to the
    /// pre-review sync.
    pub(super) fn order_review_sync_queue(
        &mut self,
        candidates: Vec<Task>,
    ) -> Result<Vec<Task>, DispatchError> {
        self.enter_review_sync_queue(&candidates);
        #[cfg(test)]
        if self.test_disable_stale_priority {
            // phase_effect_ab::stale_priority の off: 並べ替えずに候補の順（store の一覧順）で渡す。
            return Ok(candidates);
        }
        if candidates.len() < 2 {
            return Ok(candidates);
        }
        let now = self.now_utc();
        let mut keyed = Vec::with_capacity(candidates.len());
        for task in candidates {
            let summary = summarize_behind_target(&self.measure_review_sync_behind(&task)?, now);
            let wait = self
                .review_sync_queue
                .get(&task.id)
                .copied()
                .unwrap_or(ReviewSyncWait {
                    seq: u64::MAX,
                    skipped_ticks: 0,
                });
            keyed.push((
                StaleCandidate {
                    task_id: task.id,
                    wait,
                    behind_commits: summary.behind_target_commits,
                    behind_age_seconds: summary.behind_target_age_seconds,
                },
                task,
            ));
        }
        keyed.sort_by(|(a, _), (b, _)| stale_priority_cmp(a, b));
        Ok(keyed.into_iter().map(|(_, t)| t).collect())
    }

    /// Record the outcome of one offer: a started review leaves the queue; a deferral that was
    /// not a human gate counts towards the FIFO relief.
    pub(super) fn note_review_sync_offer(&mut self, task_id: TaskId, started: bool) {
        if started {
            self.review_sync_queue.remove(&task_id);
            return;
        }
        if self.awaiting_human.contains(&task_id) {
            return;
        }
        if let Some(wait) = self.review_sync_queue.get_mut(&task_id) {
            wait.skipped_ticks = wait.skipped_ticks.saturating_add(1);
        }
    }
}
