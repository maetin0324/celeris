//! ADR-0130 D4: record the task branch's behind commits against its target at the review
//! sync measurement points. Never fails the caller (Git/store errors are only logged).

use std::path::Path;

use task_core::{RepoId, TaskId};

use super::Dispatcher;

impl Dispatcher {
    pub(super) fn observe_behind_target(
        &self,
        task_id: TaskId,
        repo_id: RepoId,
        worktree: &Path,
        target_ref: &str,
    ) {
        if let Err(error) = task_ops::behind_target::observe_behind_target(
            self.store.as_ref(),
            task_id,
            repo_id,
            worktree,
            target_ref,
            self.now_utc(),
        ) {
            tracing::warn!(%task_id, %target_ref, %error, "behind target observation failed");
        }
    }
}
