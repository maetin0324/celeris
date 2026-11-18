//! ADR-0118 / ADR-0130 D4: the review sync target of a repo and the local Git worktrees the review
//! sync of a task rebases. Reads Git only; the entries in `review_spawn.rs` read the repo's default
//! branch and the branch prefix and record the behind measurement (ADR 2026-11-18
//! dispatcher-store-access-pilot).

use std::path::{Path, PathBuf};

use task_core::{RepoId, Task};
use task_worker::TaskWorkspaces;

/// Review sync target ref of one repo: the parent task branch for a tree child (`parent`), otherwise
/// the repo's default branch. `None` when the default branch cannot be resolved
/// (ADR 2026-10-11-stranded-done-branches F1): the repo is left out of the review sync.
pub(super) fn review_target(
    parent: Option<String>,
    configured_default: Option<&str>,
    source: &Path,
) -> Option<String> {
    let target = match parent {
        Some(parent) => parent,
        None => match task_ops::changes::resolve_default_branch(source, configured_default) {
            Ok(branch) => branch,
            Err(error) => {
                tracing::warn!(source = %source.display(), %error, "review sync target skipped");
                return None;
            }
        },
    };
    Some(format!("refs/heads/{target}"))
}

/// `(repo, worktree, source)` of every repo of `task` whose local Git worktree exists. Remote /
/// projectless / non-Git workspaces have none (behind stays `null`).
pub(super) fn sync_sites(task: &Task, ws: &TaskWorkspaces) -> Vec<(RepoId, PathBuf, PathBuf)> {
    let mut sites = Vec::new();
    for repo in &ws.repos {
        let Some(worktree) = repo.worktree.as_ref().filter(|w| w.dir.is_dir()) else {
            continue;
        };
        if let Some(reference) = task.repos.iter().find(|r| r.name == repo.name) {
            sites.push((reference.repo_id, worktree.dir.clone(), repo.source.clone()));
        }
    }
    sites
}
