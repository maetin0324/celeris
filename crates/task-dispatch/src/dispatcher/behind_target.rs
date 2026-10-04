//! ADR-0130 D4: record the task branch's behind commits against its target at the review
//! sync measurement points. Never fails the caller (Git/store errors are only logged).

use std::path::{Path, PathBuf};

use task_core::behind_target::BehindTargetSnapshot;
use task_core::{RepoId, Task, TaskId};

use super::{DispatchError, Dispatcher};

impl Dispatcher {
    pub(super) fn observe_behind_target(
        &self,
        task_id: TaskId,
        repo_id: RepoId,
        worktree: &Path,
        target_ref: &str,
    ) -> Option<BehindTargetSnapshot> {
        match task_ops::behind_target::observe_behind_target(
            self.store.as_ref(),
            task_id,
            repo_id,
            worktree,
            target_ref,
            self.now_utc(),
        ) {
            Ok(snapshot) => Some(snapshot),
            Err(error) => {
                tracing::warn!(%task_id, %target_ref, %error, "behind target observation failed");
                None
            }
        }
    }

    /// ADR-0118 review sync target of one repo: the parent task branch for a tree child,
    /// otherwise the repo's default branch.
    pub(super) fn review_target_ref(
        &self,
        task: &Task,
        repo_id: RepoId,
        source: &Path,
    ) -> Result<String, DispatchError> {
        let target = if let Some(parent) =
            task_core::tree::parent_branch(task, &self.config.worktree_branch_prefix)
        {
            parent
        } else {
            let configured = self.store.repo_get(repo_id)?.and_then(|r| r.default_branch);
            task_ops::changes::default_branch(source, configured.as_deref())
        };
        Ok(format!("refs/heads/{target}"))
    }

    /// `(repo, worktree, target_ref)` of every local Git worktree the review sync of `task`
    /// would rebase. Remote / projectless / non-Git workspaces have none (behind stays `null`).
    pub(super) fn review_sync_targets(
        &mut self,
        task: &Task,
    ) -> Result<Vec<(RepoId, PathBuf, String)>, DispatchError> {
        let remote = matches!(&task.workspace, task_core::WorkspaceSpec::Remote { .. })
            || self.cluster_of(task).is_some();
        if remote || task.repos.is_empty() {
            return Ok(Vec::new());
        }
        let Some(workspaces) = self.task_workspaces_for(task) else {
            return Ok(Vec::new());
        };
        let mut targets = Vec::new();
        for repo in &workspaces.repos {
            let Some(worktree) = &repo.worktree else {
                continue;
            };
            if !worktree.dir.is_dir() {
                continue;
            }
            let Some(reference) = task.repos.iter().find(|r| r.name == repo.name) else {
                continue;
            };
            let target_ref = self.review_target_ref(task, reference.repo_id, &repo.source)?;
            targets.push((reference.repo_id, worktree.dir.clone(), target_ref));
        }
        Ok(targets)
    }
}
